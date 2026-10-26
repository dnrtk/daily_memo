//! 全文検索用のトークン化(辞書なし)。
//!
//! - 日本語(漢字・かな)の連続部分 → 文字バイグラム(「議事録」→「議事」「事録」)
//! - 英数字の連続部分 → 1 単語
//! - それ以外 → 区切り
//!
//! 正規化は NFKC + 小文字化(全角英数・半角カナを揃える)。
//! 生成したトークンを空白区切りで FTS5(unicode61)に入れるので、SQLite 側は分割しない。
//! 検索時は同じ規則でバイグラム化し、連続一致(フレーズ)で問い合わせるため
//! 「部分文字列として含まれている」ことが保証される。

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Class {
    Cjk,
    Word,
    Sep,
}

fn class(c: char) -> Class {
    let cp = c as u32;
    let cjk = matches!(cp,
        0x3040..=0x30FF   // ひらがな・カタカナ(長音符含む)
        | 0x31F0..=0x31FF // カタカナ拡張
        | 0x3400..=0x4DBF // CJK 拡張 A
        | 0x4E00..=0x9FFF // CJK 統合漢字
        | 0xF900..=0xFAFF // CJK 互換漢字
        | 0x20000..=0x2FFFF
        | 0x3005 | 0x3006 // 々 〆
    );
    if cjk {
        Class::Cjk
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Sep
    }
}

fn normalize(s: &str) -> String {
    s.nfkc().flat_map(char::to_lowercase).collect()
}

/// 正規化済みで区切り文字を含まない語を、(種類, 文字列) の連続部分に分ける
fn runs(piece: &str) -> Vec<(Class, Vec<char>)> {
    let mut out: Vec<(Class, Vec<char>)> = Vec::new();
    for c in piece.chars() {
        let k = class(c);
        match out.last_mut() {
            Some((last, buf)) if *last == k => buf.push(c),
            _ => out.push((k, vec![c])),
        }
    }
    out
}

fn tokens_of(s: &str) -> Vec<Vec<String>> {
    // 区切りで分けた「語」ごとに、その中のトークン列を返す
    let norm = normalize(s);
    let mut words: Vec<Vec<String>> = Vec::new();
    for piece in norm.split(|c: char| class(c) == Class::Sep) {
        if piece.is_empty() {
            continue;
        }
        let mut toks = Vec::new();
        for (k, chars) in runs(piece) {
            match k {
                Class::Word => toks.push(chars.iter().collect()),
                Class::Cjk if chars.len() == 1 => toks.push(chars[0].to_string()),
                Class::Cjk => toks.extend(chars.windows(2).map(|w| w.iter().collect::<String>())),
                Class::Sep => {}
            }
        }
        if !toks.is_empty() {
            words.push(toks);
        }
    }
    words
}

/// インデックス用: 空白区切りのトークン列
pub fn index_tokens(text: &str) -> String {
    tokens_of(text).into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// 検索用: FTS5 の MATCH 式。各語をフレーズにして OR で結ぶ。語がなければ None。
pub fn query_expr<S: AsRef<str>>(terms: &[S]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for term in terms {
        for toks in tokens_of(term.as_ref()) {
            let p = if toks.len() == 1 {
                let t = &toks[0];
                // 1 文字の漢字・かなはバイグラムの先頭一致、英数字は前方一致
                format!("\"{t}\"*")
            } else {
                format!("\"{}\"", toks.join(" "))
            };
            if !parts.contains(&p) {
                parts.push(p);
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" OR "))
}

fn is_hiragana(c: char) -> bool {
    ('\u{3041}'..='\u{309F}').contains(&c)
}

/// 自然文の検索語を、検索に使う語に分ける。
///
/// 日本語の文は空白で区切られないので、そのままだと文全体が 1 つのフレーズになり一致しない。
/// 漢字・カタカナ・英数字を含む部分はひらがな(助詞・送り仮名)の位置で切って語を取り出す。
/// 「推論サーバーが止まった件の再発防止」→「推論サーバー」「止」「件」「再発防止」→ 1 文字語は捨てる。
/// ひらがなだけの語(「まとめる」など)はそのまま使う。
pub fn query_terms(s: &str) -> Vec<String> {
    let norm: String = s.nfkc().collect();
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: String| {
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    };
    for piece in norm.split(|c: char| class(c) == Class::Sep).filter(|p| !p.is_empty()) {
        if piece.chars().all(is_hiragana) {
            push(piece.to_string());
            continue;
        }
        let words: Vec<String> = piece
            .split(is_hiragana)
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect();
        let long: Vec<String> = words.iter().filter(|w| w.chars().count() >= 2).cloned().collect();
        // 2 文字以上の語が無ければ 1 文字語も使う(「件」だけの検索など)
        for w in if long.is_empty() { words } else { long } {
            push(w);
        }
    }
    out
}

/// スニペット・ハイライト用に、検索語を表示用の語に分ける
pub fn display_terms(s: &str) -> Vec<String> {
    query_terms(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigrams_and_words() {
        assert_eq!(index_tokens("会議議事録 GPU設定、Ｖｌｌｍ"), "会議 議議 議事 事録 gpu 設定 vllm");
        assert_eq!(index_tokens("ｶﾀｶﾅ と 々"), "カタ タカ カナ と 々");
    }

    #[test]
    fn query_is_phrase_per_word() {
        assert_eq!(query_expr(&["議事録 vllm"]).unwrap(), "\"議事 事録\" OR \"vllm\"*");
        assert_eq!(query_expr(&["見積もり"]).unwrap(), "\"見積 積も もり\"");
        assert_eq!(query_expr(&["", "、"]), None);
    }

    #[test]
    fn query_terms_split_sentences() {
        assert_eq!(query_terms("推論サーバーが止まった件の再発防止"), ["推論サーバー", "再発防止"]);
        assert_eq!(query_terms("先月の 見積もり PPT"), ["先月", "見積", "PPT"]);
        assert_eq!(query_terms("まとめる 件"), ["まとめる", "件"]);
        assert_eq!(query_terms("ＧＰＵサーバーの費用"), ["GPUサーバー", "費用"]);
    }
}
