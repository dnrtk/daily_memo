// 切り離したウィンドウ。1 ウィンドウ = 1 タブ(エディタ / ターミナル)。
// - 「メインに戻す」でメインウィンドウのタブに戻る(エディタは保存してから、ターミナルはシェルを維持したまま)
// - ウィンドウを閉じる: エディタは保存して閉じる / ターミナルはシェルを終了する
import { useCallback, useEffect, useRef, useState } from "react";
import { emit, emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, baseName, errorMessage } from "./api";
import type { AppSettings, WorkspaceInfo } from "./types";
import { isMarkdown, type DetachedPayload, type Group, type MdMode, type Tab, type TermRestore } from "./editorTypes";
import type { EditorHandle } from "./components/Editor";
import { EditorGroup } from "./components/EditorGroup";
import { ContextMenu, type MenuItem } from "./components/Overlays";
import { onTerminalExit, TermView, type TermViewHandle } from "./components/TerminalPanel";

export interface DetachedView {
  label: string;
  payload: DetachedPayload;
}

export function Detached({ view }: { view: DetachedView }) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const onError = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => (t === msg ? null : t)), 6000);
  }, []);

  useEffect(() => {
    api.getSettings().then(setSettings).catch((e) => onError(errorMessage(e)));
    const un = listen<AppSettings>("settings-changed", (e) => setSettings(e.payload));
    return () => void un.then((f) => f());
  }, [onError]);

  if (!settings) return null;
  return (
    <div className="detached">
      {view.payload.kind === "editor" ? (
        <DetachedEditor initial={view.payload.tab} settings={settings} onError={onError} />
      ) : (
        <DetachedTerminal
          id={view.payload.id}
          title={view.payload.title}
          restore={view.payload.restore ?? null}
          settings={settings}
          onError={onError}
        />
      )}
      {toast && (
        <div className="toast error" role="status">
          {toast}
        </div>
      )}
    </div>
  );
}

// ---- エディタ --------------------------------------------------------------------

function DetachedEditor({ initial, settings, onError }: { initial: Tab; settings: AppSettings; onError(m: string): void }) {
  const [tab, setTab] = useState<Tab>(initial);
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const editor = useRef<EditorHandle | null>(null);
  const tabRef = useRef(tab);
  tabRef.current = tab;
  /** 戻す / 削除で閉じるときは、閉じる前の保存・通知をしない */
  const leaving = useRef(false);

  useEffect(() => {
    api.currentWorkspace().then(setWorkspace).catch(() => {});
  }, []);

  useEffect(() => {
    void getCurrentWindow().setTitle(baseName(tab.path));
  }, [tab.path]);

  const group: Group = { id: 1, tabs: [tab], active: tab.path };

  const close = async () => {
    await editor.current?.saveAll();
    leaving.current = true;
    await emitTo("main", "detached-closed", { path: tabRef.current.path });
    await getCurrentWindow().destroy();
  };

  const reattach = async () => {
    await editor.current?.saveAll();
    leaving.current = true;
    const payload: DetachedPayload = { kind: "editor", tab: { ...tabRef.current, dirty: false } };
    try {
      await api.reattach(payload);
    } catch (e) {
      leaving.current = false;
      onError(`メインに戻せませんでした: ${errorMessage(e)}`);
    }
  };

  // メインからの通知と、閉じるボタン
  useEffect(() => {
    const subs = [
      listen<{ from: string; to: string }>("paths-moved", (e) => {
        const { from, to } = e.payload;
        const p = tabRef.current.path;
        if (p === from || p.startsWith(`${from}/`)) {
          editor.current?.rename(from, to);
          setTab((t) => ({ ...t, path: to + p.slice(from.length) }));
        }
      }),
      listen<{ path: string }>("paths-removed", (e) => {
        const p = tabRef.current.path;
        if (p === e.payload.path || p.startsWith(`${e.payload.path}/`)) {
          editor.current?.forget(p);
          leaving.current = true;
          void getCurrentWindow().destroy();
        }
      }),
      listen<{ path: string }>("file-changed", (e) => {
        if (e.payload.path === tabRef.current.path) void editor.current?.reload(e.payload.path);
      }),
    ];
    const closeReq = getCurrentWindow().onCloseRequested(async () => {
      if (leaving.current) return;
      await editor.current?.saveAll();
      await emitTo("main", "detached-closed", { path: tabRef.current.path });
    });
    return () => {
      subs.forEach((s) => void s.then((f) => f()));
      void closeReq.then((f) => f());
    };
  }, []);

  // ショートカット
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (!mod) return;
      const k = e.key.toLowerCase();
      if (k === "s" && !e.shiftKey) {
        e.preventDefault();
        void editor.current?.save(tabRef.current.path);
      } else if (k === "w" && !e.shiftKey) {
        e.preventDefault();
        void close();
      } else if (k === "v" && e.shiftKey && isMarkdown(tabRef.current.path)) {
        e.preventDefault();
        setTab((t) => ({ ...t, mdMode: (t.mdMode ?? "edit") === "edit" ? "split" : "edit" }));
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <>
      <EditorGroup
        ref={(h) => {
          editor.current = h;
        }}
        group={group}
        focused={false}
        settings={settings.editor}
        root={workspace?.root ?? ""}
        actions={
          <button className="small" title="このタブをメインウィンドウに戻す" onClick={() => void reattach()}>
            メインに戻す
          </button>
        }
        onFocus={() => {}}
        onActivate={() => {}}
        onClose={() => void close()}
        onDirtyChange={(_p, dirty) => setTab((t) => (t.dirty === dirty ? t : { ...t, dirty }))}
        onSetMode={(_p, mode: MdMode) => setTab((t) => ({ ...t, mdMode: mode }))}
        onTabMenu={(_p, x, y) =>
          setMenu({
            x,
            y,
            items: [
              { label: "メインに戻す", onSelect: () => void reattach() },
              { label: "閉じる", separatorBefore: true, onSelect: () => void close() },
            ],
          })
        }
        onTabDrop={() => {}}
        onOpenFile={(p) => void emit("open-file", { path: p })}
        onError={onError}
      />
      {menu && <ContextMenu {...menu} onClose={() => setMenu(null)} />}
    </>
  );
}

