// 検索結果のプレビュー。ヘッダーをドラッグで移動、端と角をドラッグでサイズ変更。位置とサイズは記憶する。
import { useEffect, useRef, useState, type PointerEvent as RPointerEvent } from "react";
import type { FileHit } from "../types";
import { LocationView } from "./LocationView";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Props {
  file: FileHit;
  index: number;
  terms: string[];
  /** 初回表示時の左端(サイドバーの右隣) */
  defaultLeft: number;
  onOpen(): void;
  onClose(): void;
}

const KEY = "preview.rect";
const MIN_W = 320;
const MIN_H = 180;

function loadRect(defaultLeft: number): Rect {
  try {
    const r = JSON.parse(localStorage.getItem(KEY) ?? "null") as Rect | null;
    if (r && [r.x, r.y, r.w, r.h].every(Number.isFinite)) return r;
  } catch {
    /* storage 不可でも動かす */
  }
  return { x: defaultLeft, y: 48, w: Math.min(640, window.innerWidth - defaultLeft - 24), h: Math.min(460, window.innerHeight - 96) };
}

/** 画面外に出ないように補正 */
function clamp(r: Rect): Rect {
  const w = Math.min(Math.max(r.w, MIN_W), window.innerWidth - 16);
  const h = Math.min(Math.max(r.h, MIN_H), window.innerHeight - 16);
  const x = Math.min(Math.max(r.x, 8), window.innerWidth - w - 8);
  const y = Math.min(Math.max(r.y, 8), window.innerHeight - h - 8);
  return { x, y, w, h };
}

type Edge = "move" | "e" | "w" | "s" | "se" | "sw";

export function PreviewPopup({ file, index, terms, defaultLeft, onOpen, onClose }: Props) {
  const [rect, setRect] = useState<Rect>(() => clamp(loadRect(defaultLeft)));
  const drag = useRef<{ edge: Edge; sx: number; sy: number; start: Rect } | null>(null);
  const hit = file.hits[index];

  // 位置・サイズを記憶(ドラッグ終了時の state はまだ反映前のことがあるので、変化のたびに保存)
  useEffect(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify(rect));
    } catch {
      /* storage 不可でも動かす */
    }
  }, [rect]);

  useEffect(() => {
    const onResize = () => setRect((r) => clamp(r));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const begin = (edge: Edge) => (e: RPointerEvent) => {
    if (e.button !== 0) return;
    if (edge === "move" && (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    window.getSelection()?.removeAllRanges();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { edge, sx: e.clientX, sy: e.clientY, start: rect };
    document.body.classList.add("dragging");
  };

  const move = (e: RPointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.sx;
    const dy = e.clientY - d.sy;
    const s = d.start;
    let r: Rect = { ...s };
    if (d.edge === "move") r = { ...s, x: s.x + dx, y: s.y + dy };
    if (d.edge.includes("e")) r.w = s.w + dx;
    if (d.edge.includes("s")) r.h = s.h + dy;
    if (d.edge.includes("w")) {
      const w = Math.max(MIN_W, s.w - dx);
      r.x = s.x + (s.w - w);
      r.w = w;
    }
    setRect(clamp(r));
  };

  const end = () => {
    if (!drag.current) return;
    drag.current = null;
    document.body.classList.remove("dragging");
  };

  // テキスト選択が始まらないように mousedown も止める
  const noSelect = (e: React.MouseEvent) => {
    if (!(e.target as HTMLElement).closest("button")) e.preventDefault();
  };

  const handle = (edge: Exclude<Edge, "move">) => (
    <div className={`rz rz-${edge}`} onMouseDown={noSelect} onPointerDown={begin(edge)} onPointerMove={move} onPointerUp={end} onPointerCancel={end} />
  );

  return (
    <div className="preview" style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }} role="dialog">
      <header
        className="preview-head"
        onMouseDown={noSelect}
        onPointerDown={begin("move")}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
        onDoubleClick={(e) => {
          if (!(e.target as HTMLElement).closest("button")) onOpen();
        }}
      >
        <span className="preview-title" title={file.path}>
          <strong>{file.name}</strong>
          {hit && <span className="preview-loc">{hit.label || (hit.line ? `${hit.line} 行目` : "")}</span>}
        </span>
        <button className="small" onClick={onOpen} title="メインウィンドウで開く(Enter / ダブルクリック)">
          開く
        </button>
        <button className="icon" onClick={onClose} aria-label="閉じる" title="閉じる(Esc)">
          ×
        </button>
      </header>
      <div className="preview-body">
        <LocationView
          path={file.path}
          kind={file.kind}
          target={hit ? { chunkId: hit.chunk_id, line: hit.line, lineEnd: hit.line_end } : null}
          terms={terms}
        />
      </div>
      {handle("e")}
      {handle("w")}
      {handle("s")}
      {handle("se")}
      {handle("sw")}
    </div>
  );
}
