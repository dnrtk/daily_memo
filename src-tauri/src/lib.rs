//! Tauri コマンド層。ロジックはすべて memo-core / memo-index 側に置き、ここは薄く保つ。

mod index_service;
mod terminal;

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use memo_core::{
    category::CategoryConfig,
    llm,
    settings::{AppSettings, Provider},
    text::TextFile,
    workspace::{DirEntry, SaveRequest, Saved, Workspace},
};
use index_service::{embedder_from, Cmd, IndexService, IndexStatus, INDEX_FILE};
use memo_index::{
    embed::Embedder,
    search::{self, CategoryInfo, SearchResponse},
    store::{ChunkRow, Store},
};
use serde::Serialize;
use tauri::{Emitter, Manager, RunEvent, State, WindowEvent};
use terminal::{Snapshot, Spawned, Terminals};

static WINDOW_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
use tauri_plugin_opener::OpenerExt;

type CmdResult<T> = Result<T, String>;

struct AppState {
    workspace: Mutex<Option<Workspace>>,
    settings: Mutex<AppSettings>,
    config_dir: PathBuf,
    index: IndexService,
    /// 検索用の読み取り接続(作業フォルダごとに開き直す)
    search_store: Mutex<Option<(PathBuf, Store)>>,
}

#[derive(Serialize)]
struct WorkspaceInfo {
    root: String,
    name: String,
    categories: CategoryConfig,
}

fn info(ws: &Workspace) -> WorkspaceInfo {
    WorkspaceInfo {
        root: ws.root().display().to_string(),
        name: ws
            .root()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        categories: ws.config().clone(),
    }
}

fn now() -> chrono::NaiveDateTime {
    chrono::Local::now().naive_local()
}

/// Markdown プレビューで作業フォルダ内の画像を表示できるようにする(asset プロトコルの許可範囲)
fn allow_assets(app: &tauri::AppHandle, root: &Path) {
    if let Err(e) = app.asset_protocol_scope().allow_directory(root, true) {
        eprintln!("画像の表示許可を設定できません: {e}");
    }
}

/// 作業フォルダを開いた状態で f を実行する
fn with_ws<T>(state: &State<AppState>, f: impl FnOnce(&mut Workspace) -> memo_core::Result<T>) -> CmdResult<T> {
    let mut guard = state.workspace.lock().map_err(|e| e.to_string())?;
    let ws = guard.as_mut().ok_or("作業フォルダが開かれていません")?;
    f(ws).map_err(|e| e.to_string())
}

// ---- 作業フォルダ --------------------------------------------------------------

#[tauri::command]
fn open_workspace(app: tauri::AppHandle, state: State<AppState>, path: String) -> CmdResult<WorkspaceInfo> {
    let ws = Workspace::open(&path).map_err(|e| e.to_string())?;
    let out = info(&ws);
    allow_assets(&app, ws.root());
    state.index.send(Cmd::Open(ws.root().to_path_buf()));
    *state.workspace.lock().map_err(|e| e.to_string())? = Some(ws);

    let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
    settings.last_workspace = Some(out.root.clone());
    settings.save(&state.config_dir).map_err(|e| e.to_string())?;
    Ok(out)
}

#[tauri::command]
fn current_workspace(state: State<AppState>) -> CmdResult<Option<WorkspaceInfo>> {
    Ok(state.workspace.lock().map_err(|e| e.to_string())?.as_ref().map(info))
}

#[tauri::command]
fn save_categories(state: State<AppState>, config: CategoryConfig) -> CmdResult<WorkspaceInfo> {
    with_ws(&state, |ws| {
        ws.update_categories(config)?;
        Ok(info(ws))
    })
}

// ---- ファイル ------------------------------------------------------------------

#[tauri::command]
fn list_dir(state: State<AppState>, path: String) -> CmdResult<Vec<DirEntry>> {
    with_ws(&state, |ws| ws.list_dir(&path))
}

#[tauri::command]
fn read_file(state: State<AppState>, path: String) -> CmdResult<TextFile> {
    with_ws(&state, |ws| ws.read_text(&path))
}

#[tauri::command]
fn write_file(state: State<AppState>, path: String, file: TextFile) -> CmdResult<()> {
    with_ws(&state, |ws| ws.write_text(&path, &file))
}