// ---- ターミナル ------------------------------------------------------------------

function DetachedTerminal({
  id,
  title,
  restore,
  settings,
  onError,
}: {
  id: number;
  title: string;
  restore: TermRestore | null;
  settings: AppSettings;
  onError(m: string): void;
}) {
  const view = useRef<TermViewHandle | null>(null);
  const leaving = useRef(false);

  useEffect(() => {
    // シェルが終わったらウィンドウも閉じる
    const off = onTerminalExit((x) => {
      if (x === id) {
        leaving.current = true;
        void getCurrentWindow().destroy();
      }
    });
    // ウィンドウを閉じる = シェルを終了(戻すときは destroy なのでここを通らない)
    const closeReq = getCurrentWindow().onCloseRequested(async () => {
      if (!leaving.current) await api.termKill(id).catch(() => {});
    });
    return () => {
      off();
      void closeReq.then((f) => f());
    };
  }, [id]);

  const reattach = async () => {
    leaving.current = true;
    try {
      await api.reattach({ kind: "terminal", id, title, restore: view.current?.capture() ?? null } satisfies DetachedPayload);
    } catch (e) {
      leaving.current = false;
      onError(`メインに戻せませんでした: ${errorMessage(e)}`);
    }
  };

  return (
    <section className="terminal-panel detached-terminal">
      <header className="terminal-tabs">
        <div className="ttab on">
          <span>{title}</span>
          <button className="close" title="閉じる(シェルを終了)" onClick={() => void getCurrentWindow().close()}>
            ×
          </button>
        </div>
        <span className="spacer" />
        <button className="small" title="このターミナルをメインウィンドウに戻す" onClick={() => void reattach()}>
          メインに戻す
        </button>
      </header>
      <div className="terminal-body">
        <TermView
          ref={(h) => {
            view.current = h;
          }}
          attachId={id}
          restore={restore}
          shown
          fontSize={Math.max(11, settings.editor.font_size - 1)}
          onSpawned={() => {}}
          onError={onError}
        />
      </div>
    </section>
  );
}
