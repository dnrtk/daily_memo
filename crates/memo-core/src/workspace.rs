//! 作業フォルダ。開くと `.memo/` とカテゴリフォルダを用意する。

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::{
    category::{Category, CategoryConfig, SaveMode},
    paths::{self, META_DIR},
    template::{self, Vars},
    text::{self, FileKind, TextFile},
    CoreError, Result,
};

#[derive(Debug, Clone, Serialize)]
pub struct DirEntry {
    pub name: String,
    /// `/` 区切りの作業フォルダ相対パス
    pub path: String,
    pub is_dir: bool,
    pub kind: Option<FileKind>,
    /// カテゴリのルートフォルダなら、そのカテゴリ ID
    pub category_id: Option<String>,
    pub size: u64,
    /// UNIX 秒
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveRequest {
    /// 新規ファイルモードのファイル名(slug)に使う。追記モードでは無視
    #[serde(default)]
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Saved {
    pub path: String,
    pub category_id: String,
    pub appended: bool,
}

pub struct Workspace {
    root: PathBuf,
    config: CategoryConfig,
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(CoreError::NotFound(root.display().to_string()));
        }
        let root = paths::canonical_dir(root)?;
        let meta = root.join(META_DIR);
        fs::create_dir_all(&meta)?;
        let config = CategoryConfig::load_or_init(&meta)?;
        let ws = Self { root, config };
        ws.ensure_category_dirs()?;
        Ok(ws)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn meta_dir(&self) -> PathBuf {
        self.root.join(META_DIR)
    }

    pub fn config(&self) -> &CategoryConfig {
        &self.config
    }

    fn ensure_category_dirs(&self) -> Result<()> {
        for c in &self.config.categories {
            fs::create_dir_all(paths::resolve(&self.root, &c.dir)?)?;
        }
        Ok(())
    }

    /// カテゴリ定義を差し替える。
    ///
    /// - 既存 ID のフォルダ名が変わった → フォルダを rename
    /// - 新しい ID → フォルダを作成
    /// - 消えた ID → 定義から外すだけ。フォルダとファイルは残す(誤操作でメモを消さない)
    pub fn update_categories(&mut self, next: CategoryConfig) -> Result<()> {
        next.validate()?;
        for c in &next.categories {
            let Some(prev) = self.config.get(&c.id) else { continue };
            if prev.dir == c.dir {
                continue;
            }
            let from = paths::resolve(&self.root, &prev.dir)?;
            let to = paths::resolve(&self.root, &c.dir)?;
            if from.exists() {
                if to.exists() {
                    return Err(CoreError::AlreadyExists(c.dir.clone()));
                }
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(from, to)?;
            }
        }
        next.save(&self.meta_dir())?;
        self.config = next;
        self.ensure_category_dirs()
    }

    // ---- ファイルツリー --------------------------------------------------

    /// 1 階層分の一覧(遅延展開用)。隠しファイルと Office のロックファイルは除外。
    pub fn list_dir(&self, rel: &str) -> Result<Vec<DirEntry>> {
        let dir = paths::resolve(&self.root, rel)?;
        if !dir.is_dir() {
            return Err(CoreError::NotFound(rel.into()));
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with("~$") {
                continue;
            }
            let path = entry.path();
            let md = entry.metadata()?;
            let rel_path = paths::to_rel(&self.root, &path);
            let is_dir = md.is_dir();
            out.push(DirEntry {
                category_id: if is_dir { self.category_for_dir(&rel_path) } else { None },
                kind: (!is_dir).then(|| text::kind_of(&path)),
                size: if is_dir { 0 } else { md.len() },
                modified: md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                name,
                path: rel_path,
                is_dir,
            });
        }
        // ルートはカテゴリ定義順 → その他フォルダ → ファイル
        let order = |e: &DirEntry| {
            e.category_id
                .as_ref()
                .and_then(|id| self.config.categories.iter().position(|c| &c.id == id))
                .unwrap_or(usize::MAX)
        };
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then(order(a).cmp(&order(b)))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    fn category_for_dir(&self, rel: &str) -> Option<String> {
        self.config
            .categories
            .iter()
            .find(|c| c.dir.trim_matches('/') == rel)
            .map(|c| c.id.clone())
    }

    // ---- ファイル操作 ------------------------------------------------------

    pub fn read_text(&self, rel: &str) -> Result<TextFile> {
        let p = paths::resolve(&self.root, rel)?;
        if !p.is_file() {
            return Err(CoreError::NotFound(rel.into()));
        }
        text::read(&p)
    }

    pub fn write_text(&self, rel: &str, file: &TextFile) -> Result<()> {
        let p = paths::resolve_writable(&self.root, rel)?;
        text::write_atomic(&p, &text::encode(file)?)
    }

    pub fn create_file(&self, rel: &str) -> Result<String> {
        let p = paths::resolve_writable(&self.root, rel)?;
        if p.exists() {
            return Err(CoreError::AlreadyExists(rel.into()));
        }
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&p, b"")?;
        Ok(paths::to_rel(&self.root, &p))
    }

    pub fn create_dir(&self, rel: &str) -> Result<String> {
        let p = paths::resolve_writable(&self.root, rel)?;
        if p.exists() {
            return Err(CoreError::AlreadyExists(rel.into()));
        }
        fs::create_dir_all(&p)?;
        Ok(paths::to_rel(&self.root, &p))
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<String> {
        if self.category_for_dir(from.trim_matches('/')).is_some() {
            return Err(CoreError::InvalidPath(
                "カテゴリフォルダの名前は設定画面から変更してください".into(),
            ));
        }
        let src = paths::resolve_writable(&self.root, from)?;
        let dst = paths::resolve_writable(&self.root, to)?;
        if !src.exists() {
            return Err(CoreError::NotFound(from.into()));
        }
        if dst.exists() {
            return Err(CoreError::AlreadyExists(to.into()));
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&src, &dst)?;
        Ok(paths::to_rel(&self.root, &dst))
    }

    /// 完全削除はせず OS のゴミ箱へ送る
    pub fn trash(&self, rel: &str) -> Result<()> {
        if self.category_for_dir(rel.trim_matches('/')).is_some() {
            return Err(CoreError::InvalidPath(
                "カテゴリフォルダは設定画面からカテゴリを外してから削除してください".into(),
            ));
        }
        let p = paths::resolve_writable(&self.root, rel)?;
        if !p.exists() {
            return Err(CoreError::NotFound(rel.into()));
        }
        trash::delete(&p).map_err(|e| CoreError::Trash(e.to_string()))
    }

    pub fn abs_path(&self, rel: &str) -> Result<PathBuf> {
        paths::resolve(&self.root, rel)
    }

    // ---- メモの保存 --------------------------------------------------------

    /// 今日の日次ノートを(なければ作って)返す
    pub fn daily_note(&self, now: NaiveDateTime) -> Result<String> {
        let id = self
            .config
            .daily_category
            .clone()
            .ok_or_else(|| CoreError::InvalidCategory("日次カテゴリが設定されていません".into()))?;
        let cat = self.category(&id)?;
        let p = self.category_file_path(cat, &Vars { now, title: "", content: "" })?;
        self.ensure_file(&p, cat, &Vars { now, title: "", content: "" })?;
        Ok(paths::to_rel(&self.root, &p))
    }

    /// カテゴリを指定してメモを保存する。
    /// 将来の「LLM に振り分けさせて保存」はカテゴリ ID を決めたあとここを呼ぶ。
    pub fn save_to_category(
        &self,
        category_id: &str,
        req: &SaveRequest,
        now: NaiveDateTime,
    ) -> Result<Saved> {
        let cat = self.category(category_id)?;
        let vars = Vars { now, title: req.title.trim(), content: req.content.trim_end() };
        match cat.mode {
            SaveMode::Append => {
                let p = self.category_file_path(cat, &vars)?;
                self.ensure_file(&p, cat, &vars)?;
                let mut file = text::read(&p)?;
                let entry_tpl = cat.entry_template.as_deref().unwrap_or("{content}\n\n");
                let entry = template::render(entry_tpl, &vars);
                separate_entry(&mut file.content, &entry);
                file.content.push_str(&entry);
                text::write_atomic(&p, &text::encode(&file)?)?;
                Ok(Saved {
                    path: paths::to_rel(&self.root, &p),
                    category_id: cat.id.clone(),
                    appended: true,
                })
            }
            SaveMode::NewFile => {
                let title = if vars.title.is_empty() {
                    first_line(vars.content)
                } else {
                    vars.title.to_string()
                };
                let vars = Vars { title: &title, ..vars };
                let p = paths::unique_path(self.category_file_path(cat, &vars)?);
                let head = cat.file_template.as_deref().map(|t| template::render(t, &vars));
                let body = format!("{}{}\n", head.unwrap_or_default(), vars.content);
                text::write_atomic(&p, body.as_bytes())?;
                Ok(Saved {
                    path: paths::to_rel(&self.root, &p),
                    category_id: cat.id.clone(),
                    appended: false,
                })
            }
        }
    }

    fn category(&self, id: &str) -> Result<&Category> {
        self.config
            .get(id)
            .ok_or_else(|| CoreError::NotFound(format!("カテゴリ '{id}'")))
    }

    fn category_file_path(&self, cat: &Category, vars: &Vars) -> Result<PathBuf> {
        let rel = format!("{}/{}", cat.dir.trim_end_matches('/'), template::render(&cat.filename, vars));
        paths::resolve_writable(&self.root, &rel)
    }

    fn ensure_file(&self, p: &Path, cat: &Category, vars: &Vars) -> Result<()> {
        if p.exists() {
            return Ok(());
        }
        let head = cat.file_template.as_deref().map(|t| template::render(t, vars));
        text::write_atomic(p, head.unwrap_or_default().as_bytes())
    }
}

/// 追記前の区切り。見出しで始まるエントリなら空行を 1 つ挟み、
/// それ以外(Todo のリスト項目など)は改行だけ補う。
fn separate_entry(content: &mut String, entry: &str) {
    if content.is_empty() {
        return;
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    if entry.starts_with('#') && !content.ends_with("\n\n") {
        content.push('\n');
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("memo")
        .chars()
        .take(40)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 26)
            .unwrap()
            .and_hms_opt(15, 43, 0)
            .unwrap()
    }

    #[test]
    fn open_creates_base_folders() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        for name in ["日々", "技術", "調査", "Todo", "アイデア", "教訓"] {
            assert!(dir.path().join(name).is_dir(), "{name}");
        }
        assert!(dir.path().join(".memo/categories.yaml").is_file());
        let root: Vec<_> = ws.list_dir("").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(root, ["日々", "技術", "調査", "Todo", "アイデア", "教訓"]);
    }

    #[test]
    fn daily_note_and_append() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rel = ws.daily_note(now()).unwrap();
        assert_eq!(rel, "日々/2026/10/2026-10-26.md");
        let req = SaveRequest { title: String::new(), content: "設計レビュー".into() };
        let saved = ws.save_to_category("daily", &req, now()).unwrap();
        assert!(saved.appended);
        assert_eq!(
            ws.read_text(&rel).unwrap().content,
            "# 2026-10-26\n\n## 15:43\n\n設計レビュー\n\n"
        );
    }

