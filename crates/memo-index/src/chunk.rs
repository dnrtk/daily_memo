//! セグメントを検索単位(チャンク)に分割する。
//! 行の境目で切り、テキストファイルなら各チャンクの開始行を保つ。

use crate::extract::{Loc, Segment};

/// 1 チャンクの最大文字数。bge-m3 / ruri の入力上限に十分収まり、
/// 検索結果の粒度として「段落〜節」くらいになる大きさ。
pub const MAX_CHARS: usize = 800;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub ord: u32,
    pub text: String,
    pub loc: Loc,
}

pub fn chunk(segments: Vec<Segment>) -> Vec<Chunk> {
    let mut out = Vec::new();
    for seg in segments {
        let mut buf = String::new();
        let mut buf_chars = 0usize;
        let mut buf_start_line = 0usize;
        let push = |out: &mut Vec<Chunk>, buf: &mut String, start_line: usize, loc: &Loc| {
            if buf.trim().is_empty() {
                buf.clear();
                return;
            }
            out.push(Chunk {
                ord: out.len() as u32,
                text: std::mem::take(buf).trim_end().to_string(),
                loc: Loc {
                    label: loc.label.clone(),
                    line: loc.line.map(|l| l + start_line as u32),
                    page: loc.page,
                },
            });
        };
        for (i, line) in seg.text.lines().enumerate() {
            let n = line.chars().count();
            if buf_chars + n + 1 > MAX_CHARS && buf_chars > 0 {
                push(&mut out, &mut buf, buf_start_line, &seg.loc);
                buf_chars = 0;
            }
            if buf_chars == 0 {
                buf_start_line = i;
            }
            if n > MAX_CHARS {
                // 1 行が長すぎる場合(改行のない PDF など)は文字数で切る
                let chars: Vec<char> = line.chars().collect();
                for piece in chars.chunks(MAX_CHARS) {
                    buf.extend(piece);
                    push(&mut out, &mut buf, i, &seg.loc);
                }
                buf_chars = 0;
                continue;
            }
            buf.push_str(line);
            buf.push('\n');
            buf_chars += n + 1;
        }
        push(&mut out, &mut buf, buf_start_line, &seg.loc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, line: Option<u32>) -> Segment {
        Segment { text: text.into(), loc: Loc { label: "L".into(), line, page: None } }
    }

    #[test]
    fn small_segment_is_one_chunk() {
        let c = chunk(vec![seg("a\nb\n", Some(10))]);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "a\nb");
        assert_eq!(c[0].loc.line, Some(10));
    }

    #[test]
    fn splits_on_lines_and_tracks_line_numbers() {
        let line = "あ".repeat(300);
        let text = format!("{line}\n{line}\n{line}\n{line}\n");
        let c = chunk(vec![seg(&text, Some(5))]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].loc.line, Some(5));
        assert_eq!(c[1].loc.line, Some(7));
        assert!(c.iter().all(|c| c.text.chars().count() <= MAX_CHARS));
        assert_eq!(c.iter().map(|c| c.ord).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn hard_splits_long_line() {
        let c = chunk(vec![seg(&"x".repeat(2000), None)]);
        assert_eq!(c.iter().map(|c| c.text.len()).collect::<Vec<_>>(), [800, 800, 400]);
    }
}
