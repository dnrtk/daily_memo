use serde::Serialize;
use thiserror::Error;

/// 画面で出し分けるためのエラー種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// git コマンドが見つからない
    GitMissing,
    /// ネットワークに出られない(オフライン・VPN・プロキシ)
    Network,
    /// 認証失敗・権限なし
    Auth,
    /// リモートのリポジトリが見つからない
    NotFound,
    /// ファイルサーバ上のリポジトリの所有者が違う(safe.directory)
    Ownership,
    /// 送信が拒否された(他端末が先に送信)
    Rejected,
    /// git が応答しない
    Timeout,
    /// 別の同期が実行中
    Busy,
    /// 設定・状態が同期できる状態にない
    State,
    Other,
}

#[derive(Debug, Clone, Error, Serialize)]
#[error("{message}")]
pub struct SyncError {
    pub kind: ErrorKind,
    pub message: String,
    /// git の stderr など(折りたたんで表示する)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SyncError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), detail: None }
    }

    pub fn state(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::State, message)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let d = detail.into();
        self.detail = (!d.trim().is_empty()).then_some(d);
        self
    }

    /// git の失敗出力から、原因を推定した利用者向けのエラーを作る
    pub fn from_git(stderr: &str, fallback: &str) -> Self {
        let kind = classify(stderr);
        let message = match kind {
            ErrorKind::Network => "リモートに接続できません。ネットワーク(オフライン・VPN・プロキシ)を確認してください".into(),
            ErrorKind::Auth => "認証に失敗しました。リモートへのアクセス権と認証情報(SSH 鍵 / アクセストークン)を確認してください".into(),
            ErrorKind::NotFound => "リモートのリポジトリが見つかりません。URL / パスを確認してください".into(),
            ErrorKind::Ownership => "ファイルサーバのリポジトリの所有者が異なるため、Git に拒否されました".into(),
            ErrorKind::Rejected => "他の端末が先に送信しました。もう一度同期してください".into(),
            _ => fallback.to_string(),
        };
        Self::new(kind, message).with_detail(tail(stderr, 20))
    }
}

impl From<std::io::Error> for SyncError {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Other, format!("ファイル操作に失敗しました: {e}"))
    }
}

impl From<memo_core::CoreError> for SyncError {
    fn from(e: memo_core::CoreError) -> Self {
        Self::new(ErrorKind::Other, e.to_string())
    }
}

/// git は LC_ALL=C で動かしているので、英語のメッセージで判定できる
pub(crate) fn classify(stderr: &str) -> ErrorKind {
    let s = stderr.to_ascii_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| s.contains(k));
    if s.contains("dubious ownership") {
        ErrorKind::Ownership
    } else if any(&[
        "authentication failed",
        "permission denied",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "invalid username or password",
        "access denied",
        "returned error: 401",
        "returned error: 403",
        "host key verification failed",
    ]) {
        ErrorKind::Auth
    } else if any(&[
        "could not resolve host",
        "could not resolve hostname",
        "failed to connect",
        "connection timed out",
        "connection refused",
        "network is unreachable",
        "no route to host",
        "operation timed out",
        "connection reset",
        "unable to access",
        "the remote end hung up unexpectedly",
    ]) {
        ErrorKind::Network
    } else if any(&[
        "repository not found",
        "does not appear to be a git repository",
        "returned error: 404",
        "no such file or directory",
    ]) {
        ErrorKind::NotFound
    } else if any(&["[rejected]", "non-fast-forward", "fetch first", "failed to push some refs"]) {
        ErrorKind::Rejected
    } else {
        ErrorKind::Other
    }
}

fn tail(s: &str, lines: usize) -> String {
    let v: Vec<&str> = s.trim().lines().collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_failures() {
        assert_eq!(classify("fatal: unable to access 'https://github.com/x/y.git/': Could not resolve host: github.com"), ErrorKind::Network);
        assert_eq!(classify("fatal: unable to access 'https://x/': The requested URL returned error: 403"), ErrorKind::Auth);
        assert_eq!(classify("git@github.com: Permission denied (publickey)."), ErrorKind::Auth);
        assert_eq!(classify("fatal: '/mnt/nas/memo.git' does not appear to be a git repository"), ErrorKind::NotFound);
        assert_eq!(classify(" ! [rejected]        main -> main (fetch first)"), ErrorKind::Rejected);
        assert_eq!(classify("fatal: detected dubious ownership in repository at '//nas/memo.git'"), ErrorKind::Ownership);
    }
}
