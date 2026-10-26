//! 自然言語検索。
//!
//! 1. (任意)LLM が検索文を「本題 / 完全一致させたい語 / ファイル種別 / 期間 / カテゴリ」に分解
//! 2. 全文検索(バイグラム BM25)とベクトル検索(コサイン)をそれぞれ実行
//! 3. 条件で絞り込み、Reciprocal Rank Fusion で統合
//! 4. ファイル単位にまとめ、各ファイルの該当箇所(上位 3 件)とスニペットを返す
//!
//! LLM・埋め込みサーバーのどちらが落ちていても、使える手段だけで結果を返す。

use std::collections::HashMap;

use chrono::{Local, NaiveDate, TimeZone};
use serde::Serialize;
use serde_json::Value;

use crate::{
    embed::Embedder,
    indexer::{vector_state, VectorState},
    store::{ChunkRow, Store},
    tokenize, Result,
};

pub const FILE_KINDS: &[&str] = &["markdown", "text", "csv", "html", "docx", "excel", "pptx", "pdf"];

/// RRF の定数(一般的な値)
const RRF_K: f64 = 60.0;
/// 各手段から取る候補数
const CANDIDATES: usize = 200;
/// これより遠いベクトル候補は捨てる(コサイン距離 = 1 - 類似度)
const MAX_VECTOR_DISTANCE: f64 = 0.75;

