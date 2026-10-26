use thiserror::Error;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("ファイル操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("データベースエラー: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Core(#[from] memo_core::CoreError),
    #[error("文書の読み取りに失敗しました: {0}")]
    Extract(String),
    #[error("PDF を読むための PDFium が見つかりません: {0}")]
    PdfiumUnavailable(String),
    #[error("埋め込みに失敗しました: {0}")]
    Embed(String),
}

impl From<zip::result::ZipError> for IndexError {
    fn from(e: zip::result::ZipError) -> Self {
        IndexError::Extract(format!("ZIP: {e}"))
    }
}

impl From<quick_xml::Error> for IndexError {
    fn from(e: quick_xml::Error) -> Self {
        IndexError::Extract(format!("XML: {e}"))
    }
}

impl From<calamine::Error> for IndexError {
    fn from(e: calamine::Error) -> Self {
        IndexError::Extract(format!("Excel: {e}"))
    }
}

impl From<pdfium_render::prelude::PdfiumError> for IndexError {
    fn from(e: pdfium_render::prelude::PdfiumError) -> Self {
        IndexError::Extract(format!("PDF: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, IndexError>;
