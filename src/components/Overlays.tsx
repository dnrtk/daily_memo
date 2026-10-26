// 右クリックメニューと入力ダイアログ(WebView によっては window.prompt が使えないため自前)
import { useEffect, useRef, useState } from "react";

export interface MenuItem {
  label: string;
  onSelect(): void;
  danger?: boolean;
  separatorBefore?: boolean;
}

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose(): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  // 画面外にはみ出さないよう補正
  const left = Math.min(x, window.innerWidth - 220);
  const top = Math.min(y, window.innerHeight - items.length * 32 - 16);

  return (
    <div className="menu" ref={ref} style={{ left, top }} role="menu">
      {items.map((it) => (
        <div key={it.label}>
          {it.separatorBefore && <hr />}
          <button
            role="menuitem"
            className={it.danger ? "danger" : ""}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            {it.label}
          </button>
        </div>
      ))}
    </div>
  );
}

export interface PromptState {
  title: string;
  initial: string;
  /** 初期選択範囲(拡張子の前まで選択したいときなど) */
  selectUntil?: number;
  onSubmit(value: string): void;
}

export function PromptDialog({ state, onClose }: { state: PromptState; onClose(): void }) {
  const [value, setValue] = useState(state.initial);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.setSelectionRange(0, state.selectUntil ?? state.initial.length);
  }, [state]);

  return (
    <div className="backdrop" onMouseDown={onClose}>
      <form
        className="dialog prompt"
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          if (!value.trim()) return;
          onClose();
          state.onSubmit(value.trim());
        }}
      >
        <label>
          <span>{state.title}</span>
          <input
            ref={input}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && onClose()}
          />
        </label>
        <div className="dialog-actions">
          <button type="button" onClick={onClose}>
            キャンセル
          </button>
          <button type="submit" className="primary">
            OK
          </button>
        </div>
      </form>
    </div>
  );
}
