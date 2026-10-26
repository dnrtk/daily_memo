//! memo-index: 作業フォルダの文書を抽出・インデックス化し、ハイブリッド検索する。
//!
//! - `extract`:  形式ごとのテキスト抽出(位置情報付き)
//! - `chunk`:    抽出結果を検索単位に分割
//! - `tokenize`: 全文検索用の文字バイグラム化(辞書不要)
//! - `store`:    SQLite(FTS5 + sqlite-vec)
//! - `embed`:    埋め込みの抽象化(実運用は Ollama / OpenAI 互換、テストは偽実装)
//! - `indexer`:  差分インデックス
//! - `search`:   LLM によるクエリ解釈 + BM25 / ベクトルの RRF 統合
//! - `watch`:    フォルダ監視

pub mod chunk;
pub mod embed;
mod error;
pub mod extract;
pub mod indexer;
pub mod search;
pub mod store;
pub mod tokenize;
pub mod watch;

pub use error::{IndexError, Result};
