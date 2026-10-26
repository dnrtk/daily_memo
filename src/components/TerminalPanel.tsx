// 下部のターミナルパネル。左右 2 グループまで分割でき、各グループにタブを複数持てる。
// - 非表示にしてもセッション(シェルのプロセス)と画面内容は残る。× で閉じたタブだけプロセスを終了
// - タブは別グループへ移動 / 別ウィンドウへ切り離し / 戻すことができる。移動先では Rust 側の出力履歴を
//   再生してから続きを表示するので、同じシェルの続きがそのまま見える
// - シェルが exit で終わったらタブも閉じる。最後のタブが閉じたらパネルを隠す
// - Ctrl+J はアプリ側(パネルの開閉)に渡す。Ctrl+C は選択中ならコピー、Ctrl+V は貼り付け
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type PointerEvent as RPointerEvent,
} from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SerializeAddon } from "@xterm/addon-serialize";
import "@xterm/xterm/css/xterm.css";
import { api, errorMessage } from "../api";
import { beginTabDrag, dropGroupId, type DropResult } from "./tabDrag";
import type { MenuItem } from "./Overlays";
import { SplitDivider } from "./SplitDivider";
import type { TermRestore } from "../editorTypes";

interface Session {
  key: number;
  /** PTY の ID。null の間は起動中 */
  id: number | null;
  title: string;
  /** 移動・戻ってきたときに引き継ぐ画面 */
  restore?: TermRestore | null;
}

interface TGroup {
  id: number;
  keys: number[];
  active: number | null;
}

export interface TerminalPanelHandle {
  /** タブがなければ作る。あればアクティブなタブにフォーカス */
  ensureSession(): void;
  /** 既存のセッション(別ウィンドウから戻ってきたもの)をタブとして付ける */
  attach(id: number, title: string, restore?: TermRestore | null): void;
  focus(): void;
}

interface Props {
  visible: boolean;
  height: number;
  fontSize: number;
  onHeightChange(h: number): void;
  onHide(): void;
  onEmpty(): void;
  onError(msg: string): void;
  onMenu(x: number, y: number, items: MenuItem[]): void;
}

const LIGHT: ITheme = {
  background: "#f7f6f3",
  foreground: "#2b2a27",
  cursor: "#2f6f62",
  cursorAccent: "#f7f6f3",
  selectionBackground: "#2f6f6240",
  black: "#2b2a27",
  red: "#b4432f",
  green: "#3d7a3a",
  yellow: "#9a6a00",
  blue: "#2f5fa0",
  magenta: "#8a3f8a",
  cyan: "#2f6f62",
  white: "#d9d5cc",
  brightBlack: "#75716a",
  brightRed: "#c9573f",
  brightGreen: "#4d8f48",
  brightYellow: "#b07c00",
  brightBlue: "#4a76b8",
  brightMagenta: "#a050a0",
  brightCyan: "#3d8a7a",
  brightWhite: "#ffffff",
};

const DARK: ITheme = {
  background: "#191b1d",
  foreground: "#d9d7d2",
  cursor: "#6fb5a3",
  cursorAccent: "#191b1d",
  selectionBackground: "#6fb5a350",
  black: "#26292d",
  red: "#e0806b",
  green: "#8cc084",
  yellow: "#d8b45a",
  blue: "#7aa5d8",
  magenta: "#c08ad0",
  cyan: "#6fb5a3",
  white: "#d9d7d2",
  brightBlack: "#6b6a66",
  brightRed: "#f0947f",
  brightGreen: "#a2d49a",
  brightYellow: "#ecc86e",
  brightBlue: "#94bbea",
  brightMagenta: "#d4a0e2",
  brightCyan: "#86cab8",
  brightWhite: "#ffffff",
};

const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

// ---- 出力の振り分け(ウィンドウ内で 1 回だけ購読) --------------------------------

type Writer = (data: string, start: number) => void;
const writers = new Map<number, Writer>();
const pendingOut = new Map<number, { data: string; start: number }[]>();
const exitHandlers = new Set<(id: number) => void>();
let subscribed = false;

function subscribeOutput() {
  if (subscribed) return;
  subscribed = true;
  void api.onTermOutput((id, data, start) => {
    const w = writers.get(id);
    if (w) w(data, start);
    else {
      const list = pendingOut.get(id) ?? [];
      list.push({ data, start });
      // 誰も受け取らないまま溜まり続けないように上限を設ける(履歴は Rust 側にある)
      if (list.length > 2000) list.splice(0, list.length - 2000);
      pendingOut.set(id, list);
    }
  });
  void api.onTermExit((id) => {
    writers.delete(id);
    pendingOut.delete(id);
    exitHandlers.forEach((h) => h(id));
  });
}