    #[test]
    fn new_file_mode_uses_title_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let req = SaveRequest { title: "Ollama 手順".into(), content: "ollama pull bge-m3".into() };
        let a = ws.save_to_category("tech", &req, now()).unwrap();
        let b = ws.save_to_category("tech", &req, now()).unwrap();
        assert_eq!(a.path, "技術/20261026-Ollama-手順.md");
        assert_eq!(b.path, "技術/20261026-Ollama-手順-2.md");
        assert_eq!(
            ws.read_text(&a.path).unwrap().content,
            "# Ollama 手順\n\nollama pull bge-m3\n"
        );
    }

    #[test]
    fn append_after_manual_edit_keeps_blank_line() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let rel = ws.daily_note(now()).unwrap();
        ws.write_text(&rel, &text::decode("# 2026-10-26\n\n手で書いた行".as_bytes())).unwrap();
        let req = SaveRequest { title: String::new(), content: "追記".into() };
        ws.save_to_category("daily", &req, now()).unwrap();
        assert_eq!(
            ws.read_text(&rel).unwrap().content,
            "# 2026-10-26\n\n手で書いた行\n\n## 15:43\n\n追記\n\n"
        );
    }

    #[test]
    fn todo_appends_checkbox() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let req = SaveRequest { title: String::new(), content: "見積もり提出".into() };
        ws.save_to_category("todo", &req, now()).unwrap();
        ws.save_to_category("todo", &req, now()).unwrap();
        assert_eq!(
            ws.read_text("Todo/inbox.md").unwrap().content,
            "# Todo\n\n- [ ] 見積もり提出\n- [ ] 見積もり提出\n"
        );
    }

    #[test]
    fn update_categories_renames_and_adds_but_never_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let mut ws = Workspace::open(dir.path()).unwrap();
        ws.save_to_category(
            "ideas",
            &SaveRequest { title: "x".into(), content: "y".into() },
            now(),
        )
        .unwrap();

        let mut cfg = ws.config().clone();
        cfg.categories.iter_mut().find(|c| c.id == "ideas").unwrap().dir = "ネタ帳".into();
        cfg.categories.retain(|c| c.id != "lessons");
        let mut extra = cfg.categories[1].clone();
        extra.id = "meeting".into();
        extra.name = "会議".into();
        extra.dir = "会議".into();
        cfg.categories.push(extra);
        ws.update_categories(cfg).unwrap();

        assert!(!dir.path().join("アイデア").exists());
        assert!(dir.path().join("ネタ帳/20261026-x.md").is_file());
        assert!(dir.path().join("会議").is_dir());
        assert!(dir.path().join("教訓").is_dir(), "外したカテゴリのフォルダは残す");

        // 再オープンしても定義が残っている
        let ws2 = Workspace::open(dir.path()).unwrap();
        assert!(ws2.config().get("meeting").is_some());
        assert!(ws2.config().get("lessons").is_none());
    }

    #[test]
    fn file_ops_and_guards() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open(dir.path()).unwrap();
        let f = ws.create_file("技術/rust/memo.md").unwrap();
        assert_eq!(f, "技術/rust/memo.md");
        assert!(ws.create_file("技術/rust/memo.md").is_err());
        assert_eq!(ws.rename(&f, "技術/rust/notes.md").unwrap(), "技術/rust/notes.md");
        assert!(ws.rename("技術", "tech").is_err(), "カテゴリフォルダは直接 rename しない");
        assert!(ws.write_text(".memo/categories.yaml", &text::decode(b"x")).is_err());
        assert!(ws.list_dir("").unwrap().iter().all(|e| e.name != ".memo"));
    }
}
