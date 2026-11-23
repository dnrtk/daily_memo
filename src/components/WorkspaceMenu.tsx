import { useEffect, useRef, useState, type CSSProperties } from "react";
import { api, errorMessage } from "../api";
import type { RecentWorkspace, WorkspaceInfo } from "../types";

/** パスの比較(Windows は大文字小文字・区切り文字の違いを無視) */
export function samePath(a: string, b: string): boolean {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  return norm(a) === norm(b);
}

/** 作業フォルダの頭文字(切り替えアイコンに表示) */
export function initialOf(name: string): string {
  const ch = Array.from(name.trim())[0] ?? "?";
  return ch.toUpperCase();
}

/** パスから色相を決める(同じフォルダは常に同じ色。見分けやすさのための控えめな色) */
function hueOf(path: string): number {
  let h = 0;
  for (const c of path.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

export function WorkspaceTile({ name, path, size = 26 }: { name: string; path: string; size?: number }) {
  return (
    <span className="ws-tile" style={{ "--h": hueOf(path), width: size, height: size } as CSSProperties} aria-hidden>
      {initialOf(name)}
    </span>
  );
}

interface Props {
  /** メニューの左上(画面座標) */
  x: number;
  y: number;
  current: WorkspaceInfo | null;
  onSwitch(path: string): void;
  onBrowse(): void;
  onReveal(): void;
  onClose(): void;
  onError(msg: string): void;
}

export function WorkspaceMenu({ x, y, current, onSwitch, onBrowse, onReveal, onClose, onError }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [recent, setRecent] = useState<RecentWorkspace[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .recentWorkspaces()
      .then((r) => !cancelled && setRecent(r))
      .catch((e) => {
        if (cancelled) return;
        setRecent([]);
        onError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [onError]);

  // 開いたら「直前に使っていた作業フォルダ」にフォーカス(Enter だけで行き来できる)
  useEffect(() => {
    if (!recent) return;
    const items = menuItems();
    const other = items.find((b) => b.dataset.current !== "1" && b.dataset.kind === "recent");
    (other ?? items[0])?.focus();
  }, [recent]);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      if (!ref.current?.contains(t) && !t.closest(".ws-switch")) onClose();
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  const menuItems = () => Array.from(ref.current?.querySelectorAll<HTMLButtonElement>("[role=menuitem]") ?? []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "Delete") {
      const path = (document.activeElement as HTMLElement | null)?.dataset.path;
      const isCurrent = (document.activeElement as HTMLElement | null)?.dataset.current === "1";
      if (path && !isCurrent) {
        e.preventDefault();
        void forget(path);
      }
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    const items = menuItems();
    if (items.length === 0) return;
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      e.key === "Home" ? 0
      : e.key === "End" ? items.length - 1
      : e.key === "ArrowDown" ? (i + 1) % items.length
      : (i - 1 + items.length) % items.length;
    items[next].focus();
  };

  const forget = async (path: string) => {
    try {
      await api.forgetWorkspace(path);
      setRecent((r) => r?.filter((w) => w.path !== path) ?? r);
    } catch (e) {
      onError(errorMessage(e));
    }
  };

  // 現在のフォルダを先頭に、残りは最近使った順
  const list = recent ?? [];
  const cur = current ? list.find((w) => samePath(w.path, current.root)) : undefined;
  const rows: RecentWorkspace[] = [
    ...(current ? [cur ?? { path: current.root, name: current.name, exists: true }] : []),
    ...list.filter((w) => !current || !samePath(w.path, current.root)),
  ];

  return (
    <div
      ref={ref}
      className="ws-menu"
      role="menu"
      aria-label="作業フォルダの切り替え"
      style={{ left: x, top: y }}
      onKeyDown={onKeyDown}
    >
      <div className="ws-menu-head">作業フォルダ</div>
      {recent === null ? (
        <div className="ws-menu-note">読み込み中…</div>
      ) : (
        <ul className="ws-list">
          {rows.map((w) => {
            const isCurrent = !!current && samePath(w.path, current.root);
            return (
              <li key={w.path} className={isCurrent ? "current" : w.exists ? "" : "missing"}>
                <button
                  role="menuitem"
                  data-kind="recent"
                  data-path={w.path}
                  data-current={isCurrent ? "1" : "0"}
                  aria-current={isCurrent ? "true" : undefined}
                  title={isCurrent ? `${w.path}(開いています)` : w.exists ? `${w.path} に切り替える` : `${w.path}(見つかりません。ネットワークドライブなら接続してから選んでください)`}
                  onClick={() => (isCurrent ? onClose() : onSwitch(w.path))}
                >
                  <WorkspaceTile name={w.name} path={w.path} />
                  <span className="ws-text">
                    <span className="ws-label">{w.name}</span>
                    <span className="ws-path">{w.path}</span>
                  </span>
                  {isCurrent ? (
                    <span className="ws-tag ok">
                      <svg viewBox="0 0 24 24" aria-hidden><path d="m5 12.5 4.5 4.5L19 7.5" /></svg>
                      開いています
                    </span>
                  ) : (
                    !w.exists && <span className="ws-tag">見つかりません</span>
                  )}
                </button>
                {!isCurrent && (
                  <button
                    className="ws-forget"
                    tabIndex={-1}
                    title="一覧から外す(フォルダは消えません。Delete キーでも可)"
                    aria-label={`${w.name} を一覧から外す`}
                    onClick={() => void forget(w.path)}
                  >
                    <svg viewBox="0 0 24 24" aria-hidden><path d="M7 7l10 10M17 7 7 17" /></svg>
                  </button>
                )}
              </li>
            );
          })}
          {rows.length === 0 && <li className="ws-menu-note">まだ作業フォルダを開いていません</li>}
        </ul>
      )}
      <div className="ws-menu-foot">
        <button role="menuitem" data-kind="action" onClick={onBrowse}>
          <svg viewBox="0 0 24 24" aria-hidden><path d="M3 6h7l2 2h9v11H3z" /><path d="M12 11v5M9.5 13.5h5" /></svg>
          <span>別のフォルダを開く…</span>
          <kbd>Ctrl+O</kbd>
        </button>
        {current && (
          <button role="menuitem" data-kind="action" onClick={onReveal}>
            <svg viewBox="0 0 24 24" aria-hidden><path d="M14 4h6v6M20 4l-9 9M18 14v6H4V6h6" /></svg>
            <span>エクスプローラーで開く</span>
            <kbd>Ctrl+E</kbd>
          </button>
        )}
      </div>
    </div>
  );
}