/** 終了したセッションの通知を受ける */
export function onTerminalExit(h: (id: number) => void): () => void {
  subscribeOutput();
  exitHandlers.add(h);
  return () => exitHandlers.delete(h);
}

// ---- パネル --------------------------------------------------------------------

export const TerminalPanel = forwardRef<TerminalPanelHandle, Props>(function TerminalPanel(
  { visible, height, fontSize, onHeightChange, onHide, onEmpty, onError, onMenu },
  ref,
) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [groups, setGroups] = useState<TGroup[]>([{ id: 1, keys: [], active: null }]);
  const [focusedGroup, setFocusedGroup] = useState(1);
  const [ratio, setRatio] = useState(0.5);
  const nextKey = useRef(1);
  const nextGroup = useRef(2);
  const views = useRef(new Map<number, TermViewHandle>());
  const cb = useRef({ onEmpty, onError });
  cb.current = { onEmpty, onError };

  // 終了したシェルのタブを閉じる
  useEffect(
    () =>
      onTerminalExit((id) => {
        setSessions((s) => {
          const gone = s.find((x) => x.id === id);
          if (gone) setGroups((g) => removeKey(g, gone.key));
          return s.filter((x) => x.id !== id);
        });
      }),
    [],
  );

  // 全部消えたら通知
  const prevCount = useRef(0);
  useEffect(() => {
    if (sessions.length === 0 && prevCount.current > 0) cb.current.onEmpty();
    prevCount.current = sessions.length;
  }, [sessions]);

  const focusKey = (key: number | null) => {
    if (key !== null) window.setTimeout(() => views.current.get(key)?.focus(), 0);
  };

  /** グループにセッションを追加(groupId が無ければ新しいグループを右に作る) */
  const addSession = (s: Session, groupId: number | "new") => {
    setSessions((list) => [...list, s]);
    setGroups((gs) => {
      if (groupId === "new" && gs.length < 2) {
        const id = nextGroup.current++;
        setFocusedGroup(id);
        return [...gs, { id, keys: [s.key], active: s.key }];
      }
      const target = gs.find((g) => g.id === groupId) ?? gs.find((g) => g.id === focusedGroup) ?? gs[0];
      setFocusedGroup(target.id);
      return gs.map((g) => (g.id === target.id ? { ...g, keys: [...g.keys, s.key], active: s.key } : g));
    });
  };

  const add = (groupId: number | "new" = focusedGroup) => addSession({ key: nextKey.current++, id: null, title: "…" }, groupId);

  const close = (key: number) => {
    const s = sessions.find((x) => x.key === key);
    if (s?.id != null) {
      writers.delete(s.id);
      void api.termKill(s.id);
    }
    setSessions((list) => list.filter((x) => x.key !== key));
    setGroups((g) => removeKey(g, key));
  };

  /** 画面を引き継ぎ用に保存(移動先で再マウントされる) */
  const capture = (key: number) => {
    const restore = views.current.get(key)?.capture() ?? null;
    setSessions((list) => list.map((x) => (x.key === key ? { ...x, restore } : x)));
    return restore;
  };

  /** 別グループへ移動("new" なら右に分割) */
  const move = (key: number, to: number | "new") => {
    capture(key);
    setGroups((gs) => {
      const from = gs.find((g) => g.keys.includes(key));
      if (!from) return gs;
      if (to === "new") {
        if (gs.length >= 2 || from.keys.length <= 1) {
          // 既に 2 分割、または 1 タブしかない → もう一方のグループへ
          const other = gs.find((g) => g.id !== from.id);
          if (!other) return gs;
          to = other.id;
        } else {
          const id = nextGroup.current++;
          setFocusedGroup(id);
          return [...removeKey(gs, key), { id, keys: [key], active: key }];
        }
      }
      if (to === from.id) return gs;
      const target = to;
      setFocusedGroup(target);
      return removeKey(gs, key).map((g) => (g.id === target ? { ...g, keys: [...g.keys, key], active: key } : g));
    });
    focusKey(key);
  };

  /** 別ウィンドウへ切り離す(シェルはそのまま) */
  const detach = async (key: number) => {
    const s = sessions.find((x) => x.key === key);
    if (!s || s.id === null) return;
    try {
      const restore = views.current.get(key)?.capture() ?? null;
      writers.delete(s.id);
      setSessions((list) => list.filter((x) => x.key !== key));
      setGroups((g) => removeKey(g, key));
      await api.detachWindow({ kind: "terminal", id: s.id, title: s.title, restore }, `ターミナル - ${s.title}`);
    } catch (e) {
      cb.current.onError(`切り離せませんでした: ${errorMessage(e)}`);
    }
  };

  const onDrop = (key: number, r: DropResult) => {
    if (r.outside) {
      void detach(key);
      return;
    }
    const gid = dropGroupId(r.target);
    if (gid === null) return;
    const from = groups.find((g) => g.keys.includes(key));
    if (r.split && r.target?.startsWith("term:")) {
      if (groups.length === 1) move(key, "new");
      else if (gid !== groups[1].id || from?.id !== groups[1].id) move(key, groups[1].id);
      return;
    }
    move(key, gid);
  };

  const menu = (key: number, x: number, y: number) => {
    const g = groups.find((gg) => gg.keys.includes(key));
    const other = groups.find((gg) => gg.id !== g?.id);
    const items: MenuItem[] = [];
    if (other) items.push({ label: groups[0].id === g?.id ? "右のグループへ移動" : "左のグループへ移動", onSelect: () => move(key, other.id) });
    else if ((g?.keys.length ?? 0) > 1) items.push({ label: "右に分割して移動", onSelect: () => move(key, "new") });
    items.push({ label: "別ウィンドウに切り離す", onSelect: () => void detach(key) });
    items.push({ label: "閉じる(シェルを終了)", danger: true, separatorBefore: true, onSelect: () => close(key) });
    onMenu(x, y, items);
  };

  useImperativeHandle(ref, () => ({
    ensureSession() {
      if (sessions.length === 0) add();
      else focusKey(groups.find((g) => g.id === focusedGroup)?.active ?? groups[0].active);
    },
    attach(id, title, restore) {
      const key = nextKey.current++;
      addSession({ key, id, title, restore }, focusedGroup);
      focusKey(key);
    },
    focus() {
      focusKey(groups.find((g) => g.id === focusedGroup)?.active ?? null);
    },
  }));

  // 高さのドラッグ
  const drag = useRef<{ y: number; h: number } | null>(null);
  const onDown = (e: RPointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { y: e.clientY, h: height };
    document.body.classList.add("dragging", "row-resize");
  };
  const onMove = (e: RPointerEvent) => {
    if (!drag.current) return;
    const max = Math.max(120, window.innerHeight - 160);
    onHeightChange(Math.round(Math.min(max, Math.max(100, drag.current.h - (e.clientY - drag.current.y)))));
  };
  const onUp = () => {
    drag.current = null;
    document.body.classList.remove("dragging", "row-resize");
  };

  // 同じシェル名が複数あれば番号を付ける
  const titleOf = (key: number) => {
    const s = sessions.find((x) => x.key === key);
    if (!s) return "";
    const same = sessions.filter((x) => x.title === s.title);
    return same.length > 1 ? `${s.title} ${same.findIndex((x) => x.key === key) + 1}` : s.title;
  };

  const split = groups.length === 2;

  return (
    <section className="terminal-panel" hidden={!visible} style={{ height }}>
      <div className="terminal-resizer" onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerCancel={onUp} />
      <div className="terminal-groups">
        {groups.map((g, gi) => (
          <div
            key={g.id}
            className={g.id === focusedGroup && split ? "tgroup focused" : "tgroup"}
            style={split ? { flexBasis: `${(gi === 0 ? ratio : 1 - ratio) * 100}%` } : undefined}
            onPointerDownCapture={() => setFocusedGroup(g.id)}
          >
            <header className="terminal-tabs" data-drop={`term-tabs:${g.id}`}>
              {gi === 0 && <span className="terminal-label">ターミナル</span>}
              {g.keys.map((key) => (
                <div
                  key={key}
                  className={key === g.active ? "ttab on" : "ttab"}
                  onPointerDown={(e) => beginTabDrag(e, titleOf(key), "term", (r) => onDrop(key, r))}
                  onClick={() => {
                    setGroups((gs) => gs.map((x) => (x.id === g.id ? { ...x, active: key } : x)));
                    focusKey(key);
                  }}
                  onMouseDown={(e) => {
                    if (e.button === 1) {
                      e.preventDefault();
                      close(key);
                    }
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    menu(key, e.clientX, e.clientY);
                  }}
                >
                  <span>{titleOf(key)}</span>
                  <button
                    className="close"
                    title="閉じる(シェルを終了)"
                    onClick={(e) => {
                      e.stopPropagation();
                      close(key);
                    }}
                  >
                    ×
                  </button>
                </div>
              ))}
              <button className="icon" title="新しいターミナル" onClick={() => add(g.id)}>
                ＋
              </button>
              <span className="spacer" />
              {gi === groups.length - 1 && (
                <>
                  {!split && (
                    <button className="icon" title="右に分割(新しいターミナル)" onClick={() => add("new")}>
                      ◫
                    </button>
                  )}
                  <button className="icon" title="パネルを隠す(Ctrl+J)。シェルは動いたまま" onClick={onHide}>
                    ⌄
                  </button>
                </>
              )}
            </header>
            <div className="terminal-body" data-drop={`term:${g.id}`} data-drop-split="">
              {g.keys.map((key) => {
                const s = sessions.find((x) => x.key === key);
                if (!s) return null;
                return (
                  <TermView
                    key={key}
                    ref={(h) => {
                      if (h) views.current.set(key, h);
                      else views.current.delete(key);
                    }}
                    attachId={s.id}
                    restore={s.restore ?? null}
                    shown={visible && key === g.active}
                    fontSize={fontSize}
                    onSpawned={(id, title) => setSessions((list) => list.map((x) => (x.key === key ? { ...x, id, title } : x)))}
                    onError={(m) => {
                      cb.current.onError(m);
                      setSessions((list) => list.filter((x) => x.key !== key));
                      setGroups((gs) => removeKey(gs, key));
                    }}
                  />
                );
              })}
            </div>
            {split && gi === 0 && <SplitDivider ratio={ratio} onChange={setRatio} />}
          </div>
        ))}
      </div>
    </section>
  );
});

