// 左右に並べたグループの境界。ドラッグで比率を変え、ダブルクリックで半々に戻す。
// 親(左側グループ)の右端に絶対配置し、比率は左右グループの親要素の幅に対して計算する。
import { useRef } from "react";

export function SplitDivider({ ratio, onChange }: { ratio: number; onChange(r: number): void }) {
  const st = useRef<{ x: number; r: number; w: number } | null>(null);
  return (
    <div
      className="group-divider"
      title="ドラッグで幅を変更(ダブルクリックで半々)"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.stopPropagation();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        const container = e.currentTarget.parentElement?.parentElement as HTMLElement | null;
        st.current = { x: e.clientX, r: ratio, w: container?.clientWidth ?? window.innerWidth };
        document.body.classList.add("dragging", "col-resize");
      }}
      onPointerMove={(e) => {
        if (!st.current) return;
        onChange(Math.min(0.85, Math.max(0.15, st.current.r + (e.clientX - st.current.x) / st.current.w)));
      }}
      onPointerUp={() => {
        st.current = null;
        document.body.classList.remove("dragging", "col-resize");
      }}
      onPointerCancel={() => {
        st.current = null;
        document.body.classList.remove("dragging", "col-resize");
      }}
      onDoubleClick={() => onChange(0.5)}
    />
  );
}
