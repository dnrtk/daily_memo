//! 競合の一覧・詳細・解消。
//!
//! 取り込みで競合したら、作業ツリーには「この端末の版」を置き、インデックスに未解消の情報
//! (共通の祖先 :1 / この端末 :2 / 相手 :3)を残す。画面はここから箇所ごとの差分を受け取り、
//! 選んだ結果を `resolve` で書き戻す。
//! テキスト以外(Office・PDF・画像)は箇所ごとに選べないので、両方のファイルを残して自動で解決する。

use std::{fs, path::{Path, PathBuf}};

use memo_core::text::{self, FileKind};
use serde::{Deserialize, Serialize};

use crate::{
    git::{Git, LOCAL_TIMEOUT},
    repo::{git_dir, repo_git, AutoResolved, DEVICE_TRAILER},
    Result, SyncError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// 両方の端末で編集
    BothModified,
    /// この端末で削除、相手が編集
    DeletedByUs,
    /// この端末で編集、相手が削除
    DeletedByThem,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInfo {
    pub path: String,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Segment {
    /// 両方で同じ(または片方だけの変更で自動的に統合できた)部分
    Same { text: String },
    /// 選ぶ必要がある部分
    Conflict { ours: String, theirs: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictDetail {
    pub path: String,
    pub kind: ConflictKind,
    /// この端末の名前
    pub ours_label: String,
    /// 相手の端末名(コミットの Device: から。なければ作者名)
    pub theirs_label: String,
    pub theirs_time: Option<i64>,
    /// BothModified のときの箇所ごとの差分
    pub segments: Vec<Segment>,
    /// 削除との競合で残っている側の本文(プレビュー用)
    pub remaining_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Resolution {
    /// 画面で組み立てた最終的な本文
    Content { text: String },
    /// この端末の状態にする(削除との競合では「この端末の通り残す / 削除のまま」)
    Ours,
    /// 相手の状態にする
    Theirs,
    /// 両方のファイルを残す(相手の版を別名で保存)
    Both,
}

pub(crate) struct Unmerged {
    pub path: String,
    /// [_, 祖先, この端末, 相手]
    pub stages: [bool; 4],
}

impl Unmerged {
    fn kind(&self) -> Option<ConflictKind> {
        match (self.stages[2], self.stages[3]) {
            (true, true) => Some(ConflictKind::BothModified),
            (false, true) => Some(ConflictKind::DeletedByUs),
            (true, false) => Some(ConflictKind::DeletedByThem),
            (false, false) => None,
        }
    }
}

/// `ls-files -u -z`: `<mode> <sha> <stage>\t<path>`
pub(crate) fn unmerged(g: &Git) -> Result<Vec<Unmerged>> {
    let out = g.local(["ls-files", "-u", "-z"], "競合の確認")?;
    let mut v: Vec<Unmerged> = Vec::new();
    for rec in out.nul_split() {
        let Some((meta, path)) = rec.split_once('\t') else { continue };
        let stage: usize = meta.rsplit(' ').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if stage == 0 || stage > 3 {
            continue;
        }
        match v.iter_mut().find(|u| u.path == path) {
            Some(u) => u.stages[stage] = true,
            None => {
                let mut stages = [false; 4];
                stages[stage] = true;
                v.push(Unmerged { path: path.to_string(), stages });
            }
        }
    }
    Ok(v)
}

fn blob(g: &Git, stage: u8, path: &str) -> Result<Vec<u8>> {
    Ok(g.local(["cat-file", "blob", &format!(":{stage}:{path}")], "競合ファイルの読み込み")?.stdout)
}

fn is_binary(b: &[u8]) -> bool {
    b[..b.len().min(8000)].contains(&0)
}

fn is_text(path: &str, sides: &[&[u8]]) -> bool {
    text::kind_of(Path::new(path)) == FileKind::Text && sides.iter().all(|b| !is_binary(b))
}

/// ファイル名に使えない文字を避けた端末名
fn safe_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c })
        .collect();
    let t = t.trim().trim_end_matches('.').to_string();
    if t.is_empty() { "不明".into() } else { t }
}

/// 別名で残すときのパス: `報告書 (競合 ノートPC 2026-11-20 0930).xlsx`
pub(crate) fn copy_path(root: &Path, rel: &str, device: &str) -> (PathBuf, String) {
    let p = Path::new(rel);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let stamp = chrono::Local::now().format("%Y-%m-%d %H%M");
    let base = format!("{stem} (競合 {} {stamp})", safe_name(device));
    let dir_rel = rel.rsplit_once('/').map(|(d, _)| d.to_string());
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{base}{ext}") } else { format!("{base} {n}{ext}") };
        let rel_copy = match &dir_rel {
            Some(d) => format!("{d}/{name}"),
            None => name,
        };
        let abs = root.join(&rel_copy);
        if !abs.exists() {
            return (abs, rel_copy);
        }
        n += 1;
    }
}

/// 相手(MERGE_HEAD)の端末名と時刻
fn theirs_info(g: &Git) -> (String, Option<i64>) {
    let fmt = format!("--format=%an%x1f%at%x1f%(trailers:key={DEVICE_TRAILER},valueonly,separator=%x2C)");
    match g.raw(["log", "-1", &fmt, "MERGE_HEAD"], LOCAL_TIMEOUT) {
        Ok(o) if o.ok() => {
            let t = o.text();
            let f: Vec<&str> = t.split('\x1f').collect();
            let device = f.get(2).map(|s| s.trim()).filter(|s| !s.is_empty());
            let name = device.or(f.first().copied()).unwrap_or("他の端末").to_string();
            (name, f.get(1).and_then(|s| s.trim().parse().ok()))
        }
        _ => ("他の端末".into(), None),
    }
}

/// 取り込み直後に呼ぶ。テキストの競合は作業ツリーをこの端末の版に戻して利用者の判断を待ち、
/// それ以外は両方残して解決する。戻り値は (利用者が判断する競合, 自動で解決したもの)
pub(crate) fn prepare(root: &Path, g: &Git) -> Result<(Vec<ConflictInfo>, Vec<AutoResolved>)> {
    let (theirs_name, _) = theirs_info(g);
    let mut ask = Vec::new();
    let mut auto = Vec::new();
    for u in unmerged(g)? {
        let path = u.path.clone();
        match u.kind() {
            Some(ConflictKind::BothModified) => {
                let ours = blob(g, 2, &path)?;
                let theirs = blob(g, 3, &path)?;
                g.local(["checkout", "--ours", "--", &path], "競合ファイルの復元")?;
                if is_text(&path, &[&ours, &theirs]) {
                    ask.push(ConflictInfo { path, kind: ConflictKind::BothModified });
                } else {
                    let (abs, rel) = copy_path(root, &path, &theirs_name);
                    fs::write(&abs, &theirs)?;
                    g.local(["add", "--", &path, &rel], "競合の解消")?;
                    auto.push(AutoResolved {
                        path,
                        copy: Some(rel),
                        note: "両方の端末で変更されていたため、相手の版を別名で残しました".into(),
                    });
                }
            }
            Some(ConflictKind::DeletedByThem) => {
                let ours = blob(g, 2, &path)?;
                if is_text(&path, &[&ours]) {
                    ask.push(ConflictInfo { path, kind: ConflictKind::DeletedByThem });
                } else {
                    g.local(["add", "--", &path], "競合の解消")?;
                    auto.push(AutoResolved {
                        path,
                        copy: None,
                        note: format!("{theirs_name} では削除されていましたが、この端末で変更していたため残しました"),
                    });
                }
            }
            Some(ConflictKind::DeletedByUs) => {
                let theirs = blob(g, 3, &path)?;
                if is_text(&path, &[&theirs]) {
                    // 判断がつくまでは、この端末の状態(削除済み)にしておく
                    let _ = fs::remove_file(root.join(&path));
                    ask.push(ConflictInfo { path, kind: ConflictKind::DeletedByUs });
                } else {
                    g.local(["checkout", "--theirs", "--", &path], "競合ファイルの復元")?;
                    g.local(["add", "--", &path], "競合の解消")?;
                    auto.push(AutoResolved {
                        path,
                        copy: None,
                        note: format!("この端末では削除していましたが、{theirs_name} で変更されていたため戻しました"),
                    });
                }
            }
            None => {
                g.local(["rm", "-q", "--cached", "--", &path], "競合の解消")?;
            }
        }
    }
    Ok((ask, auto))
}

pub fn list(root: &Path) -> Result<Vec<ConflictInfo>> {
    let g = repo_git(root);
    Ok(unmerged(&g)?
        .into_iter()
        .filter_map(|u| u.kind().map(|kind| ConflictInfo { path: u.path, kind }))
        .collect())
}

const OPEN: &str = "MEMO-EDITOR-OURS";
const CLOSE: &str = "MEMO-EDITOR-THEIRS";
/// Markdown の見出し下線(=======)と区別できるよう、マーカーを長くする
const MARKER: usize = 32;

pub fn detail(root: &Path, path: &str, device: &str) -> Result<ConflictDetail> {
    let g = repo_git(root);
    let u = unmerged(&g)?
        .into_iter()
        .find(|u| u.path == path)
        .ok_or_else(|| SyncError::state("この競合は既に解消されています"))?;
    let kind = u.kind().ok_or_else(|| SyncError::state("この競合は既に解消されています"))?;
    let (theirs_label, theirs_time) = theirs_info(&g);
    let mut d = ConflictDetail {
        path: path.to_string(),
        kind,
        ours_label: device.to_string(),
        theirs_label,
        theirs_time,
        segments: Vec::new(),
        remaining_text: None,
    };
    let abs = memo_core::paths::resolve(root, path)?;
    match kind {
        ConflictKind::BothModified => {
            // 競合が出た後にこの端末で編集していれば、それを「この端末の版」とする
            let ours = if abs.is_file() { fs::read(&abs)? } else { blob(&g, 2, path)? };
            let base = if u.stages[1] { blob(&g, 1, path)? } else { Vec::new() };
            let theirs = blob(&g, 3, path)?;
            d.segments = segments(root, &text::decode(&ours).content, &text::decode(&base).content, &text::decode(&theirs).content)?;
        }
        ConflictKind::DeletedByThem => d.remaining_text = Some(text::decode(&blob(&g, 2, path)?).content),
        ConflictKind::DeletedByUs => d.remaining_text = Some(text::decode(&blob(&g, 3, path)?).content),
    }
    Ok(d)
}

/// 3 方向マージ(git merge-file)の結果を、共通部分と選ぶ部分に分ける
fn segments(root: &Path, ours: &str, base: &str, theirs: &str) -> Result<Vec<Segment>> {
    let tmp = git_dir(root).join("memo-sync");
    fs::create_dir_all(&tmp)?;
    let files = [("ours.txt", ours), ("base.txt", base), ("theirs.txt", theirs)];
    for (name, body) in files {
        fs::write(tmp.join(name), body)?;
    }
    let p = |n: &str| tmp.join(n).to_string_lossy().into_owned();
    let marker = format!("--marker-size={MARKER}");
    let out = Git::bare().raw(
        [
            "merge-file", "-p", &marker, "-L", OPEN, "-L", "MEMO-EDITOR-BASE", "-L", CLOSE,
            &p("ours.txt"), &p("base.txt"), &p("theirs.txt"),
        ],
        LOCAL_TIMEOUT,
    )?;
    let _ = fs::remove_dir_all(&tmp);
    // 0..=127 は競合の数。それ以上はエラー(古い git でオプション未対応など)
    let parsed = if (0..128).contains(&out.code) { parse_markers(&String::from_utf8_lossy(&out.stdout)) } else { None };
    Ok(parsed.unwrap_or_else(|| {
        if ours == theirs {
            vec![Segment::Same { text: ours.to_string() }]
        } else {
            vec![Segment::Conflict { ours: ours.to_string(), theirs: theirs.to_string() }]
        }
    }))
}

fn parse_markers(s: &str) -> Option<Vec<Segment>> {
    let open = format!("{} {OPEN}", "<".repeat(MARKER));
    let mid = "=".repeat(MARKER);
    let close = format!("{} {CLOSE}", ">".repeat(MARKER));
    enum St {
        Same,
        Ours,
        Theirs,
    }
    let mut st = St::Same;
    let (mut same, mut ours, mut theirs) = (String::new(), String::new(), String::new());
    let mut out = Vec::new();
    for line in s.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        match st {
            St::Same if bare == open => {
                if !same.is_empty() {
                    out.push(Segment::Same { text: std::mem::take(&mut same) });
                }
                st = St::Ours;
            }
            St::Ours if bare == mid => st = St::Theirs,
            St::Theirs if bare == close => {
                out.push(Segment::Conflict { ours: std::mem::take(&mut ours), theirs: std::mem::take(&mut theirs) });
                st = St::Same;
            }
            St::Same => same.push_str(line),
            St::Ours => ours.push_str(line),
            St::Theirs => theirs.push_str(line),
        }
    }
    if !matches!(st, St::Same) {
        return None;
    }
    if !same.is_empty() {
        out.push(Segment::Same { text: same });
    }
    Some(out)
}