/** グループから key を外し、空になったグループは消す(最後の 1 つは残す) */
function removeKey(gs: TGroup[], key: number): TGroup[] {
  const next = gs.map((g) => {
    if (!g.keys.includes(key)) return g;
    const i = g.keys.indexOf(key);
    const keys = g.keys.filter((k) => k !== key);
    return { ...g, keys, active: g.active === key ? (keys[Math.min(i, keys.length - 1)] ?? null) : g.active };
  });
  const nonEmpty = next.filter((g) => g.keys.length > 0);
  return nonEmpty.length > 0 ? nonEmpty : [next[0]];
}

// ---- 1 セッション分の xterm ----------------------------------------------------

export interface TermViewHandle {
  focus(): void;
  /** 今の画面をシリアライズ(別グループ / 別ウィンドウへ引き継ぐため) */
  capture(): TermRestore | null;
}

/** UTF-8 でのバイト数(Rust 側の通算位置と合わせるため) */
function utf8Len(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

interface TermViewProps {
  /** 既存のセッションに付ける場合はその ID(履歴を再生してから続きを表示)。null なら新しく起動 */
  attachId: number | null;
  /** 引き継ぐ画面(あれば Rust の生の履歴より優先) */
  restore?: TermRestore | null;
  shown: boolean;
  fontSize: number;
  onSpawned(id: number, title: string): void;
  onError(msg: string): void;
}

export const TermView = forwardRef<TermViewHandle, TermViewProps>(function TermView(
  { attachId, restore, shown, fontSize, onSpawned, onError },
  ref,
) {
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const pid = useRef<number | null>(null);
  const lastSize = useRef({ cols: 0, rows: 0 });
  const cb = useRef({ onSpawned, onError });
  cb.current = { onSpawned, onError };
  // マウント時の値だけ使う(付け替えは再マウントで行う)
  const initialAttach = useRef(attachId);
  const initialRestore = useRef(restore ?? null);
  const serializer = useRef<SerializeAddon | null>(null);
  /** この画面に反映済みの出力の通算位置 */
  const applied = useRef(0);

  useImperativeHandle(ref, () => ({
    focus: () => term.current?.focus(),
    capture: () => {
      const s = serializer.current;
      if (!s || pid.current === null) return null;
      try {
        const t = term.current;
        return { data: s.serialize({ scrollback: 3000 }), offset: applied.current, cols: t?.cols, rows: t?.rows };
      } catch {
        return null;
      }
    },
  }));

  const refit = () => {
    const t = term.current;
    const f = fit.current;
    const el = host.current;
    if (!t || !f || !el || el.offsetParent === null || el.clientWidth === 0) return;
    try {
      f.fit();
    } catch {
      return;
    }
    const { cols, rows } = t;
    if (pid.current !== null && (cols !== lastSize.current.cols || rows !== lastSize.current.rows)) {
      lastSize.current = { cols, rows };
      void api.termResize(pid.current, cols, rows).catch(() => {});
    }
  };

  useEffect(() => {
    subscribeOutput();
    const t = new Terminal({
      fontFamily: '"Cascadia Mono", "BIZ UDGothic", "Noto Sans Mono CJK JP", Consolas, monospace',
      fontSize,
      lineHeight: 1.15,
      cursorBlink: true,
      scrollback: 5000,
      allowProposedApi: true,
      theme: darkQuery.matches ? DARK : LIGHT,
    });
    const f = new FitAddon();
    t.loadAddon(f);
    const ser = new SerializeAddon();
    t.loadAddon(ser);
    serializer.current = ser;
    t.loadAddon(new WebLinksAddon((_e, uri) => void api.openUrl(uri).catch(() => {})));
    t.open(host.current!);
    term.current = t;
    fit.current = f;

    // アプリのショートカットとコピー / 貼り付け
    t.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (mod && !e.shiftKey && !e.altKey && k === "j") return false; // パネル開閉はアプリへ
      if (mod && e.shiftKey && ["d", "n", "f"].includes(k)) return false; // 今日のメモ / クイックメモ / 検索
      if (mod && (k === "," || k === "\\")) return false; // 設定 / 分割
      if (e.key === "F11") return false; // フルスクリーン切り替えはアプリへ
      if (mod && !e.shiftKey && k === "c" && t.hasSelection()) {
        void navigator.clipboard.writeText(t.getSelection()).catch(() => {});
        t.clearSelection();
        e.preventDefault();
        return false;
      }
      if (mod && !e.shiftKey && k === "v") return false; // ブラウザの貼り付け → xterm の paste 処理へ
      return true;
    });

    const onScheme = () => {
      t.options.theme = darkQuery.matches ? DARK : LIGHT;
    };
    darkQuery.addEventListener("change", onScheme);

    let disposed = false;
    const connect = (id: number, from: number) => {
      pid.current = id;
      // from より前の出力は再生済み
      applied.current = from;
      const write: Writer = (data, start) => {
        if (start < from) return;
        t.write(data);
        applied.current = Math.max(applied.current, start + utf8Len(data));
      };
      for (const chunk of pendingOut.get(id) ?? []) write(chunk.data, chunk.start);
      pendingOut.delete(id);
      writers.set(id, write);
      t.onData((data) => void api.termWrite(id, data).catch(() => {}));
    };

    requestAnimationFrame(async () => {
      if (disposed) return;
      try {
        f.fit();
      } catch {
        /* 非表示のまま */
      }
      const existing = initialAttach.current;
      const saved = initialRestore.current;
      try {
        if (existing !== null && saved) {
          // 引き継いだ画面を描いてから、その後の出力を Rust の履歴で埋めて続ける
          writers.delete(existing);
          if (saved.cols && saved.rows) t.resize(saved.cols, saved.rows);
          t.write(saved.data);
          const rest = await api.termSince(existing, saved.offset);
          if (disposed) return;
          t.write(rest.data);
          connect(existing, rest.end);
          lastSize.current = { cols: 0, rows: 0 };
          refit();
        } else if (existing !== null) {
          // 付け替え: 受信を一旦溜めてから履歴を再生し、続きを流す
          writers.delete(existing);
          const snap = await api.termSnapshot(existing);
          if (disposed) return;
          t.write(snap.data);
          connect(existing, snap.end);
          lastSize.current = { cols: 0, rows: 0 };
          refit();
        } else {
          const { id, shell } = await api.termSpawn(t.cols, t.rows);
          if (disposed) {
            void api.termKill(id);
            return;
          }
          lastSize.current = { cols: t.cols, rows: t.rows };
          connect(id, 0);
          cb.current.onSpawned(id, shell);
        }
      } catch (e) {
        cb.current.onError(`ターミナルを${existing !== null ? "表示" : "起動"}できません: ${errorMessage(e)}`);
      }
    });

    const ro = new ResizeObserver(() => refit());
    ro.observe(host.current!);
    return () => {
      disposed = true;
      ro.disconnect();
      darkQuery.removeEventListener("change", onScheme);
      if (pid.current !== null && writers.get(pid.current)) writers.delete(pid.current);
      t.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (term.current) term.current.options.fontSize = fontSize;
    refit();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fontSize]);

  // 表示されたらサイズを合わせてフォーカス
  useEffect(() => {
    if (!shown) return;
    requestAnimationFrame(() => {
      refit();
      term.current?.focus();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown]);

  return <div className="term-host" ref={host} hidden={!shown} />;
});
