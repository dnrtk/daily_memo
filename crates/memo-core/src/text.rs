//! 文字コード・改行コードを保ったままテキストを読み書きする。
//!
//! 業務の CSV は Shift_JIS が普通にあるので、UTF-8 でデコードできなければ
//! Shift_JIS として読み、保存時も同じエンコーディングに戻す。

use std::{fs, io::Write, path::Path};

use serde::{Deserialize, Serialize};

use crate::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    ShiftJis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextFile {
    /// 改行は常に `\n` に正規化済み
    pub content: String,
    pub encoding: Encoding,
    pub eol: Eol,
}

/// エディタで開くテキスト系の拡張子
pub const TEXT_EXTS: &[&str] = &[
    "md", "markdown", "txt", "csv", "tsv", "html", "htm", "json", "yaml", "yml", "toml", "xml",
    "log", "ini", "sh", "ps1", "py", "rs", "ts", "js", "go", "c", "h", "sql",
];

/// 将来インデックス対象にするバイナリ文書
pub const DOC_EXTS: &[&str] = &["pdf", "docx", "xlsx", "xlsm", "pptx"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Text,
    Document,
    Other,
}

pub fn kind_of(path: &Path) -> FileKind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if TEXT_EXTS.contains(&ext.as_str()) {
        FileKind::Text
    } else if DOC_EXTS.contains(&ext.as_str()) {
        FileKind::Document
    } else {
        FileKind::Other
    }
}

pub fn decode(bytes: &[u8]) -> TextFile {
    let (content, encoding) = if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        (String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8Bom)
    } else if let Ok(s) = std::str::from_utf8(bytes) {
        (s.to_owned(), Encoding::Utf8)
    } else {
        let (s, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
        (s.into_owned(), Encoding::ShiftJis)
    };
    let eol = if content.contains("\r\n") { Eol::Crlf } else { Eol::Lf };
    TextFile {
        content: content.replace("\r\n", "\n"),
        encoding,
        eol,
    }
}

pub fn encode(file: &TextFile) -> Result<Vec<u8>> {
    let body = match file.eol {
        Eol::Lf => file.content.clone(),
        Eol::Crlf => file.content.replace('\n', "\r\n"),
    };
    Ok(match file.encoding {
        Encoding::Utf8 => body.into_bytes(),
        Encoding::Utf8Bom => {
            let mut v = b"\xEF\xBB\xBF".to_vec();
            v.extend_from_slice(body.as_bytes());
            v
        }
        Encoding::ShiftJis => {
            let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&body);
            if had_errors {
                return Err(CoreError::Encoding(
                    "Shift_JIS で表現できない文字が含まれています".into(),
                ));
            }
            bytes.into_owned()
        }
    })
}

pub fn read(path: &Path) -> Result<TextFile> {
    Ok(decode(&fs::read(path)?))
}

/// 一時ファイルに書いてから rename する(書き込み途中のクラッシュで壊さない)。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::InvalidPath(path.display().to_string()))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_sjis_crlf() {
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("氏名,部署\r\n山田,開発\r\n");
        let f = decode(&sjis);
        assert_eq!(f.encoding, Encoding::ShiftJis);
        assert_eq!(f.eol, Eol::Crlf);
        assert_eq!(f.content, "氏名,部署\n山田,開発\n");
        assert_eq!(encode(&f).unwrap(), sjis.into_owned());
    }

    #[test]
    fn roundtrip_bom() {
        let raw = b"\xEF\xBB\xBFhello\n".to_vec();
        let f = decode(&raw);
        assert_eq!(f.encoding, Encoding::Utf8Bom);
        assert_eq!(encode(&f).unwrap(), raw);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a.MD")), FileKind::Text);
        assert_eq!(kind_of(Path::new("a.pptx")), FileKind::Document);
        assert_eq!(kind_of(Path::new("a.png")), FileKind::Other);
    }
}
