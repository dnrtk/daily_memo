// タブのドラッグ(エディタ / ターミナル共通)。
// ポインターキャプチャを使うので、ウィンドウの外で離したことも分かる(→ 別ウィンドウに切り離す)。
//
// ドロップ先は data-drop 属性で表す:
//   data-drop="editor:3"      … グループ(本文)。data-drop-split があれば右側 40% は「右に分割」
//   data-drop="editor-tabs:3" … グループのタブバー(そのグループへ移動)

export interface DropResult {
  /** 外で離した(切り離し) */
  outside: boolean;
  /** data-drop の値 */
  target: string | null;
  /** 本文の右側で離した */
  split: boolean;
}

const DRAG_THRESHOLD = 6;

function hitTest(x: number, y: number): { el: HTMLElement | null; target: string | null; split: boolean } {
  const el = (document.elementFromPoint(x, y) as HTMLElement | null)?.closest<HTMLElement>("[data-drop]") ?? null;
  if (!el) return { el: null, target: null, split: false };
  let split = false;
  if (el.dataset.dropSplit !== undefined) {
    const r = el.getBoundingClientRect();
    split = x > r.left + r.width * 0.6;
  }
  return { el, target: el.dataset.drop ?? null, split };
}

/**
 * タブの pointerdown から呼ぶ。少し動かすまではドラッグにしない(クリックはそのまま効く)。
 * @param accept ドロップ先として受け付ける data-drop の接頭辞("editor" / "term")
 */
export function beginTabDrag(
  e: React.PointerEvent<HTMLElement>,
  label: string,
  accept: string,
  onDrop: (r: DropResult) => void,
) {
  if (e.button !== 0) return;
  if ((e.target as HTMLElement).closest("button")) return;
  const el = e.currentTarget;
  const sx = e.clientX;
  const sy = e.clientY;
  const pointerId = e.pointerId;
  let dragging = false;
  let ghost: HTMLDivElement | null = null;
  let marked: HTMLElement | null = null;

  const unmark = () => {
    marked?.classList.remove("drop-into", "drop-split");
    marked = null;
  };

  const accepts = (t: string | null) => !!t && (t === accept || t.startsWith(`${accept}:`) || t.startsWith(`${accept}-tabs:`));

  const move = (ev: PointerEvent) => {
    if (!dragging) {
      if (Math.hypot(ev.clientX - sx, ev.clientY - sy) < DRAG_THRESHOLD) return;
      dragging = true;
      try {
        el.setPointerCapture(pointerId);
      } catch {
        /* noop */
      }
      ghost = document.createElement("div");
      ghost.className = "drag-ghost";
      ghost.textContent = label;
      document.body.appendChild(ghost);
      document.body.classList.add("dragging", "tab-dragging");
    }
    window.getSelection()?.removeAllRanges();
    if (ghost) {
      ghost.style.left = `${ev.clientX + 12}px`;
      ghost.style.top = `${ev.clientY + 10}px`;
    }
    const outside = ev.clientX < 0 || ev.clientY < 0 || ev.clientX > window.innerWidth || ev.clientY > window.innerHeight;
    document.body.classList.toggle("drag-outside", outside);
    const h = hitTest(ev.clientX, ev.clientY);
    if (h.el !== marked || !h.el) unmark();
    if (h.el && accepts(h.target)) {
      marked = h.el;
      h.el.classList.toggle("drop-split", h.split);
      h.el.classList.toggle("drop-into", !h.split);
    }
  };

  let done = false;
  const finish = (ev: PointerEvent, cancelled: boolean) => {
    if (done) return;
    done = true;
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    unmark();
    ghost?.remove();
    document.body.classList.remove("dragging", "tab-dragging", "drag-outside");
    if (!dragging || cancelled) return;
    const outside = ev.clientX < 0 || ev.clientY < 0 || ev.clientX > window.innerWidth || ev.clientY > window.innerHeight;
    const h = hitTest(ev.clientX, ev.clientY);
    onDrop({ outside, target: accepts(h.target) ? h.target : null, split: h.split });
  };
  const up = (ev: PointerEvent) => finish(ev, false);
  const cancel = (ev: PointerEvent) => finish(ev, true);

  // キャプチャ中のイベントも window まで伝わるので、window だけで受ける
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
}

/** "editor:3" → 3 */
export function dropGroupId(target: string | null): number | null {
  if (!target) return null;
  const n = Number(target.split(":")[1]);
  return Number.isFinite(n) ? n : null;
}
