//! アプリ設定(作業フォルダではなく OS のユーザー設定ディレクトリに置く)。
//!
//! LLM の接続先はマシンごとの事情なので、作業フォルダ側ではなくここに持つ。
//! Windows: `%APPDATA%\<identifier>\settings.yaml`
//! Linux:   `~/.config/<identifier>/settings.yaml`

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{text, CoreError, Result};

pub const SETTINGS_FILE: &str = "settings.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Ollama ネイティブ API(`/api/tags`, `/api/embed`, `/api/chat`)
    Ollama,
    /// OpenAI 互換 API(vLLM / Ollama の `/v1` / LM Studio など)。`base_url` は `/v1` まで含める
    OpenaiCompat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// 平文保存。社内 LAN 内の推論サーバー想定。必要になったら OS キーチェーンに移す
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    60
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

/// 埋め込みモデルの指定。ruri 系のように接頭辞を要求するモデル向けに prefix を持てる
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingRef {
    pub provider: String,
    pub model: String,
    /// 検索クエリの前に付ける文字列(例: ruri-v3 は "検索クエリ: ")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query_prefix: String,
    /// 文書チャンクの前に付ける文字列(例: ruri-v3 は "検索文書: ")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub document_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Roles {
    /// 検索インデックス用の埋め込みモデル。変更するとインデックスは再構築が必要
    pub embedding: EmbeddingRef,
    /// 保存先振り分け・クエリ解釈用の LLM
    pub chat: ModelRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorSettings {
    /// 0 で自動保存オフ
    #[serde(default = "default_autosave")]
    pub autosave_ms: u64,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_true")]
    pub line_wrapping: bool,
}

fn default_autosave() -> u64 {
    1000
}
fn default_font_size() -> u32 {
    14
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchSettings {
    /// 検索文を LLM で解釈して期間・種類などの条件に分解する
    #[serde(default = "default_true")]
    pub use_llm_query: bool,
    /// これより大きいファイルはインデックスしない(MB)
    #[serde(default = "default_max_file_mb")]
    pub max_file_mb: u64,
    /// インデックス対象外にするフォルダ名(どの階層でも一致したら除外)。`.` 始まりは常に除外
    #[serde(default = "default_excludes")]
    pub exclude_dirs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalSettings {
    /// 起動するシェルと引数(空白区切り)。空なら Windows は PowerShell(pwsh があれば優先)、Linux は $SHELL
    #[serde(default)]
    pub shell: String,
}

fn default_max_file_mb() -> u64 {
    50
}
fn default_excludes() -> Vec<String> {
    vec!["node_modules".into(), "target".into(), "__pycache__".into()]
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            use_llm_query: true,
            max_file_mb: default_max_file_mb(),
            exclude_dirs: default_excludes(),
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            autosave_ms: default_autosave(),
            font_size: default_font_size(),
            line_wrapping: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub providers: Vec<Provider>,
    pub roles: Roles,
    #[serde(default)]
    pub editor: EditorSettings,
    #[serde(default)]
    pub search: SearchSettings,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_workspace: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            providers: vec![Provider {
                id: "local-ollama".into(),
                kind: ProviderKind::Ollama,
                base_url: "http://127.0.0.1:11434".into(),
                api_key: None,
                timeout_secs: default_timeout(),
            }],
            roles: Roles {
                embedding: EmbeddingRef {
                    provider: "local-ollama".into(),
                    model: "bge-m3".into(),
                    query_prefix: String::new(),
                    document_prefix: String::new(),
                },
                chat: ModelRef {
                    provider: "local-ollama".into(),
                    model: "qwen3:4b".into(),
                },
            },
            editor: EditorSettings::default(),
            search: SearchSettings::default(),
            terminal: TerminalSettings::default(),
            last_workspace: None,
        }
    }
}

impl AppSettings {
    pub fn load_or_default(dir: &Path) -> Result<Self> {
        let path = dir.join(SETTINGS_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let s: AppSettings = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        s.validate()?;
        Ok(s)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        self.validate()?;
        let body = serde_yaml_ng::to_string(self)?;
        text::write_atomic(&dir.join(SETTINGS_FILE), body.as_bytes())
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn validate(&self) -> Result<()> {
        let err = |m: String| Err(CoreError::InvalidSettings(m));
        let mut seen = std::collections::HashSet::new();
        for p in &self.providers {
            if p.id.trim().is_empty() {
                return err("プロバイダ ID が空です".into());
            }
            if !seen.insert(&p.id) {
                return err(format!("プロバイダ ID が重複しています ({})", p.id));
            }
            if !(p.base_url.starts_with("http://") || p.base_url.starts_with("https://")) {
                return err(format!(
                    "base_url は http:// か https:// で始めてください ({})",
                    p.base_url
                ));
            }
        }
        for (role, provider) in [
            ("embedding", &self.roles.embedding.provider),
            ("chat", &self.roles.chat.provider),
        ] {
            if self.provider(provider).is_none() {
                return err(format!("{role} のプロバイダ '{provider}' が存在しません"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = AppSettings::default();
        s.save(dir.path()).unwrap();
        assert_eq!(AppSettings::load_or_default(dir.path()).unwrap(), s);
    }

    #[test]
    fn parses_mixed_providers() {
        let yaml = r#"
providers:
  - id: local-ollama
    kind: ollama
    base_url: http://127.0.0.1:11434
  - id: lab-vllm
    kind: openai_compat
    base_url: http://192.168.10.20:8000/v1
    api_key: token
roles:
  embedding: { provider: local-ollama, model: bge-m3 }
  chat: { provider: lab-vllm, model: Qwen3-8B }
"#;
        let s: AppSettings = serde_yaml_ng::from_str(yaml).unwrap();
        s.validate().unwrap();
        assert_eq!(s.provider("lab-vllm").unwrap().kind, ProviderKind::OpenaiCompat);
        assert_eq!(s.editor, EditorSettings::default());
        assert_eq!(s.search, SearchSettings::default());
        assert!(s.roles.embedding.query_prefix.is_empty());
    }

    #[test]
    fn rejects_dangling_role() {
        let mut s = AppSettings::default();
        s.roles.chat.provider = "missing".into();
        assert!(s.validate().is_err());
    }
}
