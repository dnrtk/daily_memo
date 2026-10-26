//! PDF(PDFium を動的ロード)
//!
//! PDFium はプロセス内で 1 回しか初期化できないため、グローバルに 1 つだけ持つ。
//! ライブラリが見つからなければ PDF だけ対象外にして、他の形式は普通に処理する。

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use pdfium_render::prelude::*;

use super::{Loc, Segment};
use crate::{IndexError, Result};

static PDFIUM: OnceLock<std::result::Result<Pdfium, String>> = OnceLock::new();

/// 候補ディレクトリから PDFium を探して初期化する。2 回目以降は最初の結果を返す。
pub fn init_pdfium(candidates: &[PathBuf]) -> std::result::Result<(), String> {
    PDFIUM
        .get_or_init(|| {
            let mut tried = Vec::new();
            for dir in candidates {
                let lib = Pdfium::pdfium_platform_library_name_at_path(dir);
                if !lib.exists() {
                    tried.push(lib.display().to_string());
                    continue;
                }
                return Pdfium::bind_to_library(&lib)
                    .map(Pdfium::new)
                    .map_err(|e| format!("{}: {e}", lib.display()));
            }
            Err(format!("見つかりません(探した場所: {})", tried.join(", ")))
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

/// 状態表示用。未初期化なら None
pub fn pdfium_status() -> Option<std::result::Result<(), String>> {
    PDFIUM.get().map(|r| r.as_ref().map(|_| ()).map_err(Clone::clone))
}

pub fn pdf(path: &Path) -> Result<Vec<Segment>> {
    let pdfium = match PDFIUM.get() {
        Some(Ok(p)) => p,
        Some(Err(e)) => return Err(IndexError::PdfiumUnavailable(e.clone())),
        None => return Err(IndexError::PdfiumUnavailable("未初期化".into())),
    };
    let doc = pdfium.load_pdf_from_file(path, None)?;
    let mut out = Vec::new();
    for (i, page) in doc.pages().iter().enumerate() {
        let n = i as u32 + 1;
        let text = page.text()?.all();
        out.push(Segment { text, loc: Loc { label: format!("p.{n}"), line: None, page: Some(n) } });
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn test_pdfium_dir() -> Option<PathBuf> {
    // テストでは MEMO_PDFIUM_DIR か src-tauri/pdfium を見る
    std::env::var_os("MEMO_PDFIUM_DIR").map(PathBuf::from).or_else(|| {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/pdfium");
        p.exists().then_some(p)
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixture;
    use super::*;

    #[test]
    fn pdf_pages() {
        let Some(dir) = test_pdfium_dir() else {
            eprintln!("PDFium が無いのでスキップ(npm run fetch-pdfium で取得)");
            return;
        };
        init_pdfium(&[dir]).unwrap();
        let segs = pdf(&fixture("report.pdf")).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].loc.page, Some(1));
        assert!(segs[0].text.contains("推論サーバーのメモリ不足"), "{}", segs[0].text);
        assert!(segs[1].text.contains("KV キャッシュ"), "{}", segs[1].text);
    }
}
