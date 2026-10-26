// エディタのグループ(タブバー + 本文)。メインウィンドウでは最大 2 つを左右に並べる。
// 本文は、テキスト: エディタ(Markdown は 編集 / 分割 / プレビュー)、Office・PDF: 閲覧タブ。
import { forwardRef, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { baseName } from "../api";
import type { EditorSettings } from "../types";
import { isMarkdown, type Group, type MdMode } from "../editorTypes";
import { DocView } from "./DocView";
import { Editor, type EditorHandle } from "./Editor";
import { MarkdownPreview } from "./MarkdownPreview";
import { KIND_LABEL } from "./SearchPanel";
import { beginTabDrag, type DropResult } from "./tabDrag";

interface Props {
  group: Group;
  focused: boolean;
  settings: EditorSettings;
  root: string;
  /** タブがないときに本文に出すもの(ようこそ画面 / 空グループの案内) */
  placeholder?: ReactNode;
  /** タブバー右端に出す追加ボタン */
  actions?: ReactNode;
  onFocus(): void;
  onActivate(path: string): void;
  onClose(path: string): void;
  onDirtyChange(path: string, dirty: boolean): void;
  onSetMode(path: string, mode: MdMode): void;
  onTabMenu(path: string, x: number, y: number): void;
  onTabDrop(path: string, r: DropResult): void;
  onOpenFile(path: string): void;
  onError(msg: string): void;
}

const MODES: [MdMode, string, string][] = [
  ["edit", "編集", "エディタのみ"],
  ["split", "分割", "エディタとプレビューを並べる(Ctrl+Shift+V)"],
  ["preview", "プレビュー", "プレビューのみ"],
];

export const EditorGroup = forwardRef<EditorHandle, Props>(function EditorGroup(
  {
    group,
    focused,
    settings,
    root,
    placeholder,
    actions,
    onFocus,
    onActivate,
    onClose,
    onDirtyChange,
    onSetMode,
    onTabMenu,
    onTabDrop,
    onOpenFile,
    onError,
  },
  ref,
) {
  const tab = group.tabs.find((t) => t.path === group.active) ?? null;
  const md = tab?.kind === "text" && isMarkdown(tab.path);
  const mode: MdMode = md ? (tab.mdMode ?? "edit") : "edit";
  const editorPath = tab?.kind === "text" ? tab.path : null;

  // プレビュー用の本文と先頭行(エディタから受け取る。描画はフレーム単位にまとめる)
  const [preview, setPreview] = useState<{ path: string; text: string; line: number | null }>({ path: "", text: "", line: null });
  const pending = useRef<{ path: string; text?: string; line?: number } | null>(null);
  const raf = useRef(0);
  const flush = useCallback(() => {
    const p = pending.current;
    pending.current = null;
    if (!p) return;
    setPreview((cur) => {
      const same = cur.path === p.path;
      return {
        path: p.path,
        text: p.text ?? (same ? cur.text : ""),
        line: p.line ?? (same ? cur.line : null),
      };
    });
  }, []);
  const queue = (path: string, patch: { text?: string; line?: number }) => {
    const prev = pending.current?.path === path ? pending.current : { path };
    pending.current = { ...prev, ...patch };
    cancelAnimationFrame(raf.current);
    raf.current = requestAnimationFrame(flush);
  };
  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  const showPreview = md && mode !== "edit";

  return (
    <section className={focused ? "egroup focused" : "egroup"} onPointerDownCapture={onFocus} onFocusCapture={onFocus}>
      <div className="tabbar" role="tablist" data-drop={`editor-tabs:${group.id}`}>
        <div className="tabs-scroll">
          {group.tabs.map((t) => (
            <div
              key={t.path}
              role="tab"
              aria-selected={t.path === group.active}
              className={t.path === group.active ? "tab on" : "tab"}
              title={t.path}
              onPointerDown={(e) => beginTabDrag(e, baseName(t.path), "editor", (r) => onTabDrop(t.path, r))}
              onMouseDown={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  onClose(t.path);
                }
              }}
              onClick={() => onActivate(t.path)}
              onContextMenu={(e) => {
                e.preventDefault();
                onTabMenu(t.path, e.clientX, e.clientY);
              }}
            >
              {t.kind === "doc" && <span className={`kind kind-${t.docKind}`}>{KIND_LABEL[t.docKind ?? ""] ?? "DOC"}</span>}
              <span>{baseName(t.path)}</span>
              <button
                className={t.dirty ? "close dirty" : "close"}
                aria-label="閉じる"
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(t.path);
                }}
              >
                {t.dirty ? "●" : "×"}
              </button>
            </div>
          ))}
        </div>
        <div className="tabbar-actions">
          {md && tab && (
            <div className="seg mini" role="radiogroup" aria-label="Markdown の表示">
              {MODES.map(([m, label, title]) => (
                <button key={m} className={mode === m ? "on" : ""} title={title} onClick={() => onSetMode(tab.path, m)}>
                  {label}
                </button>
              ))}
            </div>
          )}
          {actions}
        </div>
      </div>

      <div className="pane-body" data-drop={`editor:${group.id}`} data-drop-split="">
        <div className={showPreview ? "pane-editor half" : "pane-editor"} hidden={mode === "preview" || !editorPath}>
          <Editor
            ref={ref}
            activePath={editorPath}
            settings={settings}
            onDirtyChange={onDirtyChange}
            onError={onError}
            onText={(p, text) => queue(p, { text })}
            onTopLine={(p, line) => queue(p, { line })}
          />
        </div>
        {showPreview && tab && preview.path === tab.path && (
          <div className={mode === "preview" ? "pane-preview full" : "pane-preview"}>
            <MarkdownPreview
              text={preview.text}
              path={tab.path}
              root={root}
              topLine={mode === "split" ? preview.line : null}
              onOpenFile={onOpenFile}
            />
          </div>
        )}
        {tab?.kind === "doc" && (
          <DocView
            key={tab.path}
            path={tab.path}
            kind={tab.docKind ?? ""}
            chunkId={tab.focusChunk ?? null}
            seq={tab.focusSeq ?? 0}
            terms={tab.terms ?? []}
            onError={onError}
          />
        )}
        {!tab && placeholder}
      </div>
    </section>
  );
});