#[tauri::command]
fn create_file(state: State<AppState>, path: String) -> CmdResult<String> {
    with_ws(&state, |ws| ws.create_file(&path))
}

#[tauri::command]
fn create_dir(state: State<AppState>, path: String) -> CmdResult<String> {
    with_ws(&state, |ws| ws.create_dir(&path))
}

#[tauri::command]
fn rename_path(state: State<AppState>, from: String, to: String) -> CmdResult<String> {
    with_ws(&state, |ws| ws.rename(&from, &to))
}

#[tauri::command]
fn trash_path(state: State<AppState>, path: String) -> CmdResult<()> {
    with_ws(&state, |ws| ws.trash(&path))
}

/// Office / PDF などを OS の既定アプリで開く
#[tauri::command]
fn open_external(app: tauri::AppHandle, state: State<AppState>, path: String) -> CmdResult<()> {
    let abs = with_ws(&state, |ws| ws.abs_path(&path))?;
    app.opener()
        .open_path(abs.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_in_folder(app: tauri::AppHandle, state: State<AppState>, path: String) -> CmdResult<()> {
    let abs = with_ws(&state, |ws| ws.abs_path(&path))?;
    app.opener().reveal_item_in_dir(abs).map_err(|e| e.to_string())
}

// ---- メモ保存 --------------------------------------------------------------------

#[tauri::command]
fn open_daily_note(state: State<AppState>) -> CmdResult<String> {
    with_ws(&state, |ws| ws.daily_note(now()))
}

#[tauri::command]
fn save_to_category(state: State<AppState>, category_id: String, request: SaveRequest) -> CmdResult<Saved> {
    with_ws(&state, |ws| ws.save_to_category(&category_id, &request, now()))
}

// ---- 設定 / LLM ----------------------------------------------------------------

#[tauri::command]
fn get_settings(state: State<AppState>) -> CmdResult<AppSettings> {
    Ok(state.settings.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: AppSettings) -> CmdResult<AppSettings> {
    let mut current = state.settings.lock().map_err(|e| e.to_string())?;
    let mut next = settings;
    next.last_workspace = current.last_workspace.clone();
    next.save(&state.config_dir).map_err(|e| e.to_string())?;
    *current = next.clone();
    state.index.send(Cmd::Settings(Box::new(next.clone())));
    Ok(next)
}

#[tauri::command]
async fn test_provider(provider: Provider) -> CmdResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || llm::list_models(&provider))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ---- 検索 ------------------------------------------------------------------------

#[tauri::command]
fn index_status(state: State<AppState>) -> CmdResult<IndexStatus> {
    Ok(state.index.status.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn index_rescan(state: State<AppState>) {
    state.index.send(Cmd::FullScan);
}

#[tauri::command]
fn index_rebuild_vectors(state: State<AppState>) {
    state.index.send(Cmd::RebuildVectors);
}

#[derive(Serialize)]
struct SearchResult {
    #[serde(flatten)]
    response: SearchResponse,
    elapsed_ms: u128,
}

#[tauri::command]
async fn search(app: tauri::AppHandle, query: String, use_llm: bool) -> CmdResult<SearchResult> {
    tauri::async_runtime::spawn_blocking(move || run_search(&app, &query, use_llm))
        .await
        .map_err(|e| e.to_string())?
}

fn run_search(app: &tauri::AppHandle, query: &str, use_llm: bool) -> CmdResult<SearchResult> {
    let started = std::time::Instant::now();
    let state = app.state::<AppState>();
    let settings = state.settings.lock().map_err(|e| e.to_string())?.clone();
    let (root, cats) = {
        let guard = state.workspace.lock().map_err(|e| e.to_string())?;
        let ws = guard.as_ref().ok_or("作業フォルダが開かれていません")?;
        let cats: Vec<CategoryInfo> = ws
            .config()
            .categories
            .iter()
            .map(|c| CategoryInfo { id: c.id.clone(), name: c.name.clone(), dir: c.dir.clone(), description: c.description.clone() })
            .collect();
        (ws.root().to_path_buf(), cats)
    };

    // クエリ解釈(LLM)。対話的な操作なので待ち時間は短めに切る
    let plan = match (use_llm && settings.search.use_llm_query, settings.provider(&settings.roles.chat.provider)) {
        (true, Some(p)) => {
            let mut p = p.clone();
            p.timeout_secs = p.timeout_secs.min(20);
            let model = settings.roles.chat.model.clone();
            let today = chrono::Local::now().date_naive();
            search::llm_plan(query, today, &cats, |sys, user| {
                llm::chat_json(&p, &model, sys, user).map_err(|e| e.to_string())
            })
        }
        _ => search::plain_plan(query),
    };

    let embedder = embedder_from(&settings);
    let response = with_search_store(&state, &root, |store| {
        search::search(store, plan, embedder.as_ref().map(|e| e as &dyn Embedder), 30).map_err(|e| e.to_string())
    })?;
    Ok(SearchResult { response, elapsed_ms: started.elapsed().as_millis() })
}

/// 検索用の読み取り接続で f を実行する(作業フォルダが変わっていたら開き直す)
fn with_search_store<T>(state: &AppState, root: &Path, f: impl FnOnce(&Store) -> CmdResult<T>) -> CmdResult<T> {
    let mut guard = state.search_store.lock().map_err(|e| e.to_string())?;
    if guard.as_ref().is_none_or(|(r, _)| r != root) {
        let path = root.join(memo_core::paths::META_DIR).join(INDEX_FILE);
        *guard = Some((root.to_path_buf(), open_store(&path)?));
    }
    f(&guard.as_ref().unwrap().1)
}

/// インデックス済み文書(Word / Excel / PowerPoint / PDF 等)の抽出テキストを、位置ラベル付きで順に返す
#[tauri::command]
fn doc_chunks(state: State<AppState>, path: String) -> CmdResult<Vec<ChunkRow>> {
    let root = with_ws(&state, |ws| Ok(ws.root().to_path_buf()))?;
    with_search_store(&state, &root, |store| store.file_chunks(&path).map_err(|e| e.to_string()))
}

fn open_store(path: &Path) -> CmdResult<Store> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    Store::open(path).map_err(|e| e.to_string())
}

// ---- 作業フォルダを OS のファイルマネージャで開く --------------------------------

/// Windows はエクスプローラー、Linux は既定のファイルマネージャ(xdg-open)
#[tauri::command]
fn open_workspace_folder(app: tauri::AppHandle, state: State<AppState>) -> CmdResult<()> {
    let root = with_ws(&state, |ws| Ok(ws.root().to_path_buf()))?;
    app.opener()
        .open_path(root.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// ターミナル内の URL をブラウザで開く(http / https のみ)
#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> CmdResult<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("http / https 以外の URL は開けません".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ---- ターミナル ------------------------------------------------------------------

#[tauri::command]
fn term_spawn(app: tauri::AppHandle, state: State<AppState>, terms: State<Terminals>, cols: u16, rows: u16) -> CmdResult<Spawned> {
    let cwd = state
        .workspace
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|ws| ws.root().to_path_buf());
    let shell = state.settings.lock().map_err(|e| e.to_string())?.terminal.shell.clone();
    terms.spawn(app, cwd, &shell, cols, rows)
}

#[tauri::command]
fn term_since(terms: State<Terminals>, id: u32, offset: u64) -> CmdResult<Snapshot> {
    terms.since(id, offset)
}

#[tauri::command]
fn term_snapshot(terms: State<Terminals>, id: u32) -> CmdResult<Snapshot> {
    terms.snapshot(id)
}

#[tauri::command]
fn term_write(terms: State<Terminals>, id: u32, data: String) -> CmdResult<()> {
    terms.write(id, &data)
}

#[tauri::command]
fn term_resize(terms: State<Terminals>, id: u32, cols: u16, rows: u16) -> CmdResult<()> {
    terms.resize(id, cols, rows)
}

#[tauri::command]
fn term_kill(terms: State<Terminals>, id: u32) {
    terms.kill(id);
}

// ---- 別ウィンドウ ----------------------------------------------------------------

/// タブを別ウィンドウに切り離す。payload は画面側でそのまま使う(kind: "editor" | "terminal" など)。
/// Windows では同期コマンドからウィンドウを作るとデッドロックすることがあるので async にしている
#[tauri::command]
async fn detach_window(app: tauri::AppHandle, payload: serde_json::Value, title: String) -> CmdResult<String> {
    let n = WINDOW_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let label = format!("w-{n}");
    let init = format!(
        "window.__MEMO_VIEW__ = {};",
        serde_json::to_string(&serde_json::json!({ "label": label, "payload": payload })).map_err(|e| e.to_string())?
    );
    tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("index.html".into()))
        .title(&title)
        .inner_size(960.0, 680.0)
        .min_inner_size(420.0, 260.0)
        .initialization_script(init)
        .build()
        .map_err(|e| format!("ウィンドウを作れません: {e}"))?;
    Ok(label)
}

/// 切り離したウィンドウのタブをメインに戻す(メインへ通知してから、このウィンドウを閉じる)
#[tauri::command]
fn reattach(app: tauri::AppHandle, window: tauri::WebviewWindow, payload: serde_json::Value) -> CmdResult<()> {
    app.emit_to("main", "reattach", payload).map_err(|e| e.to_string())?;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
    // destroy は閉じる前の確認処理(ターミナルの終了など)を通らない
    window.destroy().map_err(|e| e.to_string())
}

#[tauri::command]
fn focus_window(app: tauri::AppHandle, label: String) -> CmdResult<()> {
    let w = app.get_webview_window(&label).ok_or("ウィンドウが見つかりません")?;
    let _ = w.unminimize();
    w.set_focus().map_err(|e| e.to_string())
}

/// 切り離したウィンドウを全部閉じる(作業フォルダを切り替えるとき)。各ウィンドウは閉じる前に保存する
#[tauri::command]
fn close_detached_windows(app: tauri::AppHandle) {
    for (label, w) in app.webview_windows() {
        if label != "main" {
            let _ = w.close();
        }
    }
}

/// PDFium を探す場所: 環境変数 → 同梱リソース → 実行ファイルの隣 → 開発時の src-tauri/pdfium
fn pdfium_candidates(app: &tauri::App) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::var_os("MEMO_PDFIUM_DIR") {
        v.push(PathBuf::from(dir));
    }
    if let Ok(res) = app.path().resource_dir() {
        v.push(res.join("pdfium"));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        v.push(exe_dir.join("pdfium"));
        v.push(exe_dir);
    }
    v.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("pdfium"));
    v
}

// ---- 起動 ------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let settings = AppSettings::load_or_default(&config_dir).unwrap_or_else(|e| {
                eprintln!("設定の読み込みに失敗したため既定値で起動します: {e}");
                AppSettings::default()
            });
            if let Err(e) = memo_index::extract::init_pdfium(&pdfium_candidates(app)) {
                eprintln!("PDFium を読み込めませんでした。PDF は検索対象外になります: {e}");
            }
            let index = IndexService::start(app.handle().clone(), settings.clone());
            // 前回の作業フォルダを復元(消えていたら無視)
            let workspace = settings
                .last_workspace
                .as_deref()
                .and_then(|p| Workspace::open(p).ok());
            if let Some(ws) = &workspace {
                allow_assets(app.handle(), ws.root());
                index.send(Cmd::Open(ws.root().to_path_buf()));
            }
            app.manage(Terminals::default());
            app.manage(AppState {
                workspace: Mutex::new(workspace),
                settings: Mutex::new(settings),
                config_dir,
                index,
                search_store: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_workspace,
            current_workspace,
            save_categories,
            list_dir,
            read_file,
            write_file,
            create_file,
            create_dir,
            rename_path,
            trash_path,
            open_external,
            reveal_in_folder,
            open_daily_note,
            save_to_category,
            get_settings,
            save_settings,
            test_provider,
            index_status,
            index_rescan,
            index_rebuild_vectors,
            search,
            doc_chunks,
            open_workspace_folder,
            open_url,
            term_spawn,
            term_snapshot,
            term_since,
            detach_window,
            reattach,
            close_detached_windows,
            focus_window,
            term_write,
            term_resize,
            term_kill,
        ])
        .on_window_event(|window, event| {
            // メインを閉じたら切り離したウィンドウも閉じる(それぞれ保存してから閉じる)
            if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
                for (label, w) in window.app_handle().webview_windows() {
                    if label != "main" {
                        let _ = w.close();
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building memo-editor")
        .run(|app, event| {
            // 終了時にシェルを残さない
            if let RunEvent::Exit = event {
                app.state::<Terminals>().kill_all();
            }
        });
}
