//! 作業フォルダの監視。変更のあったパス(作業フォルダ相対)をまとめて通知する。
//!
//! 2 秒のデバウンスで、保存のたびに連続して来るイベントを 1 回にまとめる。
//! `.memo/`(インデックス DB 自身)や隠しファイル、Office のロックファイルは無視する。

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

use crate::{IndexError, Result};

pub struct Watcher {
    _inner: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

fn ignored(rel: &Path, exclude_dirs: &[String]) -> bool {
    rel.components().any(|c| match c {
        Component::Normal(s) => {
            let s = s.to_string_lossy();
            s.starts_with('.') || s.starts_with("~$") || exclude_dirs.iter().any(|d| d == s.as_ref())
        }
        _ => false,
    })
}

pub fn watch(
    root: &Path,
    exclude_dirs: Vec<String>,
    on_change: impl Fn(Vec<String>) + Send + 'static,
) -> Result<Watcher> {
    let root_buf: PathBuf = memo_core::paths::canonical_dir(root)?;
    let base = root_buf.clone();
    let mut debouncer = new_debouncer(Duration::from_secs(2), None, move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let mut paths = BTreeSet::new();
        for ev in events {
            for p in &ev.paths {
                let Ok(rel) = p.strip_prefix(&base) else { continue };
                if rel.as_os_str().is_empty() || ignored(rel, &exclude_dirs) {
                    continue;
                }
                paths.insert(memo_core::paths::to_rel(&base, p));
            }
        }
        if !paths.is_empty() {
            on_change(paths.into_iter().collect());
        }
    })
    .map_err(|e| IndexError::Extract(format!("フォルダ監視を開始できません: {e}")))?;
    debouncer
        .watch(&root_buf, RecursiveMode::Recursive)
        .map_err(|e| IndexError::Extract(format!("フォルダ監視を開始できません: {e}")))?;
    Ok(Watcher { _inner: debouncer })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn ignores_meta_and_lock_files() {
        let ex = vec!["node_modules".to_string()];
        assert!(ignored(Path::new(".memo/index.sqlite"), &ex));
        assert!(ignored(Path::new("調査/~$見積.xlsx"), &ex));
        assert!(ignored(Path::new("a/node_modules/x.md"), &ex));
        assert!(!ignored(Path::new("技術/a.md"), &ex));
    }

    #[test]
    fn reports_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".memo")).unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch(dir.path(), vec![], move |p| tx.send(p).unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join(".memo/ignored.md"), "x").unwrap();
        std::fs::write(dir.path().join("note.md"), "hello").unwrap();
        let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(got, ["note.md"]);
    }
}
