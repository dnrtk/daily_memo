// 検索で見つかった箇所を、前後の文脈ごと表示する(プレビュー / メインの閲覧タブで共用)。
// - テキスト系: ファイルを読み、行番号付きで全体を表示して該当行範囲を強調
// - Word / Excel / PowerPoint / PDF: インデックスの抽出テキストを位置ラベル付きで表示して該当チャンクを強調
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage, isTextKind } from "../api";
import type { ChunkRow } from "../types";
import { makeHighlighter } from "./highlight";

export interface LocationTarget {
  chunkId: number;
  line: number | null;
  lineEnd: number | null;
}

interface Props {
  path: string;
  kind: string;
  target: LocationTarget | null;
  terms: string[];
  /** 同じ場所へ再度スクロールさせたいときに増やす */
  seq?: number;
}

/** 巨大なテキストは該当行の前後だけ描画する */
const TEXT_WINDOW = 2000;

export function LocationView({ path, kind, target, terms, seq = 0 }: Props) {
  const text = isTextKind(kind);
  const [lines, setLines] = useState<string[] | null>(null);
  const [chunks, setChunks] = useState<ChunkRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const focus = useRef<HTMLDivElement>(null);
  const hl = useMemo(() => makeHighlighter(terms), [terms]);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    setLines(null);
    setChunks(null);
    (async () => {
      try {
        if (text) {
          const f = await api.readFile(path);
          if (!cancelled) setLines(f.content.split("\n"));
        } else {
          const c = await api.docChunks(path);
          if (!cancelled) setChunks(c);
        }
      } catch (e) {
        if (!cancelled) setError(errorMessage(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [path, text]);

  // 該当箇所を中央へ(scrollIntoView は外側の要素まで動かすことがあるので自前で計算)
  useLayoutEffect(() => {
    const b = box.current;
    const f = focus.current;
    if (!b) return;
    if (!f) {
      b.scrollTop = 0;
      return;
    }
    b.scrollTop = Math.max(0, f.offsetTop - b.clientHeight / 3);
  }, [lines, chunks, target?.chunkId, target?.line, seq]);

  if (error) return <div className="lv lv-msg error">{error}</div>;

  if (text) {
    if (!lines) return <div className="lv lv-msg">読み込み中…</div>;
    const from = target?.line ?? 0;
    const to = target?.lineEnd ?? from;
    const center = from || 1;
    const start = lines.length > TEXT_WINDOW * 2 ? Math.max(0, center - TEXT_WINDOW) : 0;
    const end = lines.length > TEXT_WINDOW * 2 ? Math.min(lines.length, center + TEXT_WINDOW) : lines.length;
    return (
      <div className="lv" ref={box}>
        {start > 0 && <div className="lv-msg">… {start} 行省略</div>}
        {lines.slice(start, end).map((l, i) => {
          const n = start + i + 1;
          const hit = from > 0 && n >= from && n <= to;
          return (
            <div key={n} className={hit ? "lv-line lv-hit" : "lv-line"} ref={n === from ? focus : undefined}>
              <span className="ln">{n}</span>
              <span className="tx">{l === "" ? "​" : hl(l)}</span>
            </div>
          );
        })}
        {end < lines.length && <div className="lv-msg">… {lines.length - end} 行省略</div>}
      </div>
    );
  }

  if (!chunks) return <div className="lv lv-msg">読み込み中…</div>;
  if (chunks.length === 0)
    return <div className="lv lv-msg">この文書はまだインデックスされていないか、文字情報がありません。</div>;
  return (
    <div className="lv lv-doc" ref={box}>
      {chunks.map((c, i) => {
        const hit = c.id === target?.chunkId;
        const showLabel = i === 0 || c.label !== chunks[i - 1].label;
        return (
          <section key={c.id} className={hit ? "lv-chunk lv-hit" : "lv-chunk"} ref={hit ? focus : undefined}>
            {showLabel && c.label && <div className="lv-label">{c.label}</div>}
            <pre>{hl(c.text)}</pre>
          </section>
        );
      })}
    </div>
  );
}
