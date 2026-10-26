// 自然言語検索パネル。結果は 1 ファイル 1 行 + 該当箇所 1 行ずつのコンパクト表示。
// シングルクリック → プレビュー、ダブルクリック / Enter → メインウィンドウで開く、↑↓ で移動、Esc でプレビューを閉じる。
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { api, errorMessage } from "../api";
import type { CategoryConfig, FileHit, IndexStatus, MatchedBy, SearchResult } from "../types";
import { makeHighlighter, shortSnippet } from "./highlight";

export interface HitRef {
  path: string;
  index: number;
}

interface Props {
  status: IndexStatus | null;
  categories: CategoryConfig;
  llmDefault: boolean;
  focusKey: number;
  selected: HitRef | null;
  /** null でプレビューを閉じる */
  onSelect(file: FileHit | null, hitIndex: number, terms: string[]): void;
  onOpenHit(file: FileHit, hitIndex: number, terms: string[]): void;
  onError(msg: string): void;
}

export const KIND_LABEL: Record<string, string> = {
  markdown: "MD",
  text: "TXT",
  csv: "CSV",
  html: "HTML",
  docx: "Word",
  excel: "Excel",
  pptx: "PPT",
  pdf: "PDF",
};

const MATCH_LABEL: Record<MatchedBy, string> = {
  keyword: "語で一致",
  semantic: "意味で一致",
  both: "語と意味で一致",
};

function shortDate(sec: number) {
  const d = new Date(sec * 1000);
  const now = new Date();
  const md = `${d.getMonth() + 1}/${d.getDate()}`;
  return d.getFullYear() === now.getFullYear() ? md : `${d.getFullYear()}/${md}`;
}

export function SearchPanel({ status, categories, llmDefault, focusKey, selected, onSelect, onOpenHit, onError }: Props) {
  const [query, setQuery] = useState("");
  const [useLlm, setUseLlm] = useState(llmDefault);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<SearchResult | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const seq = useRef(0);

  useEffect(() => setUseLlm(llmDefault), [llmDefault]);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, [focusKey]);

  const terms = result?.terms ?? [];
  const hl = useMemo(() => makeHighlighter(terms), [terms]);

  /** キーボード移動用に、全ヒットを表示順に並べたもの */
  const flat = useMemo(
    () => (result?.results ?? []).flatMap((f) => f.hits.map((_, i) => ({ file: f, index: i }))),
    [result],
  );

  const run = async (llm = useLlm) => {
    const q = query.trim();
    if (!q) return;
    const my = ++seq.current;
    setBusy(true);
    onSelect(null, 0, []);
    try {
      const r = await api.search(q, llm);
      if (my === seq.current) setResult(r);
    } catch (e) {
      onError(`検索に失敗しました: ${errorMessage(e)}`);
    } finally {
      if (my === seq.current) setBusy(false);
    }
  };

  const isSel = (path: string, i: number) => selected?.path === path && selected.index === i;

  // 選択行が見えるようにスクロール
  useEffect(() => {
    // scrollIntoView は外側の要素までスクロールさせることがあるので、一覧だけを動かす
    const box = list.current;
    const el = box?.querySelector<HTMLElement>(".hit.on");
    if (!box || !el) return;
    const top = el.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
    if (top < box.scrollTop) box.scrollTop = top;
    else if (top + el.offsetHeight > box.scrollTop + box.clientHeight) box.scrollTop = top + el.offsetHeight - box.clientHeight;
  }, [selected]);

  const moveSelection = (delta: number) => {
    if (flat.length === 0) return;
    const cur = flat.findIndex((h) => selected && h.file.path === selected.path && h.index === selected.index);
    const next = cur < 0 ? (delta > 0 ? 0 : flat.length - 1) : Math.min(flat.length - 1, Math.max(0, cur + delta));
    onSelect(flat[next].file, flat[next].index, terms);
  };

  const onListKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      moveSelection(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.key === "Enter" && selected) {
      e.preventDefault();
      const f = result?.results.find((r) => r.path === selected.path);
      if (f) onOpenHit(f, selected.index, terms);
    } else if (e.key === "Escape") {
      e.preventDefault();
      onSelect(null, 0, []);
    }
  };

  const catName = (id: string) => categories.categories.find((c) => c.id === id)?.name ?? id;

  return (
    <div className="search">
      <form
        className="search-box"
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <input
          ref={input}
          value={query}
          placeholder="例: 先月の見積もり関係のパワポ"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" && flat.length > 0) {
              e.preventDefault();
              list.current?.focus();
              moveSelection(1);
            }
          }}
        />
        <label className="check small" title="検索文を LLM で期間・種類・カテゴリの条件に分解します">
          <input type="checkbox" checked={useLlm} onChange={(e) => setUseLlm(e.target.checked)} />
          LLM で解釈
        </label>
      </form>

      {busy && <div className="search-busy">{useLlm ? "解釈して検索中…" : "検索中…"}</div>}

      {result && !busy && (
        <div className="plan">
          {result.plan.by_llm && (
            <div className="chips">
              {result.plan.search_text !== result.plan.query && <span className="chip tag">「{result.plan.search_text}」</span>}
              {result.plan.keywords.map((k) => (
                <span key={`k-${k}`} className="chip tag">語: {k}</span>
              ))}
              {result.plan.file_kinds.map((k) => (
                <span key={`f-${k}`} className="chip tag">{KIND_LABEL[k] ?? k}</span>
              ))}
              {(result.plan.date_from || result.plan.date_to) && (
                <span className="chip tag">
                  {result.plan.date_from ?? ""}〜{result.plan.date_to ?? ""}
                </span>
              )}
              {result.plan.categories.map((c) => (
                <span key={`c-${c}`} className="chip tag">{catName(c)}</span>
              ))}
            </div>
          )}
          <div className="plan-meta">
            {result.results.length} 件 ・ {(result.elapsed_ms / 1000).toFixed(1)} 秒
            {!result.used_vector && " ・ 全文検索のみ"}
          </div>
          {result.plan.note && <div className="plan-note">{result.plan.note}</div>}
          {result.vector_note && <div className="plan-note">{result.vector_note}</div>}
        </div>
      )}

      <div className="results" ref={list} tabIndex={-1} onKeyDown={onListKey}>
        {result?.results.map((f) => {
          const dir = f.path.split("/").slice(0, -1).join("/");
          return (
            <div key={f.path} className="result">
              <div
                className="result-head"
                title={f.path}
                onClick={() => onSelect(f, 0, terms)}
                onDoubleClick={() => onOpenHit(f, 0, terms)}
              >
                <span className={`kind kind-${f.kind}`}>{KIND_LABEL[f.kind] ?? f.kind}</span>
                <span className="result-name">{f.name}</span>
                {dir && <span className="result-dir">{dir}</span>}
                <span className="result-date">{shortDate(f.modified)}</span>
              </div>
              {f.hits.map((h, i) => (
                <div
                  key={i}
                  className={isSel(f.path, i) ? "hit on" : "hit"}
                  onClick={() => {
                    list.current?.focus();
                    onSelect(f, i, terms);
                  }}
                  onDoubleClick={() => onOpenHit(f, i, terms)}
                >
                  <i className={`by by-${h.matched_by}`} title={MATCH_LABEL[h.matched_by]} />
                  <span className="hit-loc">{h.label || (h.line ? `${h.line}行` : "本文")}</span>
                  <span className="hit-snippet">{hl(shortSnippet(h.snippet, terms))}</span>
                </div>
              ))}
            </div>
          );
        })}
        {result && !busy && result.results.length === 0 && (
          <div className="empty">
            <p>見つかりませんでした</p>
            {hasFilters(result) && (
              <button onClick={() => void run(false)}>条件を外して検索する</button>
            )}
          </div>
        )}
        {result && result.results.length > 0 && (
          <p className="results-hint">クリックでプレビュー ・ ダブルクリック / Enter で開く</p>
        )}
      </div>

      <IndexFooter status={status} />
    </div>
  );
}

