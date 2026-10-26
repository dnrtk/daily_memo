//! memo-core: UI 非依存のコアロジック。
//!
//! - `workspace`: 作業フォルダの初期化・ファイル操作・カテゴリ管理
//! - `category`:  カテゴリ定義(`.memo/categories.yaml`)
//! - `settings`:  アプリ設定(LLM プロバイダ・役割ごとのモデル)
//! - `llm`:       Ollama / OpenAI 互換 API クライアント
//! - `paths`:     作業フォルダ外へ出ないためのパス検証
//! - `template`:  ファイル名・追記エントリのテンプレート展開
//! - `text`:      文字コード・改行コードを保ったテキスト入出力
//!
//! Tauri 側はこのクレートの薄いラッパーに留め、検索インデックス等も
//! ここ(または別クレート)に追加していく方針。

pub mod category;
pub mod error;
pub mod llm;
pub mod paths;
pub mod settings;
pub mod template;
pub mod text;
pub mod workspace;

pub use error::{CoreError, Result};
