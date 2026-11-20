//! 実際の git で「ファイルサーバ上のリモート + 2 端末」を作り、接続・同期・競合解消を通しで確かめる。
//! git がない環境ではスキップする。

use std::{fs, path::Path};

use memo_sync::{
    conflict::{self, ConflictKind, Resolution, Segment},
    remote::{self, ProbeState},
    repo::{self, RepoState, SyncOptions},
};

fn git_available() -> bool {
    memo_sync::git::version().is_some()
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
}

fn opts(device: &str) -> SyncOptions {
    SyncOptions { device: device.into(), message: None }
}

struct Env {
    _tmp: tempfile::TempDir,
    url: String,
    a: std::path::PathBuf,
    b: std::path::PathBuf,
}

/// A がファイルサーバに同期を作り、B が参加した状態
fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let url = tmp.path().join("nas").join("memo.git").to_string_lossy().into_owned();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::create_dir_all(tmp.path().join("nas")).unwrap();

    write(&a, ".memo/categories.yaml", "version: 1\n# A のカテゴリ\n");
    write(&a, ".memo/index.sqlite", "binary-index");
    write(&a, "日々/2026-11-20.md", "# 11/20\n\n- A で書いた\n");
    write(&a, "技術/rust.md", "line1\nline2\nline3\n");

    let probe = remote::probe(&url).unwrap();
    assert_eq!(probe.state, ProbeState::WillCreate);
    let r = repo::connect(&a, &url, "PC-A").unwrap();
    assert!(r.pushed, "初回は送信する");

    // B: 既定のカテゴリ設定と、同名で中身の違うファイル・B だけのファイルを持っている
    write(&b, ".memo/categories.yaml", "version: 1\n# B の既定値\n");
    write(&b, "技術/rust.md", "B のローカル版\n");
    write(&b, "アイデア/b-only.md", "B だけのメモ\n");
    let probe = remote::probe(&url).unwrap();
    assert!(matches!(probe.state, ProbeState::HasData { .. }));
    let r = repo::connect(&b, &url, "PC-B").unwrap();
    assert!(r.pushed);
    assert_eq!(r.auto_resolved.len(), 1, "同名で内容の違うファイルは B の版を別名で残す: {:?}", r.auto_resolved);
    let copy = r.auto_resolved[0].copy.clone().unwrap();
    assert!(copy.starts_with("技術/rust (競合 PC-B "));
    assert_eq!(read(&b, &copy), "B のローカル版\n");
    assert_eq!(read(&b, "技術/rust.md"), "line1\nline2\nline3\n");
    assert_eq!(read(&b, ".memo/categories.yaml"), "version: 1\n# A のカテゴリ\n", "カテゴリ設定はリモート優先");
    assert_eq!(read(&b, "日々/2026-11-20.md"), "# 11/20\n\n- A で書いた\n");

    // A が B の追加分を受け取る
    let r = repo::sync(&a, &opts("PC-A")).unwrap();
    assert!(r.pulled);
    assert!(r.changed.iter().any(|c| c.path == "アイデア/b-only.md"));
    assert_eq!(read(&a, "アイデア/b-only.md"), "B だけのメモ\n");
    Env { _tmp: tmp, url, a, b }
}

#[test]
fn connect_and_exchange() {
    if !git_available() {
        return;
    }
    let e = setup();
    let st = repo::status(&e.a);
    assert_eq!(st.repo, RepoState::Ready);
    assert!(st.pending.is_empty());
    assert_eq!(st.ahead, 0);
    assert!(!e.a.join(".memo/index.sqlite").exists() || !st.pending.iter().any(|c| c.path.contains("index.sqlite")));
    // 履歴に端末名が残る
    let h = repo::history(&e.a, 10);
    assert!(h.iter().any(|c| c.device == "PC-B"), "{h:?}");
    let _ = e.url;
}

#[test]
fn non_overlapping_edits_merge_automatically() {
    if !git_available() {
        return;
    }
    let e = setup();
    write(&e.a, "技術/rust.md", "line1 (A)\nline2\nline3\n");
    write(&e.b, "技術/rust.md", "line1\nline2\nline3 (B)\n");
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert!(r.conflicts.is_empty());
    assert!(r.pushed);
    assert_eq!(read(&e.b, "技術/rust.md"), "line1 (A)\nline2\nline3 (B)\n");
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    assert_eq!(read(&e.a, "技術/rust.md"), "line1 (A)\nline2\nline3 (B)\n");
}

