//! memo-sync: 作業フォルダを Git で複数端末に同期する。
//!
//! - `git`:      git CLI の呼び出し(タイムアウト・コンソール非表示・エラー分類)
//! - `remote`:   リモートの種類判定・接続前の確認・ファイルサーバへのリポジトリ作成
//! - `repo`:     状態取得・接続(初回)・同期(コミット → 取得 → マージ → 送信)
//! - `conflict`: 競合の一覧・詳細(箇所ごとの差分)・解消
//!
//! libgit2 ではなくシステムの git を使う。GitHub / GitLab の HTTPS 認証は Git Credential Manager、
//! SSH は ssh-agent、ファイルサーバはパス指定と、ユーザーが普段使う git の設定がそのまま効くため。

pub mod conflict;
mod error;
pub mod git;
pub mod remote;
pub mod repo;

pub use conflict::{ConflictDetail, ConflictInfo, ConflictKind, Resolution, Segment};
pub use error::{ErrorKind, SyncError};
pub use remote::{Probe, ProbeState, RemoteKind};
pub use repo::{AutoResolved, Change, ChangeKind, CommitInfo, RepoState, SyncOptions, SyncReport, SyncStatus};

pub type Result<T> = std::result::Result<T, SyncError>;

/// 端末名の既定値(Windows: COMPUTERNAME / Linux: ホスト名)
pub fn default_device_name() -> String {
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty());
    from_env
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "この端末".to_string())
}
