//! カテゴリ定義(`<作業フォルダ>/.memo/categories.yaml`)。
//!
//! `description` と `examples` は、LLM に「xxx を保存しておいて」と頼んだときの
//! 振り分け判断の材料になる。ここを具体的に書くほど振り分け精度が上がる。

use std::{collections::HashSet, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{paths, text, CoreError, Result};

pub const CATEGORIES_FILE: &str = "categories.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SaveMode {
    /// 保存のたびに新しいファイルを作る
    #[default]
    NewFile,
    /// `filename` で決まる 1 ファイルに追記する(日次ノート・Todo 受け箱など)
    Append,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Category {
    /// 安定 ID。表示名やフォルダ名を変えても変わらない
    pub id: String,
    /// 表示名
    pub name: String,
    /// 作業フォルダからの相対パス
    pub dir: String,
    /// このカテゴリに入れるべき内容の定義(LLM 振り分け用)
    #[serde(default)]
    pub description: String,
    /// 典型例(LLM 振り分け用の few-shot)
    #[serde(default)]
    pub examples: Vec<String>,
    /// カテゴリフォルダからの相対ファイル名パターン(`template` モジュール参照)
    #[serde(default = "default_filename")]
    pub filename: String,
    #[serde(default)]
    pub mode: SaveMode,
    /// 新規ファイル作成時の初期内容
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_template: Option<String>,
    /// 追記モード時、1 回分のエントリの書式
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_template: Option<String>,
}

fn default_filename() -> String {
    "{yyyy}{MM}{dd}-{slug}.md".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 「今日のメモ」で開くカテゴリの ID
    #[serde(default)]
    pub daily_category: Option<String>,
    pub categories: Vec<Category>,
}

fn default_version() -> u32 {
    1
}

impl Default for CategoryConfig {
    fn default() -> Self {
        let cat = |id: &str, name: &str, description: &str, examples: &[&str]| Category {
            id: id.into(),
            name: name.into(),
            dir: name.into(),
            description: description.into(),
            examples: examples.iter().map(|s| s.to_string()).collect(),
            filename: default_filename(),
            mode: SaveMode::NewFile,
            file_template: Some("# {title}\n\n".into()),
            entry_template: None,
        };
        let mut daily = cat(
            "daily",
            "日々",
            "その日の作業ログ・出来事・雑感。日付を軸に時系列で残す記録。",
            &["今日は午前中に障害対応、午後は設計レビュー", "朝会で決まったこと"],
        );
        daily.filename = "{yyyy}/{MM}/{yyyy}-{MM}-{dd}.md".into();
        daily.mode = SaveMode::Append;
        daily.file_template = Some("# {yyyy}-{MM}-{dd}\n\n".into());
        daily.entry_template = Some("## {HH}:{mm}\n\n{content}\n\n".into());

        let mut todo = cat(
            "todo",
            "Todo",
            "これからやるべき作業・依頼事項・期限のあるタスク。",
            &["来週までに見積もりを出す", "〇〇さんに資料を送る"],
        );
        todo.filename = "inbox.md".into();
        todo.mode = SaveMode::Append;
        todo.file_template = Some("# Todo\n\n".into());
        todo.entry_template = Some("- [ ] {content}\n".into());

        Self {
            version: 1,
            daily_category: Some("daily".into()),
            categories: vec![
                daily,
                cat(
                    "tech",
                    "技術",
                    "再利用できる技術的な知見。手順・コマンド・設定値・設計パターン・ハマりどころの解決策。",
                    &["Ollama のモデル保存先を変える手順", "Rust の非同期で Mutex を跨ぐときの注意"],
                ),
                cat(
                    "research",
                    "調査",
                    "特定の問いに対して調べた結果。比較・出典・結論を含む調査メモ。",
                    &["ベクトル DB の比較", "〇〇ライブラリのライセンス確認"],
                ),
                todo,
                cat(
                    "ideas",
                    "アイデア",
                    "まだ検証していない思いつき・企画・改善案。",
                    &["メモツールに音声入力を付けたら便利そう"],
                ),
                cat(
                    "lessons",
                    "教訓",
                    "失敗や成功から得た、次の判断に効く学び。原因と次回の打ち手を書く。",
                    &["見積もり前に依存ライブラリのライセンスを確認すべきだった"],
                ),
            ],
        }
    }
}

impl CategoryConfig {
    pub fn load_or_init(meta_dir: &Path) -> Result<Self> {
        let path = meta_dir.join(CATEGORIES_FILE);
        if path.exists() {
            let cfg: CategoryConfig = serde_yaml_ng::from_str(&fs::read_to_string(&path)?)?;
            cfg.validate()?;
            Ok(cfg)
        } else {
            let cfg = CategoryConfig::default();
            cfg.save(meta_dir)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, meta_dir: &Path) -> Result<()> {
        self.validate()?;
        let header = "# memo-editor カテゴリ定義\n\
                      # description / examples は LLM による保存先振り分けの判断材料になります。\n";
        let body = format!("{header}{}", serde_yaml_ng::to_string(self)?);
        text::write_atomic(&meta_dir.join(CATEGORIES_FILE), body.as_bytes())
    }

    pub fn get(&self, id: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    pub fn validate(&self) -> Result<()> {
        let err = |m: String| Err(CoreError::InvalidCategory(m));
        let mut ids = HashSet::new();
        let mut dirs = HashSet::new();
        for c in &self.categories {
            if c.id.trim().is_empty() || c.name.trim().is_empty() || c.dir.trim().is_empty() {
                return err(format!("ID・表示名・フォルダは必須です ({})", c.name));
            }
            if !c
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            {
                return err(format!("ID は英数字・-・_ のみ使えます ({})", c.id));
            }
            if !ids.insert(c.id.as_str()) {
                return err(format!("ID が重複しています ({})", c.id));
            }
            let dir = c.dir.trim_matches('/');
            if !dirs.insert(dir.to_string()) {
                return err(format!("フォルダが重複しています ({})", c.dir));
            }
            if paths::is_meta(dir) || dir.split('/').any(|s| s == ".." || s.is_empty()) {
                return err(format!("使えないフォルダ名です ({})", c.dir));
            }
            if c.filename.contains("..") || c.filename.starts_with('/') {
                return err(format!("ファイル名パターンが不正です ({})", c.filename));
            }
        }
        if let Some(d) = &self.daily_category {
            if self.get(d).is_none() {
                return err(format!("日次カテゴリ '{d}' が存在しません"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_base_six_and_is_valid() {
        let c = CategoryConfig::default();
        let names: Vec<_> = c.categories.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["日々", "技術", "調査", "Todo", "アイデア", "教訓"]);
        c.validate().unwrap();
    }

    #[test]
    fn yaml_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let c = CategoryConfig::default();
        c.save(dir.path()).unwrap();
        assert_eq!(CategoryConfig::load_or_init(dir.path()).unwrap(), c);
    }

    #[test]
    fn rejects_bad_definitions() {
        let mut c = CategoryConfig::default();
        c.categories[1].dir = "日々".into();
        assert!(c.validate().is_err());

        let mut c = CategoryConfig::default();
        c.categories[1].dir = ".memo".into();
        assert!(c.validate().is_err());

        let mut c = CategoryConfig::default();
        c.categories[1].id = "技術".into();
        assert!(c.validate().is_err());

        let c = CategoryConfig { daily_category: Some("nope".into()), ..Default::default() };
        assert!(c.validate().is_err());
    }
}
