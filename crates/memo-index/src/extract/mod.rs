//! 文書からのテキスト抽出。
//!
//! どの形式も「位置情報付きのセグメント列」に揃える。位置情報は検索結果から
//! 該当箇所へ戻るために使う(テキストは行番号、PDF はページ、Excel はシートと行、
//! PowerPoint はスライド番号、Word / Markdown は見出し)。

mod office;
mod pdf;
mod text;

use std::path::Path;

use serde::Serialize;

use crate::Result;

pub use pdf::{init_pdfium, pdfium_status};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Loc {
    /// 人が読む位置の説明(「## 構成案」「p.3」「見積 行2–5」「スライド 2: タイトル」)
    pub label: String,
    /// テキストファイルの開始行(1 始まり)
    pub line: Option<u32>,
    /// PDF のページ / PowerPoint のスライド番号(1 始まり)
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub text: String,
    pub loc: Loc,
}

impl Segment {
    pub fn new(text: impl Into<String>, label: impl Into<String>) -> Self {
        Self { text: text.into(), loc: Loc { label: label.into(), ..Default::default() } }
    }
}

/// インデックス対象の拡張子と、そのファイル種別名
pub fn kind_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "md" | "markdown" => "markdown",
        "txt" | "log" => "text",
        "csv" | "tsv" => "csv",
        "html" | "htm" => "html",
        "docx" => "docx",
        "xlsx" | "xlsm" | "xls" | "ods" => "excel",
        "pptx" => "pptx",
        "pdf" => "pdf",
        _ => return None,
    })
}

/// ファイルを読み取ってセグメント列にする。対象外の拡張子は `Ok(None)`。
pub fn extract(path: &Path) -> Result<Option<Vec<Segment>>> {
    let Some(kind) = kind_for(path) else { return Ok(None) };
    let segs = match kind {
        "markdown" => text::markdown(path)?,
        "text" => text::plain(path)?,
        "csv" => text::csv(path)?,
        "html" => text::html(path)?,
        "docx" => office::docx(path)?,
        "excel" => office::excel(path)?,
        "pptx" => office::pptx(path)?,
        "pdf" => pdf::pdf(path)?,
        _ => unreachable!(),
    };
    Ok(Some(
        segs.into_iter()
            .map(|mut s| {
                s.text = s.text.trim_end().to_string();
                s
            })
            .filter(|s| !s.text.trim().is_empty())
            .collect(),
    ))
}

#[cfg(test)]
pub(crate) fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}
