//! 作業フォルダ相対パスの検証。
//!
//! フロントエンドとのやり取りは常に `/` 区切りの相対パス。
//! 絶対パス・`..`・ドライブ指定は拒否し、シンボリックリンク経由で
//! 作業フォルダ外に出るケースも canonicalize で弾く。

use std::path::{Component, Path, PathBuf};

use crate::{CoreError, Result};

/// アプリ管理用ディレクトリ(インデックス・カテゴリ定義など)
pub const META_DIR: &str = ".memo";

/// フォルダを正規化する。Windows の canonicalize は `\\?\C:\...` という拡張パス形式を返すので、
/// 通常の `C:\...` 形式で表せる場合はそちらに戻す(表示・シェルの開始フォルダ・他アプリへの受け渡し用)。
pub fn canonical_dir(path: &Path) -> std::io::Result<PathBuf> {
    let p = path.canonicalize()?;
    Ok(simplify_verbatim(&p.to_string_lossy()).map(PathBuf::from).unwrap_or(p))
}

/// `\\?\C:\foo` → `C:\foo`、`\\?\UNC\server\share` → `\\server\share`。
/// 260 文字を超えるなど、通常形式に戻すと意味が変わりうる場合は None
pub fn simplify_verbatim(s: &str) -> Option<String> {
    const MAX_PATH: usize = 260;
    let out = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        let rest = s.strip_prefix(r"\\?\")?;
        let b = rest.as_bytes();
        let is_drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
        is_drive.then(|| rest.to_string())?
    };
    // 末尾がドット / 空白の要素や予約名は通常形式で別物になるので戻さない
    let risky = out.split('\\').any(|c| {
        c.ends_with('.') && c != "." && c != ".." || c.ends_with(' ') || {
            let stem = c.split('.').next().unwrap_or("").to_ascii_uppercase();
            matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit())
        }
    });
    (out.len() < MAX_PATH && !risky).then_some(out)
}

/// 相対パスを検証して絶対パスに変換する(存在しなくてもよい)。
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.replace('\\', "/");
    let p = Path::new(&rel);
    if rel.is_empty() {
        return Ok(root.to_path_buf());
    }
    for comp in p.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(CoreError::InvalidPath(rel.clone())),
        }
    }
    let joined = root.join(p);
    // 既存部分がシンボリックリンクで外に出ていないか確認
    let mut probe = joined.as_path();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent,
            None => break,
        }
    }
    if probe.exists() {
        let canon_root = root.canonicalize()?;
        let canon = probe.canonicalize()?;
        if !canon.starts_with(&canon_root) {
            return Err(CoreError::InvalidPath(rel));
        }
    }
    Ok(joined)
}

/// 書き込み系操作用。`.memo/` 配下はアプリ専用なので拒否する。
pub fn resolve_writable(root: &Path, rel: &str) -> Result<PathBuf> {
    if is_meta(rel) || rel.trim_matches('/').is_empty() {
        return Err(CoreError::InvalidPath(rel.to_string()));
    }
    resolve(root, rel)
}

pub fn is_meta(rel: &str) -> bool {
    rel.replace('\\', "/")
        .trim_start_matches("./")
        .split('/')
        .next()
        .map(|first| first == META_DIR)
        .unwrap_or(false)
}

/// 絶対パス → `/` 区切りの相対パス
pub fn to_rel(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 既存ファイルと衝突しないパスを返す(`name.md` → `name-2.md` …)。
pub fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (2..)
        .map(|n| parent.join(format!("{stem}-{n}{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded iterator")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve(dir.path(), "../x").is_err());
        assert!(resolve(dir.path(), "a/../../x").is_err());
        assert!(resolve(dir.path(), "/etc/passwd").is_err());
        assert!(resolve(dir.path(), "a/b.md").is_ok());
        assert!(resolve(dir.path(), "日々/2026/10/2026-10-26.md").is_ok());
    }

    #[test]
    fn meta_is_not_writable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_writable(dir.path(), ".memo/categories.yaml").is_err());
        assert!(resolve_writable(dir.path(), "").is_err());
        assert!(resolve_writable(dir.path(), "技術/a.md").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        assert!(resolve(root.path(), "link/secret.md").is_err());
    }

    #[test]
    fn verbatim_paths_are_simplified() {
        assert_eq!(simplify_verbatim(r"\\?\C:\ws\claude").as_deref(), Some(r"C:\ws\claude"));
        assert_eq!(simplify_verbatim(r"\\?\UNC\nas\share\memo").as_deref(), Some(r"\\nas\share\memo"));
        assert_eq!(simplify_verbatim(r"C:\ws"), None);
        assert_eq!(simplify_verbatim("/home/taka"), None);
        assert_eq!(simplify_verbatim(r"\\?\Volume{abc}\x"), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\ws\con.txt"), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\ws\trailing. "), None);
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(simplify_verbatim(&long), None);
    }

    #[test]
    fn unique_path_appends_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "").unwrap();
        assert_eq!(unique_path(p), dir.path().join("a-2.md"));
    }
}
