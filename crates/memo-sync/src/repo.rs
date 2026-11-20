//! 作業フォルダ(= リポジトリのルート)の状態取得・初回接続・同期。
//!
//! 同期の流れ: 変更をコミット → 取得(fetch) → 取り込み(merge) → 送信(push)。
//! 取り込みで競合したら作業ツリーには「この端末の版」を残し、未解消のまま止める
//! (競合マーカー入りのファイルをエディタや検索インデックスに見せない)。
//! 同期の有効/無効・最終同期時刻は端末ごとの情報なので `.git/config` に持つ。

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    conflict::{self, ConflictInfo},
    error::ErrorKind,
    git::{self, Git, LOCAL_TIMEOUT},
    remote::{self, ProbeState, RemoteKind, DEFAULT_BRANCH},
    Result, SyncError,
};

const CFG_ENABLED: &str = "memo-editor.sync";
const CFG_REMOTE: &str = "memo-editor.remote";
const CFG_LAST: &str = "memo-editor.lastsync";
pub(crate) const DEVICE_TRAILER: &str = "Device";

/// 転送(fetch / push)は大きなファイルがあると時間がかかるので長めに待つ
const TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// 同期の対象にしないもの。作業フォルダの `.gitignore` に追記する
const IGNORE_LINES: &[&str] = &[
    ".memo/*",
    "!.memo/categories.yaml",
    ".*.tmp-*",
    "~$*",
    ".~lock.*#",
    "Thumbs.db",
    "desktop.ini",
    ".DS_Store",
];

