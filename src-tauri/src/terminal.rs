//! 組み込みターミナル(PTY)。Windows は ConPTY、Linux は openpty。
//!
//! - 出力は読み取りスレッドが `term-output` イベントで画面へ送る(UTF-8 の途中で切れないよう調整)
//! - プロセス終了は待機スレッドが検知して `term-exit` を送り、セッションを片付ける
//!   (Windows の ConPTY は疑似コンソールを閉じるまで読み取りが EOF にならないため、終了検知は wait 側で行う)
//! - 画面側でタブを非表示にしてもセッションは残る。× で閉じたときだけ kill する
//! - 出力は直近 1MB 程度を履歴として保持する。タブを別グループ / 別ウィンドウへ移したとき、
//!   新しい画面は `snapshot` で履歴を再生してから続きの出力を受け取る。
//!   出力イベントには通算バイト位置(start)を付け、履歴と重複・欠落しないようにしている

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

struct Session {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// 読み取りスレッドと共有(Session 自体を持たせると master が閉じなくなるので分ける)
    history: Arc<Mutex<History>>,
}

/// 出力の履歴。`total` はこれまでの通算バイト数
#[derive(Default)]
struct History {
    buf: String,
    total: u64,
}

const HISTORY_MAX: usize = 1024 * 1024;
const HISTORY_KEEP: usize = 512 * 1024;

impl History {
    /// offset 以降の部分(offset は出力チャンクの境界なので文字の途中にはならない)
    fn since(&self, offset: u64) -> &str {
        let base = self.total - self.buf.len() as u64;
        if offset <= base {
            return &self.buf;
        }
        let i = ((offset - base) as usize).min(self.buf.len());
        if self.buf.is_char_boundary(i) {
            &self.buf[i..]
        } else {
            ""
        }
    }

    /// 追記して、追記したデータの通算開始位置を返す
    fn push(&mut self, data: &str) -> u64 {
        let start = self.total;
        self.buf.push_str(data);
        self.total += data.len() as u64;
        if self.buf.len() > HISTORY_MAX {
            // 直近 HISTORY_KEEP バイト程度を、行の頭から残す
            let mut cut = self.buf.len() - HISTORY_KEEP;
            while !self.buf.is_char_boundary(cut) {
                cut += 1;
            }
            if let Some(nl) = self.buf[cut..].find('\n') {
                cut += nl + 1;
            }
            self.buf.drain(..cut);
        }
        start
    }
}

#[derive(Serialize)]
pub struct Snapshot {
    pub data: String,
    /// この位置以降の出力イベントを続けて書けばよい
    pub end: u64,
}

#[derive(Default)]
pub struct Terminals {
    next: AtomicU32,
    sessions: Arc<Mutex<HashMap<u32, Arc<Session>>>>,
}

#[derive(Clone, Serialize)]
struct Output {
    id: u32,
    data: String,
    start: u64,
}

#[derive(Clone, Serialize)]
struct Exit {
    id: u32,
    code: Option<u32>,
}

#[derive(Serialize)]
pub struct Spawned {
    pub id: u32,
    /// タブ名に使うシェル名(PowerShell / pwsh / bash など)
    pub shell: String,
}

/// 使うシェルを決める。設定が空なら OS ごとの既定
fn shell_command(configured: &str) -> (CommandBuilder, String) {
    let configured = configured.trim();
    if !configured.is_empty() {
        let mut parts = configured.split_whitespace();
        let prog = parts.next().unwrap_or(configured);
        let mut cmd = CommandBuilder::new(prog);
        cmd.args(parts);
        return (cmd, display_name(prog));
    }
    #[cfg(windows)]
    {
        // PowerShell 7 (pwsh) が入っていれば優先、なければ Windows PowerShell
        let prog = if find_in_path("pwsh.exe").is_some() { "pwsh.exe" } else { "powershell.exe" };
        let mut cmd = CommandBuilder::new(prog);
        cmd.arg("-NoLogo");
        (cmd, "PowerShell".into())
    }
    #[cfg(not(windows))]
    {
        let prog = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/bash".into());
        let mut cmd = CommandBuilder::new(&prog);
        cmd.arg("-l");
        let name = display_name(&prog);
        (cmd, name)
    }
}

fn display_name(prog: &str) -> String {
    let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    match base.to_ascii_lowercase().as_str() {
        "powershell" | "pwsh" => "PowerShell".into(),
        _ => base.to_string(),
    }
}

#[cfg(windows)]
fn find_in_path(exe: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(exe)).find(|f| f.is_file()))
}

/// 読み取ったバイト列を、UTF-8 として完結している部分だけ文字列にする。残りは次回に持ち越す
fn take_utf8(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&pending[..valid]).unwrap());
                match e.error_len() {
                    // 末尾が文字の途中 → 持ち越し
                    None => {
                        pending.drain(..valid);
                        return out;
                    }
                    // 不正なバイト → 置換文字にして続行
                    Some(n) => {
                        out.push('\u{FFFD}');
                        pending.drain(..valid + n);
                    }
                }
            }
        }
    }
}

