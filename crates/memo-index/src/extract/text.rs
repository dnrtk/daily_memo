//! Markdown / テキスト / CSV / HTML

use std::path::Path;

use super::{Loc, Segment};
use crate::Result;

fn read(path: &Path) -> Result<String> {
    Ok(memo_core::text::read(path)?.content)
}

/// 見出し単位で区切る。ラベルは「親 > 子」の見出しパス。コードブロック内の `#` は無視。
pub fn markdown(path: &Path) -> Result<Vec<Segment>> {
    Ok(split_markdown(&read(path)?))
}

pub(crate) fn split_markdown(src: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut buf = String::new();
    let mut start_line = 1u32;
    let mut in_fence = false;

    // 見出しだけのまとまりは次の節に持ち越す
    let mut body_seen = false;
    let flush = |buf: &mut String, stack: &[(usize, String)], start: u32, out: &mut Vec<Segment>| {
        if !buf.trim().is_empty() {
            let label = stack.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" > ");
            out.push(Segment { text: std::mem::take(buf), loc: Loc { label, line: Some(start), page: None } });
        }
        buf.clear();
    };

    for (i, line) in src.lines().enumerate() {
        let lineno = i as u32 + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let level = if in_fence { 0 } else { trimmed.chars().take_while(|&c| c == '#').count() };
        let is_heading = (1..=6).contains(&level)
            && trimmed[level..].starts_with([' ', '\t']);
        if is_heading {
            if body_seen {
                flush(&mut buf, &stack, start_line, &mut out);
                body_seen = false;
            }
            let title = trimmed[level..].trim().trim_end_matches('#').trim().to_string();
            stack.retain(|(l, _)| *l < level);
            stack.push((level, title));
            if buf.trim().is_empty() {
                start_line = lineno;
            }
        } else if !line.trim().is_empty() {
            body_seen = true;
        }
        if buf.trim().is_empty() && !is_heading {
            start_line = lineno;
        }
        buf.push_str(line);
        buf.push('\n');
    }
    flush(&mut buf, &stack, start_line, &mut out);
    out
}

pub fn plain(path: &Path) -> Result<Vec<Segment>> {
    Ok(vec![Segment {
        text: read(path)?,
        loc: Loc { label: String::new(), line: Some(1), page: None },
    }])
}

/// 行のまとまりごとにセグメント化し、各セグメントの先頭にヘッダ行を付ける
/// (列の意味がチャンク単体でも分かるように)。
pub fn csv(path: &Path) -> Result<Vec<Segment>> {
    const BLOCK_CHARS: usize = 700;
    let src = read(path)?;
    let mut lines = src.lines();
    let Some(header) = lines.next() else { return Ok(vec![]) };
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut first = 2u32;
    let mut cur = 1u32;
    for line in lines {
        cur += 1;
        if line.trim().is_empty() {
            continue;
        }
        if buf.is_empty() {
            first = cur;
        }
        buf.push_str(line);
        buf.push('\n');
        if buf.len() >= BLOCK_CHARS {
            out.push(csv_segment(header, &buf, first, cur));
            buf.clear();
        }
    }
    if !buf.is_empty() {
        out.push(csv_segment(header, &buf, first, cur));
    }
    if out.is_empty() {
        out.push(Segment { text: header.to_string(), loc: Loc { label: "行1".into(), line: Some(1), page: None } });
    }
    Ok(out)
}

fn csv_segment(header: &str, rows: &str, first: u32, last: u32) -> Segment {
    Segment {
        text: format!("{header}\n{rows}"),
        loc: Loc { label: format!("行{first}–{last}"), line: Some(first), page: None },
    }
}

pub fn html(path: &Path) -> Result<Vec<Segment>> {
    let src = read(path)?;
    let text = html2text::from_read(src.as_bytes(), 10_000)
        .map_err(|e| crate::IndexError::Extract(format!("HTML: {e}")))?;
    Ok(vec![Segment::new(text, "")])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_headings_and_fences() {
        let src = "前文\n# A\nこんにちは\n```\n# not heading\n```\n## B\n本文\n# C\n## D\n\nD本文\n";
        let segs = split_markdown(src);
        let got: Vec<_> = segs.iter().map(|s| (s.loc.label.as_str(), s.loc.line.unwrap())).collect();
        // 見出しだけの「C」は「C > D」にまとまる
        assert_eq!(got, [("", 1), ("A", 2), ("A > B", 7), ("C > D", 9)]);
        assert!(segs[1].text.contains("# not heading"));
        assert_eq!(segs[3].text, "# C\n## D\n\nD本文\n");
    }

    #[test]
    fn csv_repeats_header() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.csv");
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("氏名,部署\r\n山田,開発\r\n佐藤,営業\r\n");
        std::fs::write(&p, sjis).unwrap();
        let segs = csv(&p).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "氏名,部署\n山田,開発\n佐藤,営業\n");
        assert_eq!(segs[0].loc.label, "行2–3");
    }

    #[test]
    fn html_strips_tags() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.html");
        std::fs::write(&p, "<html><head><style>x{}</style></head><body><h1>見出し</h1><p>本文です</p></body></html>").unwrap();
        let t = &html(&p).unwrap()[0].text;
        assert!(t.contains("見出し") && t.contains("本文です"));
        assert!(!t.contains("<p>"));
    }
}
