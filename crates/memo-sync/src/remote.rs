//! リモートの種類判定と、接続前の確認(空 / データあり / 作成する)。
//!
//! 受け付ける形式:
//! - GitHub / GitLab / 任意の Git サーバー: `https://…` / `git@host:owner/repo.git` / `ssh://…`
//! - ファイルサーバ: `\\server\share\memo.git` / `Z:\memo.git` / `/mnt/nas/memo.git` / `file://…`
//!   (存在しない・空のフォルダなら bare リポジトリを作る)

use std::path::PathBuf;

use serde::Serialize;

use crate::{
    error::ErrorKind,
    git::{Git, LOCAL_TIMEOUT, NETWORK_TIMEOUT},
    Result, SyncError,
};

pub const DEFAULT_BRANCH: &str = "main";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKind {
    Github,
    Gitlab,
    File,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProbeState {
    /// リポジトリはあるが空(このフォルダの内容を送る)
    Empty,
    /// 既にデータがある(取り込んで合わせる)
    HasData { branch: String },
    /// ファイルサーバ上にまだない(作ってから送る)
    WillCreate,
}

#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    /// 正規化した URL / パス(この値を remote に登録する)
    pub url: String,
    pub kind: RemoteKind,
    #[serde(flatten)]
    pub state: ProbeState,
}

pub fn kind_of(url: &str) -> RemoteKind {
    let l = url.to_ascii_lowercase();
    if is_local_path(url) {
        RemoteKind::File
    } else if l.contains("github.com") || l.contains("github") {
        RemoteKind::Github
    } else if l.contains("gitlab") {
        RemoteKind::Gitlab
    } else {
        RemoteKind::Other
    }
}

/// ファイルサーバ / ローカルのパスか(URL や scp 形式 `user@host:path` ではない)
pub fn is_local_path(url: &str) -> bool {
    let u = url.trim();
    let b = u.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    drive
        || u.starts_with("file://")
        || u.starts_with("\\\\")
        || u.starts_with("//")
        || u.starts_with('/')
        || u.starts_with("~/")
        || u.starts_with("./")
        || u.starts_with("../")
}

/// 入力の揺れをそろえる(前後の空白・引用符、`~`、Windows の `\`)
pub fn normalize(url: &str) -> String {
    let u = url.trim().trim_matches('"').trim_matches('\'').trim();
    if !is_local_path(u) {
        return u.to_string();
    }
    let mut s = u.to_string();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            s = PathBuf::from(home).join(rest).to_string_lossy().into_owned();
        }
    }
    // Git for Windows は `/` 区切りなら UNC(//server/share)もドライブ(Z:/x)も確実に扱える
    if cfg!(windows) {
        s = s.replace('\\', "/");
    }
    while s.len() > 1 && s.ends_with('/') && !s.ends_with(":/") {
        s.pop();
    }
    s
}

/// ファイルリモートのディスク上のパス
pub fn local_path(url: &str) -> Option<PathBuf> {
    if !is_local_path(url) {
        return None;
    }
    let s = url.strip_prefix("file://").unwrap_or(url);
    // file:///C:/x → C:/x
    let s = match s.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => &s[1..],
        _ => s,
    };
    Some(PathBuf::from(s))
}

/// リモートに対して git を呼ぶ準備。ファイルサーバのリポジトリは所有者が別ユーザーになりやすく、
/// git 2.35.2 以降は "dubious ownership" で拒否するため、そのリモートに限って許可する
pub(crate) fn git_for(url: &str) -> Git {
    let g = Git::bare();
    if is_local_path(url) {
        g.config("safe.directory=*")
    } else {
        g
    }
}

pub(crate) struct RemoteRefs {
    pub head: Option<String>,
    pub branches: Vec<String>,
}

impl RemoteRefs {
    /// 取り込むブランチ: リモートの HEAD → main → master → 最初のもの
    pub fn default_branch(&self) -> Option<String> {
        let has = |b: &str| self.branches.iter().any(|x| x == b);
        self.head
            .clone()
            .filter(|h| has(h))
            .or_else(|| ["main", "master"].iter().find(|b| has(b)).map(|b| b.to_string()))
            .or_else(|| self.branches.first().cloned())
    }
}

pub(crate) fn ls_remote(url: &str) -> Result<RemoteRefs> {
    let out = git_for(url).run(["ls-remote", "--symref", url], NETWORK_TIMEOUT, "リモートの確認")?;
    let mut refs = RemoteRefs { head: None, branches: Vec::new() };
    for line in out.text().lines() {
        if let Some(rest) = line.strip_prefix("ref: ") {
            if let Some((target, name)) = rest.split_once('\t') {
                if name == "HEAD" {
                    refs.head = target.strip_prefix("refs/heads/").map(str::to_string);
                }
            }
        } else if let Some((_, name)) = line.split_once('\t') {
            if let Some(b) = name.strip_prefix("refs/heads/") {
                refs.branches.push(b.to_string());
            }
        }
    }
    Ok(refs)
}