impl Terminals {
    pub fn spawn(
        &self,
        app: AppHandle,
        cwd: Option<PathBuf>,
        shell: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Spawned, String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("疑似端末を作れません: {e}"))?;
        let (mut cmd, shell_name) = shell_command(shell);
        if let Some(dir) = cwd.filter(|d| d.is_dir()) {
            cmd.cwd(dir);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| format!("シェルを起動できません: {e}"))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let history = Arc::new(Mutex::new(History::default()));
        let session = Arc::new(Session {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(child.clone_killer()),
            history: Arc::clone(&history),
        });
        self.sessions.lock().unwrap().insert(id, session);

        // 出力の読み取り
        let out_app = app.clone();
        std::thread::Builder::new()
            .name(format!("term-read-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut pending = Vec::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            pending.extend_from_slice(&buf[..n]);
                            let data = take_utf8(&mut pending);
                            if !data.is_empty() {
                                // 履歴への追記とイベント送信を同じロック内で行い、snapshot と順序を揃える
                                let mut h = history.lock().unwrap();
                                let start = h.push(&data);
                                let _ = out_app.emit("term-output", Output { id, data, start });
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        // 終了の検知
        let sessions = Arc::clone(&self.sessions);
        std::thread::Builder::new()
            .name(format!("term-wait-{id}"))
            .spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code());
                // セッションを外すと master が閉じ、読み取りスレッドも終わる
                sessions.lock().unwrap().remove(&id);
                let _ = app.emit("term-exit", Exit { id, code });
            })
            .map_err(|e| e.to_string())?;

        Ok(Spawned { id, shell: shell_name })
    }

    fn get(&self, id: u32) -> Result<Arc<Session>, String> {
        self.sessions.lock().unwrap().get(&id).cloned().ok_or_else(|| "ターミナルは終了しています".into())
    }

    /// 通算位置 offset 以降の出力(付け替え中の取りこぼしを埋める)。履歴から消えていれば残っている分だけ
    pub fn since(&self, id: u32, offset: u64) -> Result<Snapshot, String> {
        let s = self.get(id)?;
        let h = s.history.lock().unwrap();
        Ok(Snapshot { data: h.since(offset).to_string(), end: h.total })
    }

    /// 履歴の取得(別の画面に付け替えるとき用)
    pub fn snapshot(&self, id: u32) -> Result<Snapshot, String> {
        let s = self.get(id)?;
        let h = s.history.lock().unwrap();
        Ok(Snapshot { data: h.buf.clone(), end: h.total })
    }

    pub fn write(&self, id: u32, data: &str) -> Result<(), String> {
        let s = self.get(id)?;
        let mut w = s.writer.lock().unwrap();
        w.write_all(data.as_bytes()).and_then(|_| w.flush()).map_err(|e| e.to_string())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let s = self.get(id)?;
        let m = s.master.lock().unwrap();
        m.resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())
    }

    pub fn kill(&self, id: u32) {
        if let Ok(s) = self.get(id) {
            let _ = s.killer.lock().unwrap().kill();
        }
        self.sessions.lock().unwrap().remove(&id);
    }

    pub fn kill_all(&self) {
        let all: Vec<u32> = self.sessions.lock().unwrap().keys().copied().collect();
        for id in all {
            self.kill(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_reads() {
        let bytes = "あいう".as_bytes();
        let mut p = bytes[..4].to_vec(); // 「あ」+「い」の 1 バイト目
        assert_eq!(take_utf8(&mut p), "あ");
        assert_eq!(p.len(), 1);
        p.extend_from_slice(&bytes[4..]);
        assert_eq!(take_utf8(&mut p), "いう");
        assert!(p.is_empty());
        let mut bad = vec![b'a', 0xFF, b'b'];
        assert_eq!(take_utf8(&mut bad), "a\u{FFFD}b");
    }

    #[test]
    fn history_trims_at_line_boundary_and_tracks_offsets() {
        let mut h = History::default();
        assert_eq!(h.push("abc\n"), 0);
        assert_eq!(h.push("de"), 4);
        let line = format!("{}\n", "あ".repeat(1000)); // 3001 バイト
        for _ in 0..400 {
            h.push(&line);
        }
        assert_eq!(h.total, 6 + 3001 * 400);
        assert!(h.buf.len() <= HISTORY_MAX);
        assert!(h.buf.starts_with('あ'), "行の途中から始まらない");
        let end = h.total;
        h.push("tail");
        assert_eq!(h.since(end), "tail");
        assert_eq!(h.since(h.total), "");
        assert_eq!(h.since(0).len(), h.buf.len(), "消えた範囲は残っている分だけ返す");
    }

    #[test]
    fn names() {
        assert_eq!(display_name("C:\\Program Files\\PowerShell\\7\\pwsh.exe"), "PowerShell");
        assert_eq!(display_name("/usr/bin/zsh"), "zsh");
    }
}
