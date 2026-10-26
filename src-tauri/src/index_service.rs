//! バックグラウンドのインデックスサービス。
//!
//! 専用スレッドが SQLite への書き込みを一手に引き受け、コマンドをチャネルで受け取る。
//! - 作業フォルダを開いたら全体走査 → 埋め込み
//! - フォルダ監視の通知で差分だけ更新
//! - 埋め込みサーバーに繋がらなければ 1 分ごとに再試行(全文検索はその間も使える)
//!
//! 進捗と状態は `index-status` イベントで画面に送る。

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use memo_core::settings::AppSettings;
use memo_index::{
    embed::{Embedder, ProviderEmbedder},
    extract::pdfium_status,
    indexer::{self, IndexOptions, Phase, Progress, ScanReport, VectorState},
    store::{Stats, Store},
    watch::{self, Watcher},
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const INDEX_FILE: &str = "index.sqlite";

pub enum Cmd {
    Open(PathBuf),
    FullScan,
    Paths(Vec<String>),
    RebuildVectors,
    Settings(Box<AppSettings>),
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexStatus {
    pub root: Option<String>,
    pub progress: Progress,
    pub stats: Stats,
    pub vector: VectorState,
    /// PDFium の状態。None なら利用可能
    pub pdf_note: Option<String>,
    pub last_report: Option<ScanReport>,
    pub last_error: Option<String>,
    /// 抽出に失敗したファイル(先頭 20 件)
    pub failures: Vec<(String, String)>,
}

impl Default for IndexStatus {
    fn default() -> Self {
        Self {
            root: None,
            progress: Progress { phase: Phase::Idle, done: 0, total: 0, current: None },
            stats: Stats::default(),
            vector: VectorState::Ready,
            pdf_note: None,
            last_report: None,
            last_error: None,
            failures: vec![],
        }
    }
}

#[derive(Clone)]
pub struct IndexService {
    tx: Sender<Cmd>,
    cancel: Arc<AtomicBool>,
    pub status: Arc<Mutex<IndexStatus>>,
}

impl IndexService {
    pub fn start(app: AppHandle, settings: AppSettings) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(IndexStatus {
            pdf_note: pdf_note(),
            ..Default::default()
        }));
        let svc = Self { tx: tx.clone(), cancel: cancel.clone(), status: status.clone() };
        std::thread::Builder::new()
            .name("memo-indexer".into())
            .spawn(move || Worker { app, tx, rx, cancel, status, settings, ws: None, last_emit: None }.run())
            .expect("indexer thread");
        svc
    }

    pub fn send(&self, cmd: Cmd) {
        if matches!(cmd, Cmd::Open(_) | Cmd::RebuildVectors) {
            // 実行中の重い処理を打ち切ってから
            self.cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.tx.send(cmd);
    }
}

fn pdf_note() -> Option<String> {
    match pdfium_status() {
        Some(Ok(())) => None,
        Some(Err(e)) => Some(format!("PDF は検索対象外です(PDFium {e})")),
        None => Some("PDFium が初期化されていません".into()),
    }
}

pub fn embedder_from(settings: &AppSettings) -> Option<ProviderEmbedder> {
    let cfg = settings.roles.embedding.clone();
    let provider = settings.provider(&cfg.provider)?.clone();
    Some(ProviderEmbedder { provider, cfg })
}

pub fn index_options(settings: &AppSettings) -> IndexOptions {
    IndexOptions {
        max_file_bytes: settings.search.max_file_mb * 1024 * 1024,
        exclude_dirs: settings.search.exclude_dirs.clone(),
    }
}

struct OpenWorkspace {
    root: PathBuf,
    store: Store,
    _watcher: Option<Watcher>,
}

struct Worker {
    app: AppHandle,
    tx: Sender<Cmd>,
    rx: Receiver<Cmd>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<IndexStatus>>,
    settings: AppSettings,
    ws: Option<OpenWorkspace>,
    last_emit: Option<Instant>,
}

impl Worker {
    fn run(mut self) {
        loop {
            let cmd = match self.rx.recv_timeout(Duration::from_secs(60)) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) => {
                    // 埋め込みサーバーが落ちていた場合の再試行
                    let retry = matches!(self.status.lock().unwrap().vector, VectorState::Unavailable { .. });
                    if retry && self.ws.is_some() {
                        self.embed();
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            };
            self.cancel.store(false, Ordering::Relaxed);
            let result = match cmd {
                Cmd::Open(root) => self.open(root),
                Cmd::FullScan => self.full_scan(),
                Cmd::Paths(mut paths) => {
                    // 連続して届いた通知をまとめて処理
                    while let Ok(Cmd::Paths(more)) = self.rx.try_recv() {
                        paths.extend(more);
                    }
                    paths.sort();
                    paths.dedup();
                    self.update(paths)
                }
                Cmd::RebuildVectors => self.rebuild_vectors(),
                Cmd::Settings(s) => {
                    self.settings = *s;
                    self.refresh_vector_state();
                    self.embed();
                    Ok(())
                }
            };
            if let Err(e) = result {
                self.status.lock().unwrap().last_error = Some(e);
            }
            self.set_phase(Phase::Idle);
            self.emit(true);
        }
    }

    fn open(&mut self, root: PathBuf) -> Result<(), String> {
        self.ws = None;
        let meta = root.join(memo_core::paths::META_DIR);
        std::fs::create_dir_all(&meta).map_err(|e| e.to_string())?;
        let store = Store::open(&meta.join(INDEX_FILE)).map_err(|e| e.to_string())?;
        let tx = self.tx.clone();
        let watcher = watch::watch(&root, self.settings.search.exclude_dirs.clone(), move |paths| {
            let _ = tx.send(Cmd::Paths(paths));
        });
        let watch_err = watcher.as_ref().err().map(|e| e.to_string());
        {
            let mut st = self.status.lock().unwrap();
            *st = IndexStatus { root: Some(root.display().to_string()), pdf_note: pdf_note(), ..Default::default() };
            st.last_error = watch_err;
        }
        self.ws = Some(OpenWorkspace { root, store, _watcher: watcher.ok() });
        self.refresh_stats();
        self.emit(true);
        self.full_scan()
    }

    fn full_scan(&mut self) -> Result<(), String> {
        let opts = index_options(&self.settings);
        let report = {
            let Some(ws) = self.ws.as_mut() else { return Ok(()) };
            let (status, app) = (self.status.clone(), self.app.clone());
            // 進捗イベントは 150ms に 1 回まで(Fn クロージャ内で更新するので Cell)
            let last = std::cell::Cell::new(Instant::now() - Duration::from_secs(1));
            let progress = |p: Progress| {
                status.lock().unwrap().progress = p;
                if last.get().elapsed() > Duration::from_millis(150) {
                    last.set(Instant::now());
                    let _ = app.emit("index-status", status.lock().unwrap().clone());
                }
            };
            indexer::full_scan(&ws.root, &mut ws.store, &opts, &self.cancel, &progress).map_err(|e| e.to_string())?
        };
        self.status.lock().unwrap().last_report = Some(report);
        self.refresh_stats();
        self.embed();
        Ok(())
    }

    fn update(&mut self, paths: Vec<String>) -> Result<(), String> {
        let opts = index_options(&self.settings);
        let Some(ws) = self.ws.as_mut() else { return Ok(()) };
        indexer::update_paths(&ws.root, &mut ws.store, &opts, &paths).map_err(|e| e.to_string())?;
        self.refresh_stats();
        self.embed();
        Ok(())
    }

    fn rebuild_vectors(&mut self) -> Result<(), String> {
        let Some(embedder) = embedder_from(&self.settings) else { return Ok(()) };
        let Some(ws) = self.ws.as_mut() else { return Ok(()) };
        ws.store.reset_vectors(&embedder.model_key()).map_err(|e| e.to_string())?;
        self.status.lock().unwrap().vector = VectorState::Ready;
        self.refresh_stats();
        self.embed();
        Ok(())
    }

    fn refresh_vector_state(&mut self) {
        let (Some(ws), Some(e)) = (self.ws.as_ref(), embedder_from(&self.settings)) else { return };
        if let Ok(v) = indexer::vector_state(&ws.store, &e) {
            self.status.lock().unwrap().vector = v;
        }
    }

    fn embed(&mut self) {
        let Some(embedder) = embedder_from(&self.settings) else { return };
        let Some(ws) = self.ws.as_mut() else { return };
        let pending = ws.store.stats().map(|s| s.pending_embeddings).unwrap_or(0);
        if pending == 0 {
            let v = indexer::vector_state(&ws.store, &embedder).unwrap_or(VectorState::Ready);
            self.status.lock().unwrap().vector = v;
            return;
        }
        let (status, app) = (self.status.clone(), self.app.clone());
        let last = std::cell::Cell::new(Instant::now() - Duration::from_secs(1));
        let progress = |p: Progress| {
            status.lock().unwrap().progress = p;
            if last.get().elapsed() > Duration::from_millis(300) {
                last.set(Instant::now());
                let _ = app.emit("index-status", status.lock().unwrap().clone());
            }
        };
        let v = indexer::embed_pending(&mut ws.store, &embedder, &self.cancel, &progress)
            .unwrap_or_else(|e| VectorState::Unavailable { message: e.to_string() });
        self.status.lock().unwrap().vector = v;
        self.refresh_stats();
    }

    fn refresh_stats(&mut self) {
        let Some(ws) = self.ws.as_ref() else { return };
        let stats = ws.store.stats().unwrap_or_default();
        let failures = ws.store.errors(20).unwrap_or_default();
        let mut st = self.status.lock().unwrap();
        st.stats = stats;
        st.failures = failures;
    }

    fn set_phase(&mut self, phase: Phase) {
        let mut st = self.status.lock().unwrap();
        st.progress = Progress { phase, done: 0, total: 0, current: None };
    }

    fn emit(&mut self, force: bool) {
        if !force && self.last_emit.is_some_and(|t| t.elapsed() < Duration::from_millis(150)) {
            return;
        }
        self.last_emit = Some(Instant::now());
        let _ = self.app.emit("index-status", self.status.lock().unwrap().clone());
    }
}