/// 接続前の確認。ローカルのフォルダは一切変更しない
pub fn probe(url: &str) -> Result<Probe> {
    let url = normalize(url);
    if url.is_empty() {
        return Err(SyncError::state("リモートの URL またはパスを入力してください"));
    }
    let kind = kind_of(&url);
    if let Some(p) = local_path(&url) {
        if let Some(state) = local_state(&p)? {
            return Ok(Probe { url, kind, state });
        }
    }
    let refs = ls_remote(&url)?;
    let state = match refs.default_branch() {
        Some(branch) => ProbeState::HasData { branch },
        None => ProbeState::Empty,
    };
    Ok(Probe { url, kind, state })
}

/// ファイルリモートの事前チェック。Some なら ls-remote は不要(まだ作られていない)
fn local_state(p: &std::path::Path) -> Result<Option<ProbeState>> {
    if !p.exists() {
        return match p.parent() {
            Some(parent) if parent.as_os_str().is_empty() || parent.is_dir() => Ok(Some(ProbeState::WillCreate)),
            _ => Err(SyncError::new(
                ErrorKind::NotFound,
                format!("フォルダ {} が見つかりません(ファイルサーバに接続できているか確認してください)", p.parent().unwrap_or(p).display()),
            )),
        };
    }
    if !p.is_dir() {
        return Err(SyncError::state("ファイルではなくフォルダを指定してください"));
    }
    if std::fs::read_dir(p)?.next().is_none() {
        return Ok(Some(ProbeState::WillCreate));
    }
    if p.join("HEAD").is_file() && p.join("objects").is_dir() {
        return Ok(None);
    }
    if p.join(".git").exists() {
        return Err(SyncError::state(
            "作業フォルダ付きのリポジトリには送信できません。空のフォルダか、`git init --bare` で作ったリポジトリを指定してください",
        ));
    }
    Err(SyncError::state(
        "Git リポジトリではないフォルダです。空のフォルダか、まだ存在しないフォルダ名(例: memo.git)を指定してください",
    ))
}

/// ファイルサーバ上に bare リポジトリを作る(既にあれば何もしない)
pub fn ensure_created(url: &str, branch: &str) -> Result<()> {
    let Some(p) = local_path(url) else { return Ok(()) };
    if local_state(&p)? != Some(ProbeState::WillCreate) {
        return Ok(());
    }
    std::fs::create_dir_all(&p)?;
    let g = git_for(url);
    g.run([std::ffi::OsStr::new("init"), "--bare".as_ref(), p.as_os_str()], LOCAL_TIMEOUT, "リポジトリの作成")?;
    let gp = Git::at(&p).config("safe.directory=*");
    gp.local(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")], "既定ブランチの設定")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kinds() {
        assert_eq!(kind_of("https://github.com/me/memo.git"), RemoteKind::Github);
        assert_eq!(kind_of("git@github.com:me/memo.git"), RemoteKind::Github);
        assert_eq!(kind_of("https://gitlab.example.co.jp/me/memo.git"), RemoteKind::Gitlab);
        assert_eq!(kind_of(r"\\nas\share\memo.git"), RemoteKind::File);
        assert_eq!(kind_of("Z:\\memo.git"), RemoteKind::File);
        assert_eq!(kind_of("/mnt/nas/memo.git"), RemoteKind::File);
        assert_eq!(kind_of("ssh://git@example.com/memo.git"), RemoteKind::Other);
    }

    #[test]
    fn scp_style_is_not_local() {
        assert!(!is_local_path("git@github.com:me/memo.git"));
        assert!(!is_local_path("host:memo.git"));
        assert!(is_local_path("C:/memo.git"));
    }

    #[test]
    fn normalizes_input() {
        assert_eq!(normalize("  \"https://github.com/me/memo.git\" "), "https://github.com/me/memo.git");
        if cfg!(windows) {
            assert_eq!(normalize(r"\\nas\share\memo.git\"), "//nas/share/memo.git");
            assert_eq!(normalize(r"Z:\memo.git"), "Z:/memo.git");
        }
        assert_eq!(local_path("file:///C:/x").unwrap(), PathBuf::from("C:/x"));
    }

    #[test]
    fn default_branch_prefers_head() {
        let r = RemoteRefs { head: Some("trunk".into()), branches: vec!["main".into(), "trunk".into()] };
        assert_eq!(r.default_branch().as_deref(), Some("trunk"));
        let r = RemoteRefs { head: None, branches: vec!["x".into(), "master".into()] };
        assert_eq!(r.default_branch().as_deref(), Some("master"));
        let r = RemoteRefs { head: None, branches: vec![] };
        assert_eq!(r.default_branch(), None);
    }
}
