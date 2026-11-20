//! git CLI の呼び出し。
//!
//! - 対話プロンプトは出さない(GIT_TERMINAL_PROMPT=0)。GUI の Git Credential Manager はそのまま使える
//! - メッセージは英語(LC_ALL=C)にして原因を判定しやすくする
//! - Windows ではコンソールウィンドウを出さない
//! - 応答がなければ打ち切る(ネットワーク操作は長め)

use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{error::ErrorKind, Result, SyncError};

pub const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_string()
    }

    /// `-z` 出力を NUL で区切る
    pub fn nul_split(&self) -> Vec<String> {
        self.stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Git {
    dir: Option<PathBuf>,
    config: Vec<String>,
    envs: Vec<(String, String)>,
}

impl Git {
    pub fn at(dir: &Path) -> Self {
        Self { dir: Some(dir.to_path_buf()), ..Self::default() }
    }

    /// 作業フォルダを指定しない呼び出し(バージョン確認・ls-remote など)
    pub fn bare() -> Self {
        Self::default()
    }

    /// `-c key=value` を付ける
    pub fn config(mut self, kv: impl Into<String>) -> Self {
        self.config.push(kv.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.envs.push((k.into(), v.into()));
        self
    }

    /// 終了コードに関わらず結果を返す。git を起動できないときだけ Err
    pub fn raw<I, S>(&self, args: I, timeout: Duration) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        if let Some(d) = &self.dir {
            cmd.arg("-C").arg(d);
        }
        for c in ["core.quotepath=false", "color.ui=false", "core.pager=cat"] {
            cmd.arg("-c").arg(c);
        }
        for c in &self.config {
            cmd.arg("-c").arg(c);
        }
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("GIT_MERGE_AUTOEDIT", "no");
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SyncError::new(ErrorKind::GitMissing, "Git が見つかりません。Git をインストールしてください")
            } else {
                SyncError::new(ErrorKind::Other, format!("git を起動できません: {e}"))
            }
        })?;
        wait_with_timeout(child, timeout)
    }

    /// 失敗したら stderr から原因を推定した Err にする
    pub fn run<I, S>(&self, args: I, timeout: Duration, what: &str) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.raw(args, timeout)?;
        if out.ok() {
            Ok(out)
        } else {
            Err(SyncError::from_git(&out.stderr, &format!("{what}に失敗しました")))
        }
    }

    /// ローカル操作用の短縮形
    pub fn local<I, S>(&self, args: I, what: &str) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(args, LOCAL_TIMEOUT, what)
    }

    /// 成否だけ知りたいとき(rev-parse --verify など)
    pub fn check<I, S>(&self, args: I) -> Result<bool>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.raw(args, LOCAL_TIMEOUT)?.ok())
    }

    /// パスを多数渡す操作は、コマンドライン長の上限(Windows は 32K 文字)に収まるよう分割する
    pub fn local_paths(&self, head: &[&str], paths: &[String], what: &str) -> Result<()> {
        for chunk in paths.chunks(200) {
            let mut args: Vec<&str> = head.to_vec();
            args.push("--");
            args.extend(chunk.iter().map(String::as_str));
            self.local(&args, what)?;
        }
        Ok(())
    }
}

fn wait_with_timeout(mut child: Child, timeout: Duration) -> Result<Output> {
    // パイプが詰まらないよう別スレッドで読み切る
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_t = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_t = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let mut sleep = Duration::from_millis(2);
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                // 子プロセス(ssh 等)がパイプを握ったままの場合があるので読み取りスレッドは待たない
                return Err(SyncError::new(ErrorKind::Timeout, "Git の応答がタイムアウトしました"));
            }
            None => {
                thread::sleep(sleep);
                sleep = (sleep * 2).min(Duration::from_millis(25));
            }
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
    Ok(Output { code: status.code().unwrap_or(-1), stdout, stderr })
}

/// `git version 2.43.0` → `2.43.0`。git がなければ None
pub fn version() -> Option<String> {
    let out = Git::bare().raw(["--version"], Duration::from_secs(10)).ok()?;
    out.ok().then(|| out.text().trim_start_matches("git version ").to_string())
}