#[derive(Debug, Clone)]
pub struct CategoryInfo {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct QueryPlan {
    /// 入力そのまま
    pub query: String,
    /// 意味検索に使う本題
    pub search_text: String,
    /// 完全一致させたい語
    pub keywords: Vec<String>,
    pub file_kinds: Vec<String>,
    /// YYYY-MM-DD(更新日で絞る)
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// カテゴリ ID と、そのフォルダ
    pub categories: Vec<String>,
    pub category_dirs: Vec<String>,
    pub by_llm: bool,
    /// LLM 解釈に失敗した理由など
    pub note: Option<String>,
}

/// LLM を使わない素の解釈
pub fn plain_plan(query: &str) -> QueryPlan {
    QueryPlan {
        query: query.to_string(),
        search_text: query.trim().to_string(),
        keywords: vec![],
        ..Default::default()
    }
}

fn system_prompt(today: NaiveDate, cats: &[CategoryInfo]) -> String {
    let cat_lines: String = cats
        .iter()
        .map(|c| format!("- {}: {}({})\n", c.id, c.name, c.description))
        .collect();
    format!(
        r#"あなたはメモ・業務文書のファイル検索のためのクエリ解析器です。
ユーザーの検索文を解析し、次の形の JSON オブジェクトだけを返してください。説明文は不要です。

{{
  "search_text": "探している内容そのもの。期間・ファイル種類・カテゴリの指定語は取り除く",
  "keywords": ["完全一致で探すべき固有名詞・型番・製品名・専門用語(最大5個、なければ空配列)"],
  "file_types": ["次から該当するものだけ: markdown, text, csv, html, docx, excel, pptx, pdf"],
  "date_from": "YYYY-MM-DD または null",
  "date_to": "YYYY-MM-DD または null",
  "categories": ["次のカテゴリ ID から、検索文が明示的に指しているものだけ"]
}}

ファイル種類の言い換え:
- pptx: PowerPoint, パワポ, PPT, スライド, プレゼン資料
- excel: Excel, エクセル, xlsx, 表計算, スプレッドシート
- docx: Word, ワード, 文書ファイル
- pdf: PDF
- markdown: メモ, md, ノート

規則:
- 今日は {today} です。「先月」「去年」「今週」「10月」などの期間の言及があるときだけ date_from / date_to を入れる。
- 指定がないものは空配列または null にする。推測で条件を足さない。

カテゴリ:
{cat_lines}"#
    )
}

/// LLM で検索文を解釈する。失敗したら素の解釈に理由を添えて返す。
pub fn llm_plan(
    query: &str,
    today: NaiveDate,
    cats: &[CategoryInfo],
    chat: impl FnOnce(&str, &str) -> std::result::Result<Value, String>,
) -> QueryPlan {
    let v = match chat(&system_prompt(today, cats), query) {
        Ok(v) => v,
        Err(e) => {
            let mut p = plain_plan(query);
            p.note = Some(format!("LLM でのクエリ解釈をスキップしました: {e}"));
            return p;
        }
    };
    let strings = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let date = |key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
            .map(|d| d.format("%Y-%m-%d").to_string())
    };
    let search_text = v
        .get("search_text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(query.trim())
        .to_string();
    let file_kinds: Vec<String> = strings("file_types")
        .into_iter()
        .map(|s| s.to_lowercase())
        .filter(|s| FILE_KINDS.contains(&s.as_str()))
        .collect();
    let categories: Vec<String> = strings("categories")
        .into_iter()
        .filter(|id| cats.iter().any(|c| &c.id == id))
        .collect();
    let category_dirs = categories
        .iter()
        .filter_map(|id| cats.iter().find(|c| &c.id == id))
        .map(|c| c.dir.trim_matches('/').to_string())
        .collect();
    let mut keywords = strings("keywords");
    keywords.truncate(5);
    QueryPlan {
        query: query.to_string(),
        search_text,
        keywords,
        file_kinds,
        date_from: date("date_from"),
        date_to: date("date_to"),
        categories,
        category_dirs,
        by_llm: true,
        note: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    Keyword,
    Semantic,
    Both,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkHit {
    pub chunk_id: i64,
    pub label: String,
    pub line: Option<u32>,
    /// チャンクの最終行(テキストファイルのみ。プレビューの強調範囲に使う)
    pub line_end: Option<u32>,
    pub page: Option<u32>,
    pub snippet: String,
    pub score: f64,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileHit {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub modified: i64,
    pub score: f64,
    pub hits: Vec<ChunkHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub plan: QueryPlan,
    pub results: Vec<FileHit>,
    /// ハイライト用の語
    pub terms: Vec<String>,
    pub used_vector: bool,
    pub vector_note: Option<String>,
}

fn day_bounds(plan: &QueryPlan) -> (Option<i64>, Option<i64>) {
    let ts = |s: &Option<String>, end: bool| {
        s.as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .and_then(|d| if end { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) })
            .and_then(|dt| Local.from_local_datetime(&dt).earliest())
            .map(|dt| dt.timestamp())
    };
    (ts(&plan.date_from, false), ts(&plan.date_to, true))
}

fn passes(row: &ChunkRow, plan: &QueryPlan, from: Option<i64>, to: Option<i64>) -> bool {
    (plan.file_kinds.is_empty() || plan.file_kinds.contains(&row.kind))
        && from.is_none_or(|f| row.mtime >= f)
        && to.is_none_or(|t| row.mtime <= t)
        && (plan.category_dirs.is_empty()
            || plan.category_dirs.iter().any(|d| row.path.starts_with(&format!("{d}/"))))
}

pub fn search(store: &Store, plan: QueryPlan, embedder: Option<&dyn Embedder>, limit: usize) -> Result<SearchResponse> {
    let main_text = if plan.search_text.is_empty() { plan.query.clone() } else { plan.search_text.clone() };
    let mut terms: Vec<String> = plan.keywords.clone();
    for t in tokenize::display_terms(&main_text) {
        if !terms.contains(&t) {
            terms.push(t);
        }
    }

    // 全文検索
    let fts_ids = match tokenize::query_expr(&terms) {
        Some(expr) => store.fts_search(&expr, CANDIDATES).unwrap_or_default(),
        None => vec![],
    };

    // ベクトル検索
    let mut vector_note = None;
    let mut vec_ids: Vec<i64> = vec![];
    let mut used_vector = false;
    match embedder {
        None => vector_note = Some("埋め込みモデルが設定されていません".to_string()),
        Some(e) => match vector_state(store, e)? {
            VectorState::Ready if store.vec_dim()?.is_some() => match e.embed_query(&main_text) {
                Ok(q) => {
                    used_vector = true;
                    vec_ids = store
                        .vec_search(&q, CANDIDATES)?
                        .into_iter()
                        .filter(|(_, d)| *d <= MAX_VECTOR_DISTANCE)
                        .map(|(id, _)| id)
                        .collect();
                }
                Err(err) => vector_note = Some(format!("意味検索をスキップしました: {err}")),
            },
            VectorState::Ready => vector_note = Some("ベクトルがまだ作成されていません".into()),
            VectorState::ModelMismatch { .. } => {
                vector_note = Some("埋め込みモデルが変わったため、ベクトルの作り直しが必要です".into())
            }
            VectorState::Unavailable { message } => vector_note = Some(message),
        },
    }

    // 絞り込みと RRF
    let mut all: Vec<i64> = fts_ids.clone();
    all.extend(vec_ids.iter().filter(|id| !fts_ids.contains(id)));
    let rows = store.chunk_rows(&all)?;
    let (from, to) = day_bounds(&plan);
    let keep = |ids: &[i64]| -> Vec<i64> {
        ids.iter()
            .copied()
            .filter(|id| rows.get(id).is_some_and(|r| passes(r, &plan, from, to)))
            .collect()
    };
    let fts_ranked = keep(&fts_ids);
    let vec_ranked = keep(&vec_ids);

    let mut scores: HashMap<i64, (f64, bool, bool)> = HashMap::new();
    for (rank, id) in fts_ranked.iter().enumerate() {
        let e = scores.entry(*id).or_default();
        e.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
        e.1 = true;
    }
    for (rank, id) in vec_ranked.iter().enumerate() {
        let e = scores.entry(*id).or_default();
        e.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
        e.2 = true;
    }

    // ファイル単位に集約
    let mut by_file: HashMap<String, Vec<(f64, MatchedBy, &ChunkRow)>> = HashMap::new();
    for (id, (score, kw, sem)) in &scores {
        let row = &rows[id];
        let by = match (kw, sem) {
            (true, true) => MatchedBy::Both,
            (true, false) => MatchedBy::Keyword,
            _ => MatchedBy::Semantic,
        };
        by_file.entry(row.path.clone()).or_default().push((*score, by, row));
    }
    let mut results: Vec<FileHit> = by_file
        .into_iter()
        .map(|(path, mut hits)| {
            hits.sort_by(|a, b| b.0.total_cmp(&a.0));
            let score = hits.iter().take(3).zip([1.0, 0.5, 0.25]).map(|((s, _, _), w)| s * w).sum();
            let first = hits[0].2;
            FileHit {
                name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                kind: first.kind.clone(),
                modified: first.mtime,
                score,
                hits: hits
                    .iter()
                    .take(3)
                    .map(|(s, by, r)| ChunkHit {
                        chunk_id: r.id,
                        label: r.label.clone(),
                        line: r.line,
                        line_end: r.line.map(|l| l + r.text.lines().count().saturating_sub(1) as u32),
                        page: r.page,
                        snippet: snippet(&r.text, &terms),
                        score: *s,
                        matched_by: *by,
                    })
                    .collect(),
                path,
            }
        })
        .collect();
    // 同点なら「語で一致した箇所がある」ファイルを優先し、最後はパスで順序を固定する
    let has_kw = |f: &FileHit| f.hits.iter().any(|h| h.matched_by != MatchedBy::Semantic);
    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| has_kw(b).cmp(&has_kw(a)))
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.path.cmp(&b.path))
    });
    results.truncate(limit);

    Ok(SearchResponse { plan, results, terms, used_vector, vector_note })
}