/// 接続時にリモート側を優先する設定ファイル(この端末の既定値で上書きしない)
const PREFER_REMOTE: &[&str] = &[".memo/categories.yaml", ".gitignore", ".gitattributes"];

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RepoState {
    /// git コマンドがない
    GitMissing,
    /// まだ同期を設定していない
    #[default]
    NotRepo,
    /// 上位フォルダが Git 管理下(接続すると作業フォルダを独立したリポジトリにする)
    Nested { top: String },
    /// Git リポジトリだが、このアプリの同期は有効になっていない
    Disabled { remote_url: Option<String> },
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStatus {
    #[serde(flatten)]
    pub repo: RepoState,
    pub git_version: Option<String>,
    pub remote_url: Option<String>,
    pub remote_kind: Option<RemoteKind>,
    pub branch: Option<String>,
    /// まだコミットしていない変更
    pub pending: Vec<Change>,
    /// コミット済みで未送信の数
    pub ahead: u32,
    /// 取り込みの途中(競合の解消待ち)
    pub merging: bool,
    pub conflicts: Vec<String>,
    /// 最後に同期できた時刻(UNIX 秒)
    pub last_sync: Option<i64>,
    /// 状態の取得自体に失敗したとき
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SyncOptions {
    pub device: String,
    /// 手動同期で付けたメモ(コミットメッセージ)
    #[serde(default)]
    pub message: Option<String>,
}

/// 利用者に確認せずに解決したもの(バイナリの競合など)
#[derive(Debug, Clone, Serialize)]
pub struct AutoResolved {
    pub path: String,
    /// 別名で残したファイル
    pub copy: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    pub committed: bool,
    pub pulled: bool,
    pub pushed: bool,
    /// 他の端末の変更で書き換わったファイル(開いているタブの再読み込み用)
    pub changed: Vec<Change>,
    /// 利用者の判断が必要な競合
    pub conflicts: Vec<ConflictInfo>,
    pub auto_resolved: Vec<AutoResolved>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    pub id: String,
    pub author: String,
    pub device: String,
    pub time: i64,
    pub subject: String,
}

// ---- 共通 ---------------------------------------------------------------------

/// 作業フォルダに対する git。所有者の違うネットワークドライブでも動くよう、このフォルダに限り safe.directory を許可する
pub(crate) fn repo_git(root: &Path) -> Git {
    let p = root.to_string_lossy().replace('\\', "/");
    Git::at(root)
        .config(format!("safe.directory={p}"))
        .config("core.autocrlf=false")
        .config("core.safecrlf=false")
}

/// fetch / push 用。ファイルサーバのリモートなら safe.directory を許可し、SSH は対話なし・初回のホスト鍵は受け入れる
fn net_git(g: &Git, url: &str) -> Git {
    let mut g = g.clone();
    if remote::is_local_path(url) {
        g = g.config("safe.directory=*");
    }
    let ssh_like = url.starts_with("ssh://") || (!url.contains("://") && url.contains('@') && url.contains(':'));
    let user_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || g.raw(["config", "--get", "core.sshCommand"], LOCAL_TIMEOUT).map(|o| o.ok()).unwrap_or(false);
    if ssh_like && !user_ssh {
        g = g.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20");
    }
    g
}

pub(crate) fn git_dir(root: &Path) -> PathBuf {
    let d = root.join(".git");
    if d.is_dir() {
        return d;
    }
    // worktree 等で .git がファイルの場合
    repo_git(root)
        .raw(["rev-parse", "--absolute-git-dir"], LOCAL_TIMEOUT)
        .ok()
        .filter(|o| o.ok())
        .map(|o| PathBuf::from(o.text()))
        .unwrap_or(d)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn local_config(g: &Git) -> HashMap<String, String> {
    let Ok(out) = g.raw(["config", "--local", "-z", "--list"], LOCAL_TIMEOUT) else {
        return HashMap::new();
    };
    out.nul_split()
        .into_iter()
        .filter_map(|e| e.split_once('\n').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn remote_name(cfg: &HashMap<String, String>) -> String {
    cfg.get(CFG_REMOTE).cloned().unwrap_or_else(|| "origin".into())
}

fn has_head(g: &Git) -> bool {
    g.check(["rev-parse", "-q", "--verify", "HEAD"]).unwrap_or(false)
}

fn rev(g: &Git, r: &str) -> Option<String> {
    g.raw(["rev-parse", "-q", "--verify", r], LOCAL_TIMEOUT).ok().filter(|o| o.ok()).map(|o| o.text())
}

fn count(g: &Git, range: &str) -> u32 {
    g.raw(["rev-list", "--count", range], LOCAL_TIMEOUT)
        .ok()
        .filter(|o| o.ok())
        .and_then(|o| o.text().parse().ok())
        .unwrap_or(0)
}

fn change_kind(code: char) -> ChangeKind {
    match code {
        'A' | '?' => ChangeKind::Added,
        'D' => ChangeKind::Deleted,
        _ => ChangeKind::Modified,
    }
}

/// `--name-status -z` の出力(状態と パスが交互に並ぶ)
fn parse_name_status(out: &git::Output) -> Vec<Change> {
    let f = out.nul_split();
    f.chunks(2)
        .filter_map(|c| match c {
            [st, path] => Some(Change { path: path.clone(), kind: change_kind(st.chars().next().unwrap_or('M')) }),
            _ => None,
        })
        .collect()
}

/// 端末の識別子をコミットに残す(履歴表示・競合相手の表示用)。
/// git の user.name / user.email が未設定でもコミットできるよう、端末名から補う
fn with_identity(g: &Git, device: &str) -> Git {
    let has = |k: &str| {
        g.raw(["config", "--get", k], LOCAL_TIMEOUT)
            .map(|o| o.ok() && !o.text().trim().is_empty())
            .unwrap_or(false)
    };
    let mut g = g.clone();
    if !has("user.name") {
        g = g.config(format!("user.name={device}"));
    }
    if !has("user.email") {
        let slug: String = device
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect();
        let slug = slug.trim_matches('-');
        g = g.config(format!("user.email={}@memo-editor.local", if slug.is_empty() { "device" } else { slug }));
    }
    g
}

fn trailer(device: &str) -> String {
    format!("{DEVICE_TRAILER}: {device}")
}

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

// ---- 状態 ---------------------------------------------------------------------

pub fn status(root: &Path) -> SyncStatus {
    let mut st = SyncStatus { git_version: git::version(), ..SyncStatus::default() };
    if st.git_version.is_none() {
        st.repo = RepoState::GitMissing;
        return st;
    }
    let g = repo_git(root);
    match g.raw(["rev-parse", "--show-toplevel"], LOCAL_TIMEOUT) {
        Ok(o) if o.ok() => {
            let top = o.text();
            if !same_path(Path::new(&top), root) {
                st.repo = RepoState::Nested { top };
                return st;
            }
        }
        Ok(o) if o.stderr.contains("dubious ownership") => {
            st.repo = RepoState::NotRepo;
            st.problem = Some("フォルダの所有者が異なるため Git に拒否されました".into());
            return st;
        }
        _ => {
            st.repo = RepoState::NotRepo;
            return st;
        }
    }
    let cfg = local_config(&g);
    let remote = remote_name(&cfg);
    st.remote_url = cfg.get(&format!("remote.{remote}.url")).cloned();
    st.remote_kind = st.remote_url.as_deref().map(remote::kind_of);
    st.last_sync = cfg.get(CFG_LAST).and_then(|v| v.parse().ok());
    if cfg.get(CFG_ENABLED).map(String::as_str) != Some("true") {
        st.repo = RepoState::Disabled { remote_url: st.remote_url.clone() };
        return st;
    }
    st.repo = RepoState::Ready;
    st.merging = git_dir(root).join("MERGE_HEAD").exists();
    if let Err(e) = fill_worktree(&g, &remote, &mut st) {
        st.problem = Some(e.message);
    }
    st
}

fn fill_worktree(g: &Git, remote: &str, st: &mut SyncStatus) -> Result<()> {
    let out = g.clone().env("GIT_OPTIONAL_LOCKS", "0").local(
        ["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all", "--no-renames"],
        "状態の取得",
    )?;
    let mut upstream = false;
    let mut initial = false;
    for rec in out.nul_split() {
        if let Some(h) = rec.strip_prefix("# branch.head ") {
            st.branch = (h != "(detached)").then(|| h.to_string());
        } else if let Some(o) = rec.strip_prefix("# branch.oid ") {
            initial = o == "(initial)";
        } else if rec.starts_with("# branch.upstream ") {
            upstream = true;
        } else if let Some(ab) = rec.strip_prefix("# branch.ab ") {
            st.ahead = ab.split(' ').next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
        } else if let Some(rest) = rec.strip_prefix("1 ") {
            let f: Vec<&str> = rest.splitn(8, ' ').collect();
            if let (Some(xy), Some(path)) = (f.first(), f.get(7)) {
                let code = xy.chars().find(|c| *c != '.').unwrap_or('M');
                st.pending.push(Change { path: path.to_string(), kind: change_kind(code) });
            }
        } else if let Some(rest) = rec.strip_prefix("u ") {
            if let Some(path) = rest.splitn(10, ' ').nth(9) {
                st.conflicts.push(path.to_string());
            }
        } else if let Some(path) = rec.strip_prefix("? ") {
            st.pending.push(Change { path: path.to_string(), kind: ChangeKind::Added });
        }
    }
    if !upstream && !initial {
        let b = st.branch.clone().unwrap_or_default();
        let tracking = format!("refs/remotes/{remote}/{b}");
        st.ahead = if rev(g, &tracking).is_some() { count(g, &format!("{tracking}..HEAD")) } else { count(g, "HEAD") };
    }
    Ok(())
}

pub fn history(root: &Path, n: usize) -> Vec<CommitInfo> {
    let g = repo_git(root);
    if !has_head(&g) {
        return Vec::new();
    }
    let fmt = format!("--format=%h%x1f%an%x1f%at%x1f%s%x1f%(trailers:key={DEVICE_TRAILER},valueonly,separator=%x2C)%x1e");
    let Ok(out) = g.raw(["log", "-n", &n.to_string(), "--no-merges", &fmt], LOCAL_TIMEOUT) else {
        return Vec::new();
    };
    if !out.ok() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_matches(['\n', '\r']).split('\x1f').collect();
            (f.len() >= 4).then(|| CommitInfo {
                id: f[0].to_string(),
                author: f[1].to_string(),
                time: f[2].parse().unwrap_or(0),
                subject: f[3].to_string(),
                device: f.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
            })
        })
        .collect()
}

// ---- 接続 ---------------------------------------------------------------------

/// 同期を始める。リモートが空ならこのフォルダの内容を送り、データがあれば取り込んで合わせる。
/// リモートに接続できるかを先に確かめ、だめなら作業フォルダには何もしない
pub fn connect(root: &Path, url: &str, device: &str) -> Result<SyncReport> {
    let probe = remote::probe(url)?;
    let url = probe.url.clone();
    let remote_branch = match &probe.state {
        ProbeState::HasData { branch } => Some(branch.clone()),
        _ => None,
    };
    if probe.state == ProbeState::WillCreate {
        remote::ensure_created(&url, DEFAULT_BRANCH)?;
    }

    let st = status(root);
    let g = repo_git(root);
    match &st.repo {
        RepoState::GitMissing => return Err(SyncError::new(ErrorKind::GitMissing, "Git が見つかりません。Git をインストールしてください")),
        // 上位フォルダ(ホームの dotfiles 管理など)が Git 管理下でも、作業フォルダを独立したリポジトリにする。
        // 上位のリポジトリからは入れ子のリポジトリとして無視される
        RepoState::NotRepo | RepoState::Nested { .. } => {
            g.local(["init", "-q"], "リポジトリの作成")?;
            let b = remote_branch.as_deref().unwrap_or(DEFAULT_BRANCH);
            g.local(["symbolic-ref", "HEAD", &format!("refs/heads/{b}")], "ブランチの設定")?;
        }
        _ => {}
    }

    let cfg = local_config(&g);
    let remote = remote_name(&cfg);
    if cfg.contains_key(&format!("remote.{remote}.url")) {
        g.local(["remote", "set-url", &remote, &url], "リモートの設定")?;
    } else {
        g.local(["remote", "add", &remote, &url], "リモートの設定")?;
    }
    g.local(["config", "--local", "core.autocrlf", "false"], "設定")?;
    g.local(["config", "--local", "core.quotepath", "false"], "設定")?;
    g.local(["config", "--local", CFG_REMOTE, &remote], "設定")?;
    g.local(["config", "--local", CFG_ENABLED, "true"], "設定")?;

    let mut report = SyncReport::default();
    if let Some(branch) = &remote_branch {
        if !has_head(&g) {
            let cur = g.raw(["symbolic-ref", "--short", "-q", "HEAD"], LOCAL_TIMEOUT).map(|o| o.text()).unwrap_or_default();
            if &cur != branch {
                g.local(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")], "ブランチの設定")?;
            }
            fetch(&g, &remote, &url)?;
            adopt_remote(root, &g, &remote, branch, device, &mut report)?;
        }
    }
    ensure_ignore_files(root)?;
    let more = sync_inner(root, &SyncOptions { device: device.into(), message: None }, true)?;
    merge_report(&mut report, more);
    Ok(report)
}

/// このアプリでの同期をやめる(リポジトリと履歴はそのまま残す)
pub fn disable(root: &Path) -> Result<()> {
    let g = repo_git(root);
    let _ = g.raw(["config", "--local", "--unset", CFG_ENABLED], LOCAL_TIMEOUT)?;
    Ok(())
}

/// まだコミットのないこの端末に、リモートの内容を取り込む。
/// 同じパスで内容が違うファイルは、この端末の版を別名で残してからリモートの版にする
fn adopt_remote(root: &Path, g: &Git, remote: &str, branch: &str, device: &str, report: &mut SyncReport) -> Result<()> {
    let tracking = format!("refs/remotes/{remote}/{branch}");
    g.local(["update-ref", &format!("refs/heads/{branch}"), &tracking], "リモートの取り込み")?;
    // インデックスだけリモートの内容にする(作業ツリーはこの端末のまま)
    g.local(["read-tree", "HEAD"], "リモートの取り込み")?;
    let _ = g.raw(["update-index", "-q", "--refresh"], LOCAL_TIMEOUT)?;
    let missing = g.local(["ls-files", "-d", "-z"], "差分の確認")?.nul_split();
    let differs: Vec<String> = g
        .local(["ls-files", "-m", "-z"], "差分の確認")?
        .nul_split()
        .into_iter()
        .filter(|p| !missing.contains(p))
        .collect();

    let mut restore = missing.clone();
    for p in &differs {
        if PREFER_REMOTE.contains(&p.as_str()) {
            if p == ".memo/categories.yaml" {
                report.notes.push("カテゴリ設定は同期先のものを使います".into());
            }
        } else {
            let (abs_copy, rel_copy) = conflict::copy_path(root, p, device);
            fs::copy(root.join(p), &abs_copy)?;
            report.auto_resolved.push(AutoResolved {
                path: p.clone(),
                copy: Some(rel_copy),
                note: "同じ名前で内容の違うファイルがあったため、この端末の版を別名で残しました".into(),
            });
        }
        restore.push(p.clone());
    }
    g.local_paths(&["checkout"], &restore, "リモートのファイルの展開")?;
    report.changed.extend(missing.into_iter().map(|path| Change { path, kind: ChangeKind::Added }));
    report.changed.extend(differs.into_iter().map(|path| Change { path, kind: ChangeKind::Modified }));
    report.pulled = true;
    Ok(())
}

/// `.gitignore`(インデックス・一時ファイル・Office のロックファイルを除外)と
/// `.gitattributes`(改行コードを変換しない)を用意する
pub fn ensure_ignore_files(root: &Path) -> Result<()> {
    let gi = root.join(".gitignore");
    let cur = fs::read_to_string(&gi).unwrap_or_default();
    let have: Vec<&str> = cur.lines().map(str::trim).collect();
    let missing: Vec<&str> = IGNORE_LINES.iter().copied().filter(|l| !have.contains(l)).collect();
    if !missing.is_empty() {
        let nl = if cur.contains("\r\n") { "\r\n" } else { "\n" };
        let mut body = cur.clone();
        if !body.is_empty() && !body.ends_with('\n') {
            body.push_str(nl);
        }
        if !body.is_empty() {
            body.push_str(nl);
        }
        body.push_str(&format!("# memo-editor: 端末ごとに作り直すもの・一時ファイル{nl}"));
        for l in missing {
            body.push_str(l);
            body.push_str(nl);
        }
        memo_core::text::write_atomic(&gi, body.as_bytes())?;
    }
    let ga = root.join(".gitattributes");
    if !ga.exists() {
        memo_core::text::write_atomic(
            &ga,
            "# Windows / Linux の間でファイルをそのまま共有する(改行コードを変換しない)\n* -text\n".as_bytes(),
        )?;
    }
    Ok(())
}

// ---- 同期 ---------------------------------------------------------------------

pub fn sync(root: &Path, opts: &SyncOptions) -> Result<SyncReport> {
    sync_inner(root, opts, false)
}

fn merge_report(into: &mut SyncReport, more: SyncReport) {
    into.committed |= more.committed;
    into.pulled |= more.pulled;
    into.pushed |= more.pushed;
    for c in more.changed {
        if !into.changed.iter().any(|x| x.path == c.path) {
            into.changed.push(c);
        }
    }
    into.conflicts.extend(more.conflicts);
    into.auto_resolved.extend(more.auto_resolved);
    into.notes.extend(more.notes);
}

fn fetch(g: &Git, remote: &str, url: &str) -> Result<()> {
    net_git(g, url).run(["fetch", "--prune", "--no-tags", "-q", remote], TRANSFER_TIMEOUT, "リモートからの取得")?;
    Ok(())
}

fn sync_inner(root: &Path, opts: &SyncOptions, allow_unrelated: bool) -> Result<SyncReport> {
    let st = status(root);
    if let Some(p) = &st.problem {
        return Err(SyncError::state(p.clone()));
    }
    match &st.repo {
        RepoState::Ready => {}
        RepoState::GitMissing => return Err(SyncError::new(ErrorKind::GitMissing, "Git が見つかりません。Git をインストールしてください")),
        _ => return Err(SyncError::state("このフォルダは同期が設定されていません")),
    }
    let device = if opts.device.trim().is_empty() { crate::default_device_name() } else { opts.device.trim().to_string() };
    let g = with_identity(&repo_git(root), &device);
    let cfg = local_config(&g);
    let remote = remote_name(&cfg);
    let mut report = SyncReport::default();

    // 前回の取り込みが競合で止まっている
    if st.merging {
        if !st.conflicts.is_empty() {
            report.conflicts = conflict::list(root)?;
            return Ok(report);
        }
        g.local(["add", "-A"], "変更の記録")?;
        g.local(["commit", "-q", "--no-verify", "-m", "他の端末との競合を解消", "-m", &trailer(&device)], "コミット")?;
        report.committed = true;
    }

    if commit_pending(&g, &device, opts.message.as_deref())? {
        report.committed = true;
    }

    let Some(url) = st.remote_url.clone() else {
        return Ok(report);
    };
    let branch = st.branch.clone().unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    let tracking = format!("refs/remotes/{remote}/{branch}");

    for attempt in 0..3 {
        fetch(&g, &remote, &url)?;
        let remote_exists = rev(&g, &tracking).is_some();
        if remote_exists {
            if !has_head(&g) {
                adopt_remote(root, &g, &remote, &branch, &device, &mut report)?;
            } else if count(&g, &format!("HEAD..{tracking}")) > 0 {
                let before = rev(&g, "HEAD").unwrap_or_default();
                let merged = merge(&g, &tracking, &device, allow_unrelated)?;
                report.pulled = true;
                if !merged {
                    let (conflicts, auto) = conflict::prepare(root, &g)?;
                    report.auto_resolved.extend(auto);
                    if conflicts.is_empty() {
                        g.local(["commit", "-q", "--no-verify", "-m", "他の端末との競合を自動で解消", "-m", &trailer(&device)], "コミット")?;
                    } else {
                        // 競合以外のファイルは取り込み済み。作業ツリーとの差分で再読み込み対象を出す
                        let out = g.local(["diff", "--no-renames", "--name-status", "-z", &before], "差分の確認")?;
                        let unresolved: Vec<&str> = conflicts.iter().map(|c| c.path.as_str()).collect();
                        report.changed.extend(parse_name_status(&out).into_iter().filter(|c| !unresolved.contains(&c.path.as_str())));
                        report.conflicts = conflicts;
                        return Ok(report);
                    }
                }
                let out = g.local(["diff", "--no-renames", "--name-status", "-z", &before, "HEAD"], "差分の確認")?;
                for c in parse_name_status(&out) {
                    if !report.changed.iter().any(|x| x.path == c.path) {
                        report.changed.push(c);
                    }
                }
            }
        }

        let ahead = if remote_exists { count(&g, &format!("{tracking}..HEAD")) } else { count(&g, "HEAD") };
        if ahead == 0 {
            break;
        }
        match push(&g, &remote, &branch, &url) {
            Ok(()) => {
                report.pushed = true;
                break;
            }
            // 取得から送信までの間に他の端末が送信した: もう一度取り込んでから送る
            Err(e) if e.kind == ErrorKind::Rejected && attempt < 2 => continue,
            Err(e) => return Err(e),
        }
    }
    g.local(["config", "--local", CFG_LAST, &now_secs().to_string()], "設定")?;
    Ok(report)
}

/// 変更をすべてコミットする。変更がなければ false
fn commit_pending(g: &Git, device: &str, message: Option<&str>) -> Result<bool> {
    g.local(["add", "-A"], "変更の記録")?;
    let out = g.local(["diff", "--cached", "--no-renames", "--name-status", "-z"], "変更の確認")?;
    let staged = parse_name_status(&out);
    if staged.is_empty() {
        return Ok(false);
    }
    let subject = match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None => summary(&staged),
    };
    g.local(["commit", "-q", "--no-verify", "-m", &subject, "-m", &trailer(device)], "コミット")?;
    Ok(true)
}

/// コミットメッセージの自動生成(「日々/2026/11/2026-11-20.md ほか 2 件を更新」)
fn summary(changes: &[Change]) -> String {
    let verb = |k: ChangeKind| match k {
        ChangeKind::Added => "追加",
        ChangeKind::Modified => "更新",
        ChangeKind::Deleted => "削除",
    };
    let first = &changes[0];
    let all_same = changes.iter().all(|c| c.kind == first.kind);
    let v = if all_same { verb(first.kind) } else { "更新" };
    if changes.len() == 1 {
        format!("{} を{v}", first.path)
    } else {
        format!("{} ほか {} 件を{v}", first.path, changes.len() - 1)
    }
}

/// 取り込み。競合したら false(MERGE_HEAD が残る)
fn merge(g: &Git, tracking: &str, device: &str, allow_unrelated: bool) -> Result<bool> {
    let t = trailer(device);
    let mut args = vec!["merge", "--no-edit", "-q", "-X", "no-renames", "-m", "他の端末の変更を取り込み", "-m", &t];
    if allow_unrelated {
        args.push("--allow-unrelated-histories");
    }
    args.push(tracking);
    let out = g.raw(&args, LOCAL_TIMEOUT)?;
    if out.ok() {
        return Ok(true);
    }
    let unmerged = g.local(["ls-files", "-u", "-z"], "競合の確認")?;
    if !unmerged.stdout.is_empty() {
        return Ok(false);
    }
    // 競合以外の理由(編集中のファイルが上書きされる等)で失敗: 取り込み前に戻す
    let _ = g.raw(["merge", "--abort"], LOCAL_TIMEOUT);
    let mut e = SyncError::from_git(&out.stderr, "他の端末の変更を取り込めませんでした");
    if out.stderr.contains("would be overwritten") {
        e = SyncError::new(ErrorKind::Other, "編集中のファイルと重なったため取り込みを見送りました。もう一度同期してください")
            .with_detail(out.stderr.clone());
    }
    Err(e)
}

fn push(g: &Git, remote: &str, branch: &str, url: &str) -> Result<()> {
    net_git(g, url).run(["push", "-q", "-u", remote, branch], TRANSFER_TIMEOUT, "送信")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_wording() {
        let c = |p: &str, k| Change { path: p.into(), kind: k };
        assert_eq!(summary(&[c("a.md", ChangeKind::Modified)]), "a.md を更新");
        assert_eq!(summary(&[c("a.md", ChangeKind::Added), c("b.md", ChangeKind::Added)]), "a.md ほか 1 件を追加");
        assert_eq!(summary(&[c("a.md", ChangeKind::Added), c("b.md", ChangeKind::Deleted)]), "a.md ほか 1 件を更新");
    }

    #[test]
    fn ignore_files_are_appended_once() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "node_modules/\r\n").unwrap();
        ensure_ignore_files(dir.path()).unwrap();
        ensure_ignore_files(dir.path()).unwrap();
        let body = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(body.starts_with("node_modules/\r\n"));
        assert_eq!(body.matches(".memo/*").count(), 1);
        assert!(body.contains("!.memo/categories.yaml\r\n"));
        assert!(dir.path().join(".gitattributes").is_file());
    }
}