/// 競合を 1 件解消する。戻り値は残りの競合数
pub fn resolve(root: &Path, path: &str, res: &Resolution) -> Result<usize> {
    let g = repo_git(root);
    let u = unmerged(&g)?
        .into_iter()
        .find(|u| u.path == path)
        .ok_or_else(|| SyncError::state("この競合は既に解消されています"))?;
    let kind = u.kind();
    let abs = memo_core::paths::resolve(root, path)?;
    match res {
        Resolution::Content { text: body } => {
            // 文字コード・改行コードはこの端末の版(なければ相手の版)に合わせる
            let sample = if abs.is_file() { fs::read(&abs)? } else if u.stages[3] { blob(&g, 3, path)? } else { blob(&g, 2, path)? };
            let mut tf = text::decode(&sample);
            tf.content = body.replace("\r\n", "\n");
            text::write_atomic(&abs, &text::encode(&tf)?)?;
            g.local(["add", "--", path], "競合の解消")?;
        }
        Resolution::Ours => {
            if abs.is_file() {
                g.local(["add", "--", path], "競合の解消")?;
            } else {
                g.local(["rm", "-q", "--cached", "--", path], "競合の解消")?;
            }
        }
        Resolution::Theirs => {
            if u.stages[3] {
                g.local(["checkout", "--theirs", "--", path], "競合の解消")?;
                g.local(["add", "--", path], "競合の解消")?;
            } else {
                g.local(["rm", "-q", "-f", "--", path], "競合の解消")?;
            }
        }
        Resolution::Both => {
            if kind != Some(ConflictKind::BothModified) {
                return Err(SyncError::state("この競合では「両方残す」は使えません"));
            }
            let (theirs_name, _) = theirs_info(&g);
            let (copy_abs, copy_rel) = copy_path(root, path, &theirs_name);
            fs::write(&copy_abs, blob(&g, 3, path)?)?;
            g.local(["add", "--", path, &copy_rel], "競合の解消")?;
        }
    }
    Ok(unmerged(&g)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marker_output() {
        let o = format!("<{} {OPEN}", "<".repeat(MARKER - 1));
        let m = "=".repeat(MARKER);
        let c = format!(">{} {CLOSE}", ">".repeat(MARKER - 1));
        let s = format!("head\n{o}\nmine\n=======\n{m}\ntheirs\n{c}\ntail\n");
        let seg = parse_markers(&s).unwrap();
        assert_eq!(
            seg,
            vec![
                Segment::Same { text: "head\n".into() },
                // 7 個の = は Markdown の見出し下線として本文扱い
                Segment::Conflict { ours: "mine\n=======\n".into(), theirs: "theirs\n".into() },
                Segment::Same { text: "tail\n".into() },
            ]
        );
    }

    #[test]
    fn unbalanced_markers_fall_back() {
        let o = format!("{} {OPEN}", "<".repeat(MARKER));
        assert!(parse_markers(&format!("{o}\nmine\n")).is_none());
    }

    #[test]
    fn copy_names_avoid_collisions_and_bad_chars() {
        let dir = tempfile::tempdir().unwrap();
        let (abs, rel) = copy_path(dir.path(), "技術/報告書.xlsx", "PC:01");
        assert!(rel.starts_with("技術/報告書 (競合 PC-01 "));
        assert!(rel.ends_with(").xlsx"));
        fs::create_dir_all(abs.parent().unwrap()).unwrap();
        fs::write(&abs, b"x").unwrap();
        let (_, rel2) = copy_path(dir.path(), "技術/報告書.xlsx", "PC:01");
        assert_ne!(rel, rel2);
    }
}