/// 最初に見つかった検索語の周辺を切り出す。見つからなければ先頭。
pub fn snippet(text: &str, terms: &[String]) -> String {
    const BEFORE: usize = 40;
    const LEN: usize = 160;
    let flat: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let lower: Vec<char> = flat.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let pos = terms
        .iter()
        .filter_map(|t| {
            let t: Vec<char> = t.chars().flat_map(char::to_lowercase).collect();
            if t.is_empty() || t.len() > lower.len() {
                return None;
            }
            lower.windows(t.len()).position(|w| w == t.as_slice())
        })
        .min()
        .unwrap_or(0);
    let start = pos.saturating_sub(BEFORE);
    let end = (start + LEN).min(flat.len());
    let mut s: String = flat[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < flat.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        embed::HashEmbedder,
        indexer::{embed_pending, full_scan, IndexOptions},
    };
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    fn cats() -> Vec<CategoryInfo> {
        vec![
            CategoryInfo { id: "tech".into(), name: "技術".into(), dir: "技術".into(), description: "知見".into() },
            CategoryInfo { id: "research".into(), name: "調査".into(), dir: "調査".into(), description: "調べた".into() },
        ]
    }

    fn indexed() -> (tempfile::TempDir, Store, HashEmbedder) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("技術")).unwrap();
        std::fs::create_dir_all(p.join("調査")).unwrap();
        std::fs::write(p.join("技術/ollama.md"), "# Ollama 運用\n\nモデルの保存先は OLLAMA_MODELS 環境変数で変更する。\n").unwrap();
        std::fs::write(p.join("技術/rust.md"), "# Rust\n\n所有権と借用の基本。\n").unwrap();
        std::fs::write(p.join("調査/vectordb.md"), "# ベクトルDB比較\n\nsqlite-vec と Qdrant を比較した議事録。\n").unwrap();
        std::fs::copy(crate::extract::fixture("overview.pptx"), p.join("調査/overview.pptx")).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        full_scan(p, &mut store, &IndexOptions::default(), &cancel, &|_| {}).unwrap();
        let e = HashEmbedder::new(64);
        embed_pending(&mut store, &e, &cancel, &|_| {}).unwrap();
        (dir, store, e)
    }

    #[test]
    fn keyword_hit_with_snippet_and_location() {
        let (_d, store, e) = indexed();
        let r = search(&store, plain_plan("OLLAMA_MODELS"), Some(&e), 10).unwrap();
        assert_eq!(r.results[0].path, "技術/ollama.md");
        let hit = &r.results[0].hits[0];
        assert_eq!(hit.label, "Ollama 運用");
        assert_eq!(hit.line, Some(1));
        assert_eq!(hit.line_end, Some(3));
        assert!(hit.snippet.contains("OLLAMA_MODELS"));
        assert!(r.used_vector);
    }

    #[test]
    fn japanese_substring_and_pptx_location() {
        let (_d, store, e) = indexed();
        let r = search(&store, plain_plan("ハイブリッド検索"), Some(&e), 10).unwrap();
        let top = &r.results[0];
        assert_eq!(top.path, "調査/overview.pptx");
        assert_eq!(top.hits[0].page, Some(1));
        assert!(matches!(top.hits[0].matched_by, MatchedBy::Keyword | MatchedBy::Both));
    }

    #[test]
    fn filters_from_plan() {
        let (_d, store, e) = indexed();
        let mut plan = plain_plan("比較");
        plan.file_kinds = vec!["pptx".into()];
        assert!(search(&store, plan, Some(&e), 10).unwrap().results.iter().all(|f| f.kind == "pptx"));

        let mut plan = plain_plan("運用 比較 基本");
        plan.category_dirs = vec!["技術".into()];
        let r = search(&store, plan, Some(&e), 10).unwrap();
        assert!(!r.results.is_empty());
        assert!(r.results.iter().all(|f| f.path.starts_with("技術/")));

        let mut plan = plain_plan("比較");
        plan.date_to = Some("2000-01-01".into());
        assert!(search(&store, plan, Some(&e), 10).unwrap().results.is_empty());
    }

    #[test]
    fn works_without_embedder() {
        let (_d, store, _) = indexed();
        let r = search(&store, plain_plan("議事録"), None, 10).unwrap();
        assert_eq!(r.results[0].path, "調査/vectordb.md");
        assert!(!r.used_vector);
        assert!(r.vector_note.is_some());
    }

    #[test]
    fn llm_plan_parses_and_validates() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let p = llm_plan("先月の見積もり関係のパワポ", today, &cats(), |sys, user| {
            assert!(sys.contains("2026-10-26") && sys.contains("tech: 技術"));
            assert_eq!(user, "先月の見積もり関係のパワポ");
            Ok(json!({
                "search_text": "見積もり",
                "keywords": ["見積"],
                "file_types": ["pptx", "unknown"],
                "date_from": "2026-09-01", "date_to": "2026-09-30",
                "categories": ["research", "nope"]
            }))
        });
        assert!(p.by_llm);
        assert_eq!(p.search_text, "見積もり");
        assert_eq!(p.file_kinds, ["pptx"]);
        assert_eq!(p.date_from.as_deref(), Some("2026-09-01"));
        assert_eq!(p.categories, ["research"]);
        assert_eq!(p.category_dirs, ["調査"]);

        let p = llm_plan("x", today, &cats(), |_, _| Err("timeout".into()));
        assert!(!p.by_llm);
        assert!(p.note.unwrap().contains("timeout"));
    }

    #[test]
    fn snippet_window() {
        let text = format!("{}目印{}", "あ".repeat(100), "い".repeat(300));
        let s = snippet(&text, &["目印".into()]);
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("目印"));
        assert_eq!(snippet("short", &["x".into()]), "short");
    }
}
