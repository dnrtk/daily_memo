//! 差分インデックス。
//!
//! 1. 作業フォルダを走査し、対象拡張子のファイルを列挙
//! 2. サイズ・更新時刻が DB と違うものだけハッシュを取り、内容が変わっていれば再抽出
//! 3. 消えたファイルは DB から削除
//! 4. 埋め込み待ちのチャンクをまとめて埋め込む(サーバーが落ちていても全文検索は使える)
//!
//! 長時間かかるので進捗コールバックとキャンセルフラグを受け取る。

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::UNIX_EPOCH,
};

use serde::Serialize;
use walkdir::WalkDir;

use crate::{
    chunk,
    embed::Embedder,
    extract::{self, kind_for},
    store::Store,
    IndexError, Result,
};

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub max_file_bytes: u64,
    pub exclude_dirs: Vec<String>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self { max_file_bytes: 50 * 1024 * 1024, exclude_dirs: vec!["node_modules".into()] }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Scanning,
    Extracting,
    Embedding,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
    pub current: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanReport {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub failed: usize,
    /// PDFium が使えずに後回しにした PDF
    pub skipped_pdf: usize,
}

/// ベクトル検索の状態
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VectorState {
    Ready,
    /// インデックスのモデルと設定のモデルが違う(再作成が必要)
    ModelMismatch { indexed: String, configured: String },
    /// 埋め込みサーバーに接続できない等
    Unavailable { message: String },
}

/// 対象ファイル(作業フォルダ相対パス, 絶対パス)を列挙する
pub fn list_files(root: &Path, opts: &IndexOptions) -> Vec<(String, PathBuf)> {
    let excluded = |name: &str| name.starts_with('.') || opts.exclude_dirs.iter().any(|d| d == name);
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0 || {
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_dir() {
                    !excluded(&name)
                } else {
                    !name.starts_with('.') && !name.starts_with("~$")
                }
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && kind_for(e.path()).is_some())
        .map(|e| (memo_core::paths::to_rel(root, e.path()), e.into_path()))
        .collect()
}

