//! `{yyyy}/{MM}/{dd}` などのプレースホルダ展開。
//!
//! | 記法 | 意味 |
//! |---|---|
//! | `{yyyy}` `{MM}` `{dd}` | 年・月・日(ゼロ埋め) |
//! | `{HH}` `{mm}` `{ss}` | 時・分・秒 |
//! | `{slug}` | タイトルをファイル名向けに整形したもの |
//! | `{title}` | タイトルそのまま |
//! | `{content}` | 本文(追記エントリ用) |

use chrono::{Datelike, NaiveDateTime, Timelike};

pub struct Vars<'a> {
    pub now: NaiveDateTime,
    pub title: &'a str,
    pub content: &'a str,
}

pub fn render(pattern: &str, v: &Vars) -> String {
    pattern
        .replace("{yyyy}", &format!("{:04}", v.now.year()))
        .replace("{MM}", &format!("{:02}", v.now.month()))
        .replace("{dd}", &format!("{:02}", v.now.day()))
        .replace("{HH}", &format!("{:02}", v.now.hour()))
        .replace("{mm}", &format!("{:02}", v.now.minute()))
        .replace("{ss}", &format!("{:02}", v.now.second()))
        .replace("{slug}", &slugify(v.title))
        .replace("{title}", v.title)
        .replace("{content}", v.content)
}

/// ファイル名に使えない文字を `-` に置換。日本語はそのまま残す。
pub fn slugify(title: &str) -> String {
    const MAX_CHARS: usize = 40;
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in title.trim().chars() {
        let bad = ch.is_whitespace()
            || ch.is_control()
            || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '.');
        if bad {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(ch);
            prev_dash = false;
        }
        if out.chars().count() >= MAX_CHARS {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "memo".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn t() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 6)
            .unwrap()
            .and_hms_opt(9, 5, 3)
            .unwrap()
    }

    #[test]
    fn renders_date_parts() {
        let v = Vars { now: t(), title: "Ollama 接続: 手順", content: "" };
        assert_eq!(
            render("{yyyy}/{MM}/{yyyy}-{MM}-{dd}.md", &v),
            "2026/10/2026-10-06.md"
        );
        assert_eq!(render("{HH}:{mm}:{ss}", &v), "09:05:03");
        assert_eq!(render("{yyyy}{MM}{dd}-{slug}.md", &v), "20261006-Ollama-接続-手順.md");
    }

    #[test]
    fn slug_fallback() {
        assert_eq!(slugify("  ///  "), "memo");
        assert_eq!(slugify("a/b\\c"), "a-b-c");
    }
}
