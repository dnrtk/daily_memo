// クイックメモ: カテゴリを選んで保存。
// 次フェーズで「自動(LLM が振り分け)」を選択肢に追加する予定。
import { useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../api";
import type { CategoryConfig, Saved } from "../types";

interface Props {
  categories: CategoryConfig;
  onClose(): void;
  onSaved(saved: Saved): void;
}

const LAST_KEY = "quickCapture.lastCategory";

/** ヒント表示用に日付系プレースホルダだけ展開する */
function previewName(pattern: string): string {
  const d = new Date();
  const p2 = (n: number) => String(n).padStart(2, "0");
  return pattern
    .replaceAll("{yyyy}", String(d.getFullYear()))
    .replaceAll("{MM}", p2(d.getMonth() + 1))
    .replaceAll("{dd}", p2(d.getDate()))
    .replaceAll("{HH}", p2(d.getHours()))
    .replaceAll("{mm}", p2(d.getMinutes()));
}

export function QuickCapture({ categories, onClose, onSaved }: Props) {
  const initial = (() => {
    try {
      const last = localStorage.getItem(LAST_KEY);
      if (last && categories.categories.some((c) => c.id === last)) return last;
    } catch {
      /* storage 不可でも動かす */
    }
    return categories.daily_category ?? categories.categories[0]?.id ?? "";
  })();
  const [categoryId, setCategoryId] = useState(initial);
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const body = useRef<HTMLTextAreaElement>(null);
  useEffect(() => body.current?.focus(), []);

  const cat = categories.categories.find((c) => c.id === categoryId);

  const submit = async () => {
    if (!content.trim() || !cat) return;
    setBusy(true);
    try {
      const saved = await api.saveToCategory(cat.id, { title, content });
      try {
        localStorage.setItem(LAST_KEY, cat.id);
      } catch {
        /* noop */
      }
      onSaved(saved);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <div className="backdrop" onMouseDown={onClose}>
      <div
        className="dialog capture"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onClose();
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void submit();
        }}
      >
        <h2>クイックメモ</h2>
        <div className="chips" role="radiogroup" aria-label="保存先カテゴリ">
          {categories.categories.map((c) => (
            <button
              key={c.id}
              role="radio"
              aria-checked={c.id === categoryId}
              className={c.id === categoryId ? "chip on" : "chip"}
              title={c.description}
              onClick={() => setCategoryId(c.id)}
            >
              {c.name}
            </button>
          ))}
        </div>
        {cat?.mode === "new_file" && (
          <input
            placeholder="タイトル(空なら本文の1行目)"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
        )}
        <textarea
          ref={body}
          rows={8}
          placeholder="内容"
          value={content}
          onChange={(e) => setContent(e.target.value)}
        />
        <p className="hint">
          {cat?.mode === "append" ? `${cat.dir}/${previewName(cat.filename)} に追記` :  `${cat?.dir ?? ""}/ に新しいファイルとして保存`} ・ Ctrl+Enter で保存
        </p>
        {error && <p className="error">{error}</p>}
        <div className="dialog-actions">
          <button onClick={onClose}>キャンセル</button>
          <button className="primary" disabled={busy || !content.trim()} onClick={() => void submit()}>
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