fn stat(path: &Path) -> Option<(i64, i64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((md.len() as i64, mtime))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

enum Outcome {
    Indexed,
    Unchanged,
    Failed,
    SkippedPdf,
}

/// 1 ファイルを(必要なら)インデックスし直す
fn index_one(store: &mut Store, rel: &str, abs: &Path, opts: &IndexOptions) -> Result<Outcome> {
    let Some((size, mtime)) = stat(abs) else { return Ok(Outcome::Unchanged) };
    let prev = store.file_state(rel)?;
    if let Some(p) = &prev {
        if p.size == size && p.mtime == mtime {
            return Ok(Outcome::Unchanged);
        }
    }
    let kind = kind_for(abs).unwrap_or("text");
    if size as u64 > opts.max_file_bytes {
        let msg = format!("サイズ上限({} MB)を超えています", opts.max_file_bytes / 1024 / 1024);
        store.replace_file(rel, kind, size, mtime, "", Some(&msg), &[])?;
        return Ok(Outcome::Failed);
    }
    let hash = hash_file(abs)?;
    if let Some(p) = &prev {
        if p.hash.as_deref() == Some(hash.as_str()) {
            store.touch_file(p.id, size, mtime)?;
            return Ok(Outcome::Unchanged);
        }
    }
    match extract::extract(abs) {
        Ok(Some(segments)) => {
            let chunks = chunk::chunk(segments);
            store.replace_file(rel, kind, size, mtime, &hash, None, &chunks)?;
            Ok(Outcome::Indexed)
        }
        Ok(None) => Ok(Outcome::Unchanged),
        // PDFium が無いだけなら記録しない(使えるようになったら次の走査で拾う)
        Err(IndexError::PdfiumUnavailable(_)) => Ok(Outcome::SkippedPdf),
        Err(e) => {
            store.replace_file(rel, kind, size, mtime, &hash, Some(&e.to_string()), &[])?;
            Ok(Outcome::Failed)
        }
    }
}

fn tally(report: &mut ScanReport, o: Outcome) {
    match o {
        Outcome::Indexed => report.indexed += 1,
        Outcome::Unchanged => report.unchanged += 1,
        Outcome::Failed => report.failed += 1,
        Outcome::SkippedPdf => report.skipped_pdf += 1,
    }
}

/// 作業フォルダ全体を走査して差分を反映する
pub fn full_scan(
    root: &Path,
    store: &mut Store,
    opts: &IndexOptions,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<ScanReport> {
    progress(Progress { phase: Phase::Scanning, done: 0, total: 0, current: None });
    let files = list_files(root, opts);
    let mut report = ScanReport { scanned: files.len(), ..Default::default() };

    let on_disk: HashSet<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
    for path in store.file_states()?.into_keys() {
        if !on_disk.contains(path.as_str()) && store.delete_file(&path)? {
            report.removed += 1;
        }
    }

    let total = files.len();
    for (i, (rel, abs)) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        progress(Progress { phase: Phase::Extracting, done: i, total, current: Some(rel.clone()) });
        let o = index_one(store, rel, abs, opts)?;
        tally(&mut report, o);
    }
    progress(Progress { phase: Phase::Extracting, done: total, total, current: None });
    Ok(report)
}

/// 変更通知のあったパスだけ反映する(フォルダ監視から呼ぶ)
pub fn update_paths(root: &Path, store: &mut Store, opts: &IndexOptions, rel_paths: &[String]) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let mut dirs = Vec::new();
    for rel in rel_paths {
        let abs = memo_core::paths::resolve(root, rel)?;
        if abs.is_dir() {
            dirs.push(rel.clone());
            continue;
        }
        if abs.is_file() && kind_for(&abs).is_some() {
            report.scanned += 1;
            let o = index_one(store, rel, &abs, opts)?;
            tally(&mut report, o);
        } else if !abs.exists() {
            // ファイルが消えた、またはフォルダごと消えた / 移動した
            let prefix = format!("{rel}/");
            for path in store.file_states()?.into_keys() {
                if (path == *rel || path.starts_with(&prefix)) && store.delete_file(&path)? {
                    report.removed += 1;
                }
            }
        }
    }
    // フォルダが現れた(移動してきた)場合は中身を拾う
    for dir in dirs {
        let abs = memo_core::paths::resolve(root, &dir)?;
        for (rel, file) in list_files(root, opts).into_iter().filter(|(_, p)| p.starts_with(&abs)) {
            report.scanned += 1;
            let o = index_one(store, &rel, &file, opts)?;
            tally(&mut report, o);
        }
    }
    Ok(report)
}

/// 現在の設定モデルでベクトル検索できるか判定する(埋め込みは行わない)
pub fn vector_state(store: &Store, embedder: &dyn Embedder) -> Result<VectorState> {
    let configured = embedder.model_key();
    Ok(match store.meta("embed_model")? {
        Some(indexed) if indexed != configured => VectorState::ModelMismatch { indexed, configured },
        _ => VectorState::Ready,
    })
}

