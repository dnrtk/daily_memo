// 検索語のハイライトと、一覧用の短いスニペット
import type { ReactNode } from "react";

const escapeRe = (t: string) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** 検索語を <mark> で囲む関数を作る(正規表現は 1 回だけ作る) */
export function makeHighlighter(terms: string[]): (text: string) => ReactNode {
  const ts = [...new Set(terms.filter((t) => t.length > 0))].sort((a, b) => b.length - a.length);
  if (ts.length === 0) return (text) => text;
  const re = new RegExp(`(${ts.map(escapeRe).join("|")})`, "gi");
  const lower = new Set(ts.map((t) => t.toLowerCase()));
  return (text) => {
    const parts = text.split(re);
    if (parts.length === 1) return text;
    return parts.map((p, i) => (lower.has(p.toLowerCase()) ? <mark key={i}>{p}</mark> : p));
  };
}

/** 1 行表示用: 最初の検索語の少し手前から始める */
export function shortSnippet(snippet: string, terms: string[], before = 8): string {
  const s = snippet.replace(/^…/, "");
  const lower = s.toLowerCase();
  const idx = terms
    .map((t) => lower.indexOf(t.toLowerCase()))
    .filter((i) => i >= 0)
    .reduce((a, b) => Math.min(a, b), Infinity);
  if (!Number.isFinite(idx) || idx <= before) return snippet;
  return `…${s.slice(idx - before)}`;
}