#[test]
fn text_conflict_is_resolved_by_hunk() {
    if !git_available() {
        return;
    }
    let e = setup();
    write(&e.a, "技術/rust.md", "line1\nline2 A\nline3\n");
    write(&e.b, "技術/rust.md", "line1\nline2 B\nline3\n");
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].kind, ConflictKind::BothModified);
    // 作業ツリーにはマーカーではなく B の版が残る
    assert_eq!(read(&e.b, "技術/rust.md"), "line1\nline2 B\nline3\n");
    let st = repo::status(&e.b);
    assert!(st.merging);
    assert_eq!(st.conflicts, vec!["技術/rust.md".to_string()]);

    // 未解消のまま同期しても進まない
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert_eq!(r.conflicts.len(), 1);
    assert!(!r.pushed);

    let d = conflict::detail(&e.b, "技術/rust.md", "PC-B").unwrap();
    assert_eq!(d.theirs_label, "PC-A");
    assert_eq!(
        d.segments,
        vec![
            Segment::Same { text: "line1\n".into() },
            Segment::Conflict { ours: "line2 B\n".into(), theirs: "line2 A\n".into() },
            Segment::Same { text: "line3\n".into() },
        ]
    );
    let left = conflict::resolve(&e.b, "技術/rust.md", &Resolution::Content { text: "line1\nline2 A\nline2 B\nline3\n".into() }).unwrap();
    assert_eq!(left, 0);
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert!(r.pushed);
    assert!(!repo::status(&e.b).merging);
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    assert_eq!(read(&e.a, "技術/rust.md"), "line1\nline2 A\nline2 B\nline3\n");
}

#[test]
fn keep_both_files() {
    if !git_available() {
        return;
    }
    let e = setup();
    write(&e.a, "日々/2026-11-20.md", "A の版\n");
    write(&e.b, "日々/2026-11-20.md", "B の版\n");
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert_eq!(r.conflicts.len(), 1);
    conflict::resolve(&e.b, "日々/2026-11-20.md", &Resolution::Both).unwrap();
    repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert_eq!(read(&e.b, "日々/2026-11-20.md"), "B の版\n");
    let copies: Vec<_> = fs::read_dir(e.b.join("日々"))
        .unwrap()
        .filter_map(|x| x.ok())
        .map(|x| x.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("競合 PC-A"))
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(read(&e.b, &format!("日々/{}", copies[0])), "A の版\n");
}

#[test]
fn binary_conflict_keeps_both_automatically() {
    if !git_available() {
        return;
    }
    let e = setup();
    fs::write(e.a.join("見積.xlsx"), b"PK\x03\x04\x00original").unwrap();
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    repo::sync(&e.b, &opts("PC-B")).unwrap();
    fs::write(e.a.join("見積.xlsx"), b"PK\x03\x04\x00from A").unwrap();
    fs::write(e.b.join("見積.xlsx"), b"PK\x03\x04\x00from B").unwrap();
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert!(r.conflicts.is_empty(), "バイナリは確認なしで解決する");
    assert_eq!(r.auto_resolved.len(), 1);
    assert!(r.pushed);
    assert_eq!(fs::read(e.b.join("見積.xlsx")).unwrap(), b"PK\x03\x04\x00from B");
    let copy = r.auto_resolved[0].copy.clone().unwrap();
    assert_eq!(fs::read(e.b.join(&copy)).unwrap(), b"PK\x03\x04\x00from A");
}

#[test]
fn delete_vs_edit_asks() {
    if !git_available() {
        return;
    }
    let e = setup();
    fs::remove_file(e.a.join("技術/rust.md")).unwrap();
    write(&e.b, "技術/rust.md", "B が編集\n");
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    let r = repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].kind, ConflictKind::DeletedByThem);
    let d = conflict::detail(&e.b, "技術/rust.md", "PC-B").unwrap();
    assert_eq!(d.remaining_text.as_deref(), Some("B が編集\n"));
    conflict::resolve(&e.b, "技術/rust.md", &Resolution::Ours).unwrap();
    repo::sync(&e.b, &opts("PC-B")).unwrap();
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    assert_eq!(read(&e.a, "技術/rust.md"), "B が編集\n");
}

#[test]
fn crlf_and_sjis_survive() {
    if !git_available() {
        return;
    }
    let e = setup();
    let crlf = b"a\r\nb\r\n".to_vec();
    fs::write(e.a.join("crlf.txt"), &crlf).unwrap();
    repo::sync(&e.a, &opts("PC-A")).unwrap();
    repo::sync(&e.b, &opts("PC-B")).unwrap();
    assert_eq!(fs::read(e.b.join("crlf.txt")).unwrap(), crlf, "改行コードを変換しない");
}

#[test]
fn offline_keeps_local_commit() {
    if !git_available() {
        return;
    }
    let e = setup();
    let moved = e.url.clone() + ".away";
    fs::rename(&e.url, &moved).unwrap();
    write(&e.a, "日々/offline.md", "電車の中で書いた\n");
    let err = repo::sync(&e.a, &opts("PC-A")).unwrap_err();
    assert_eq!(err.kind, memo_sync::ErrorKind::NotFound);
    let st = repo::status(&e.a);
    assert!(st.pending.is_empty(), "コミットは済ませておく");
    assert_eq!(st.ahead, 1);
    fs::rename(&moved, &e.url).unwrap();
    let r = repo::sync(&e.a, &opts("PC-A")).unwrap();
    assert!(r.pushed);
    assert_eq!(repo::status(&e.a).ahead, 0);
}