/// 埋め込み待ちのチャンクを処理する
pub fn embed_pending(
    store: &mut Store,
    embedder: &dyn Embedder,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<VectorState> {
    const BATCH: usize = 16;
    let configured = embedder.model_key();
    match store.meta("embed_model")? {
        None => store.set_meta("embed_model", &configured)?,
        Some(indexed) if indexed != configured => {
            return Ok(VectorState::ModelMismatch { indexed, configured });
        }
        _ => {}
    }
    let total = store.stats()?.pending_embeddings as usize;
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let batch = store.pending_chunks(BATCH)?;
        if batch.is_empty() {
            break;
        }
        progress(Progress { phase: Phase::Embedding, done, total, current: None });
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let vectors = match embedder.embed_documents(&texts) {
            Ok(v) => v,
            Err(e) => return Ok(VectorState::Unavailable { message: e.to_string() }),
        };
        let Some(dim) = vectors.first().map(Vec::len) else { break };
        if let Some(existing) = store.vec_dim()? {
            if existing != dim {
                return Ok(VectorState::Unavailable {
                    message: format!("ベクトルの次元が変わりました({existing} → {dim})。ベクトルを作り直してください"),
                });
            }
        }
        store.ensure_vec_table(dim)?;
        let items: Vec<(i64, Vec<f32>)> = batch.iter().map(|(id, _)| *id).zip(vectors).collect();
        store.set_embeddings(&items)?;
        done += items.len();
    }
    progress(Progress { phase: Phase::Idle, done, total, current: None });
    Ok(VectorState::Ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;

    fn ws() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("技術")).unwrap();
        std::fs::create_dir_all(p.join(".memo")).unwrap();
        std::fs::create_dir_all(p.join("node_modules/x")).unwrap();
        std::fs::write(p.join("技術/ollama.md"), "# Ollama\n\nモデル保存先は OLLAMA_MODELS で変える\n").unwrap();
        std::fs::write(p.join("node_modules/x/readme.md"), "除外").unwrap();
        std::fs::write(p.join(".memo/x.md"), "除外").unwrap();
        std::fs::write(p.join("~$lock.docx"), "除外").unwrap();
        std::fs::copy(crate::extract::fixture("estimate.xlsx"), p.join("estimate.xlsx")).unwrap();
        dir
    }

    fn noop(_: Progress) {}

    #[test]
    fn scan_is_incremental() {
        let dir = ws();
        let mut store = Store::open_in_memory().unwrap();
        let opts = IndexOptions::default();
        let cancel = AtomicBool::new(false);

        let r = full_scan(dir.path(), &mut store, &opts, &cancel, &noop).unwrap();
        assert_eq!((r.scanned, r.indexed), (2, 2));

        // 変更なし
        let r = full_scan(dir.path(), &mut store, &opts, &cancel, &noop).unwrap();
        assert_eq!((r.indexed, r.unchanged), (0, 2));

        // 内容変更 → 再インデックス、削除 → 除去
        std::fs::write(dir.path().join("技術/ollama.md"), "# Ollama\n\n変更後の内容です。長さも変える\n").unwrap();
        std::fs::remove_file(dir.path().join("estimate.xlsx")).unwrap();
        let r = full_scan(dir.path(), &mut store, &opts, &cancel, &noop).unwrap();
        assert_eq!((r.indexed, r.removed), (1, 1));
        assert_eq!(store.stats().unwrap().files, 1);
    }

    #[test]
    fn update_paths_handles_moves() {
        let dir = ws();
        let mut store = Store::open_in_memory().unwrap();
        let opts = IndexOptions::default();
        full_scan(dir.path(), &mut store, &opts, &AtomicBool::new(false), &noop).unwrap();

        std::fs::rename(dir.path().join("技術"), dir.path().join("tech")).unwrap();
        let r = update_paths(dir.path(), &mut store, &opts, &["技術".into(), "tech".into()]).unwrap();
        assert_eq!((r.removed, r.indexed), (1, 1));
        assert!(store.file_state("tech/ollama.md").unwrap().is_some());
        assert!(store.file_state("技術/ollama.md").unwrap().is_none());
    }

    #[test]
    fn embedding_and_model_change() {
        let dir = ws();
        let mut store = Store::open_in_memory().unwrap();
        full_scan(dir.path(), &mut store, &IndexOptions::default(), &AtomicBool::new(false), &noop).unwrap();
        let e = HashEmbedder::new(32);
        assert_eq!(embed_pending(&mut store, &e, &AtomicBool::new(false), &noop).unwrap(), VectorState::Ready);
        assert_eq!(store.stats().unwrap().pending_embeddings, 0);

        let mut other = HashEmbedder::new(32);
        other.key = "other-model".into();
        assert!(matches!(
            embed_pending(&mut store, &other, &AtomicBool::new(false), &noop).unwrap(),
            VectorState::ModelMismatch { .. }
        ));
        store.reset_vectors(&other.model_key()).unwrap();
        assert!(store.stats().unwrap().pending_embeddings > 0);
        assert_eq!(embed_pending(&mut store, &other, &AtomicBool::new(false), &noop).unwrap(), VectorState::Ready);
    }
}
