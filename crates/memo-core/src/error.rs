use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("ファイル操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("設定ファイルの形式が不正です: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("不正なパスです: {0}")]
    InvalidPath(String),
    #[error("見つかりません: {0}")]
    NotFound(String),
    #[error("既に存在します: {0}")]
    AlreadyExists(String),
    #[error("カテゴリ定義が不正です: {0}")]
    InvalidCategory(String),
    #[error("設定が不正です: {0}")]
    InvalidSettings(String),
    #[error("LLM サーバーとの通信に失敗しました: {0}")]
    Http(String),
    #[error("ゴミ箱への移動に失敗しました: {0}")]
    Trash(String),
    #[error("文字コード変換に失敗しました: {0}")]
    Encoding(String),
}

impl From<reqwest::Error> for CoreError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest の生メッセージは長いので、よくある原因は短く言い換える
        let host = e
            .url()
            .map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")) + &u.port().map(|p| format!(":{p}")).unwrap_or_default())
            .unwrap_or_default();
        let msg = if e.is_connect() {
            format!("{host} に接続できません(サーバーが起動しているか確認してください)")
        } else if e.is_timeout() {
            format!("{host} からの応答がタイムアウトしました")
        } else if e.is_decode() {
            format!("{host} の応答を解釈できません: {e}")
        } else {
            e.to_string()
        };
        CoreError::Http(msg)
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