function hasFilters(r: SearchResult) {
  const p = r.plan;
  return p.by_llm && (p.file_kinds.length > 0 || p.categories.length > 0 || !!p.date_from || !!p.date_to);
}

function IndexFooter({ status }: { status: IndexStatus | null }) {
  const [open, setOpen] = useState(false);
  if (!status || !status.root) return null;
  const { progress: p, stats, vector } = status;
  const busy = p.phase !== "idle";
  const phaseLabel = { idle: "", scanning: "フォルダを走査中", extracting: "文書を読み取り中", embedding: "ベクトル作成中" }[p.phase];
  const pct = p.total > 0 ? Math.round((p.done / p.total) * 100) : 0;

  return (
    <footer className="index-foot">
      {busy && (
        <div className="index-progress" title={p.current ?? ""}>
          <div className="bar">
            <div style={{ width: `${pct}%` }} />
          </div>
          <span>
            {phaseLabel} {p.total > 0 && `${p.done}/${p.total}`}
          </span>
        </div>
      )}
      <div className="index-line">
        <span>
          {stats.files} ファイル
          {stats.pending_embeddings > 0 && ` ・ ベクトル待ち ${stats.pending_embeddings}`}
          {stats.errors > 0 && (
            <button className="link" onClick={() => setOpen((o) => !o)}>
              {" "}・ 読めない {stats.errors}
            </button>
          )}
        </span>
        <span className="spacer" />
        <button className="icon" title="再スキャン" disabled={busy} onClick={() => void api.indexRescan()}>
          ↻
        </button>
      </div>
      {vector.state === "model_mismatch" && (
        <div className="index-warn">
          埋め込みモデルが変わりました。意味検索を使うにはベクトルを作り直してください。
          <button onClick={() => void api.indexRebuildVectors()}>作り直す</button>
        </div>
      )}
      {vector.state === "unavailable" && (
        <div className="index-warn" title={vector.message}>
          埋め込みサーバーに接続できません(全文検索は使えます)。1 分ごとに再試行します。
        </div>
      )}
      {status.pdf_note && <div className="index-warn">{status.pdf_note}</div>}
      {status.last_error && <div className="index-warn">{status.last_error}</div>}
      {open && (
        <ul className="failures">
          {status.failures.map(([path, err]) => (
            <li key={path} title={err}>
              {path}: {err}
            </li>
          ))}
        </ul>
      )}
    </footer>
  );
}
