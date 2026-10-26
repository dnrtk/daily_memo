import { useCallback, useEffect, useRef, useState } from "react";
import { ask, open as openDialog } from "@tauri-apps/plugin-dialog";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, baseName, errorMessage, isTextKind, joinPath, parentOf } from "./api";
import type { AppSettings, DirEntry, FileHit, IndexStatus, WorkspaceInfo } from "./types";
import { isMarkdown, type DetachedPayload, type Group, type MdMode, type Tab, type TermRestore } from "./editorTypes";
import type { EditorHandle } from "./components/Editor";
import { EditorGroup } from "./components/EditorGroup";
import { PreviewPopup } from "./components/PreviewPopup";
import { TerminalPanel, type TerminalPanelHandle } from "./components/TerminalPanel";
import { FileTree } from "./components/FileTree";
import { ContextMenu, PromptDialog, type MenuItem, type PromptState } from "./components/Overlays";
import { QuickCapture } from "./components/QuickCapture";
import { SearchPanel } from "./components/SearchPanel";
import { SettingsDialog } from "./components/SettingsDialog";
import { AboutDialog } from "./components/AboutDialog";
import { SplitDivider } from "./components/SplitDivider";
import { dropGroupId, type DropResult } from "./components/tabDrag";

type SideView = "files" | "search";

interface Preview {
  file: FileHit;
  index: number;
  terms: string[];
}

const ACTIVITY_W = 44;
const SIDEBAR_DEFAULT = 280;
const SIDEBAR_MIN = 180;
/** サイドバーの最大幅(ウィンドウ幅に対する割合) */
const SIDEBAR_MAX_RATIO = 0.8;
const sidebarMax = (winW: number) => Math.max(SIDEBAR_MIN, Math.floor(winW * SIDEBAR_MAX_RATIO));

export function loadPref<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}

export function savePref(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage 不可でも動かす */
  }
}

/** パスがフォルダ from(またはそのファイル自身)の配下なら付け替える */
const remapPath = (p: string, from: string, to: string) => (p === from || p.startsWith(`${from}/`) ? to + p.slice(from.length) : p);

/** グループからタブを外す。外したことで空になったグループは(他にグループがあれば)消す */
function removeTab(gs: Group[], path: string): Group[] {
  let emptied: number | null = null;
  const next = gs.map((g) => {
    const i = g.tabs.findIndex((t) => t.path === path);
    if (i < 0) return g;
    const tabs = g.tabs.filter((t) => t.path !== path);
    if (tabs.length === 0) emptied = g.id;
    return { ...g, tabs, active: g.active === path ? (tabs[Math.min(i, tabs.length - 1)]?.path ?? null) : g.active };
  });
  return emptied !== null && next.length > 1 ? next.filter((g) => g.id !== emptied) : next;
}

export default function App() {
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [groups, setGroups] = useState<Group[]>([{ id: 1, tabs: [], active: null }]);
  const [focusedGroup, setFocusedGroup] = useState(1);
  const [splitRatio, setSplitRatio] = useState(() => loadPref("editor.split", 0.5));
  const [reloadKey, setReloadKey] = useState(0);
  const [sideView, setSideView] = useState<SideView>("files");
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [prompt, setPrompt] = useState<PromptState | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showCapture, setShowCapture] = useState(false);
  const [toast, setToast] = useState<{ msg: string; error?: boolean } | null>(null);
  const [indexStatus, setIndexStatus] = useState<IndexStatus | null>(null);
  const [searchFocus, setSearchFocus] = useState(0);
  const [showAbout, setShowAbout] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => loadPref("sidebar.open", true));
  const [sidebarWidth, setSidebarWidth] = useState(() => loadPref("sidebar.width", SIDEBAR_DEFAULT));
  const [resizing, setResizing] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [termOpen, setTermOpen] = useState(false);
  const [termMounted, setTermMounted] = useState(false);
  const [termHeight, setTermHeight] = useState(() => loadPref("terminal.height", 260));

  const editors = useRef(new Map<number, EditorHandle>());
  const terminal = useRef<TerminalPanelHandle>(null);
  const nextGroupId = useRef(2);
  /** 別ウィンドウに切り離したファイル → ウィンドウのラベル */
  const detached = useRef(new Map<string, string>());
  /** ターミナルパネルの準備前に戻ってきたターミナル */
  const pendingAttach = useRef<{ id: number; title: string; restore?: TermRestore | null }[]>([]);

  // 非同期処理から最新の状態を読むための参照
  const groupsRef = useRef(groups);
  groupsRef.current = groups;
  const focusedRef = useRef(focusedGroup);
  focusedRef.current = focusedGroup;

  useEffect(() => savePref("terminal.height", termHeight), [termHeight]);
  useEffect(() => savePref("sidebar.open", sidebarOpen), [sidebarOpen]);
  useEffect(() => savePref("sidebar.width", sidebarWidth), [sidebarWidth]);
  // ウィンドウを狭めたときは表示幅だけ詰める(保存した幅はそのまま)
  const [winW, setWinW] = useState(() => window.innerWidth);
  useEffect(() => {
    const h = () => setWinW(window.innerWidth);
    window.addEventListener("resize", h);
    return () => window.removeEventListener("resize", h);
  }, []);
  const sideW = Math.min(sidebarWidth, sidebarMax(winW));
  useEffect(() => savePref("editor.split", splitRatio), [splitRatio]);

  // 開いたらタブを用意してフォーカス(パネルのマウント後に実行)。戻ってきたターミナルもここで付ける
  useEffect(() => {
    if (!termOpen || !terminal.current) return;
    const queued = pendingAttach.current.splice(0);
    if (queued.length > 0) queued.forEach((q) => terminal.current?.attach(q.id, q.title, q.restore));
    else terminal.current.ensureSession();
  }, [termOpen, termMounted]);

  const toggleTerminal = () => {
    if (termOpen) {
      setTermOpen(false);
      return;
    }
    setTermMounted(true);
    setTermOpen(true);
  };

  const notify = useCallback((msg: string, error = false) => {
    setToast({ msg, error });
    window.setTimeout(() => setToast((t) => (t?.msg === msg ? null : t)), error ? 6000 : 2500);
  }, []);
  const onError = useCallback((msg: string) => notify(msg, true), [notify]);
  const refreshTree = () => setReloadKey((k) => k + 1);
  const openFolder = () => void api.openWorkspaceFolder().catch((e) => onError(errorMessage(e)));

  useEffect(() => {
    api.getSettings().then(setSettings).catch((e) => onError(errorMessage(e)));
    api.currentWorkspace().then(setWorkspace).catch((e) => onError(errorMessage(e)));
    api.indexStatus().then(setIndexStatus).catch(() => {});
    const un = api.onIndexStatus(setIndexStatus);
    return () => void un.then((f) => f());
  }, [onError]);

  // ---- エディタグループ ------------------------------------------------------------

  const groupIdOf = (path: string) => groupsRef.current.find((g) => g.tabs.some((t) => t.path === path))?.id ?? null;
  const editorOf = (gid: number | null) => (gid === null ? undefined : editors.current.get(gid));
  /** そのファイルを開くことになるグループ(既に開いていればそこ、なければフォーカス中) */
  const targetGroup = (path: string) => {
    const gs = groupsRef.current;
    return groupIdOf(path) ?? (gs.some((g) => g.id === focusedRef.current) ? focusedRef.current : gs[0].id);
  };
  const saveAll = async () => {
    await Promise.all([...editors.current.values()].map((e) => e.saveAll()));
  };

  /** タブを開く / 更新する。別ウィンドウで開いているファイルならそのウィンドウを前面に出す */
  const upsertTab = async (path: string, make: (existing?: Tab) => Tab) => {
    const label = detached.current.get(path);
    if (label) {
      try {
        await api.focusWindow(label);
        return;
      } catch {
        detached.current.delete(path);
      }
    }
    const gid = targetGroup(path);
    setGroups((gs) =>
      gs.map((g) => {
        if (g.id !== gid) return g;
        const i = g.tabs.findIndex((t) => t.path === path);
        const tabs = i < 0 ? [...g.tabs, make()] : g.tabs.map((t, j) => (j === i ? make(t) : t));
        return { ...g, tabs, active: path };
      }),
    );
    setFocusedGroup(gid);
  };

  const openTab = (path: string, extra?: Partial<Tab>) =>
    upsertTab(path, (t) => ({ ...(t ?? { path, dirty: false, kind: "text" as const }), ...extra }));

  /** Office / PDF を閲覧タブで開き、指定チャンクへスクロール */
  const openDocTab = (path: string, docKind: string, chunkId: number | null, terms: string[]) =>
    upsertTab(path, (t) =>
      t
        ? { ...t, focusChunk: chunkId, focusSeq: (t.focusSeq ?? 0) + 1, terms }
        : { path, dirty: false, kind: "doc", docKind, focusChunk: chunkId, focusSeq: 0, terms },
    );

  const closeTab = async (path: string) => {
    const ed = editorOf(groupIdOf(path));
    await ed?.save(path);
    ed?.forget(path);
    setGroups((gs) => removeTab(gs, path));
  };

  const activateTab = (gid: number, path: string) => {
    setGroups((gs) => gs.map((g) => (g.id === gid ? { ...g, active: path } : g)));
    setFocusedGroup(gid);
  };

  /** タブを別グループへ移す("new" なら右に新しいグループを作る) */
  const moveTab = async (path: string, to: number | "new") => {
    const gs = groupsRef.current;
    const from = groupIdOf(path);
    if (from === null) return;
    let target: number | "new" = to;
    if (target === "new" && gs.length >= 2) target = gs.find((g) => g.id !== from)!.id;
    if (target === from) return;
    const tab = gs.find((g) => g.id === from)!.tabs.find((t) => t.path === path)!;
    const ed = editorOf(from);
    await ed?.save(path);
    ed?.forget(path);
    const newId = target === "new" ? nextGroupId.current++ : target;
    setGroups((cur) => {
      let next = cur;
      if (target === "new") next = [...next, { id: newId, tabs: [], active: null }];
      next = removeTab(next, path);
      return next.map((g) => (g.id === newId ? { ...g, tabs: [...g.tabs, { ...tab, dirty: false }], active: path } : g));
    });
    setFocusedGroup(newId);
  };

  /** 右に分割(Ctrl+\)。2 グループなら反対側へ移動。タブが 1 つなら空のグループを右に作る */
  const splitEditor = () => {
    const gs = groupsRef.current;
    const g = gs.find((x) => x.id === focusedRef.current) ?? gs[0];
    if (gs.length === 1 && g.tabs.length <= 1) {
      const id = nextGroupId.current++;
      setGroups((cur) => [...cur, { id, tabs: [], active: null }]);
      setFocusedGroup(id);
      return;
    }
    if (g.active) void moveTab(g.active, "new");
  };

  const closeGroup = (gid: number) => {
    setGroups((gs) => (gs.length > 1 ? gs.filter((g) => g.id !== gid || g.tabs.length > 0) : gs));
    setFocusedGroup(groupsRef.current.find((g) => g.id !== gid)?.id ?? gid);
  };

  /** タブを別ウィンドウに切り離す */
  const detachTab = async (path: string) => {
    const gid = groupIdOf(path);
    const tab = groupsRef.current.find((g) => g.id === gid)?.tabs.find((t) => t.path === path);
    if (!tab) return;
    const ed = editorOf(gid);
    await ed?.save(path);
    try {
      const payload: DetachedPayload = { kind: "editor", tab: { ...tab, dirty: false } };
      const label = await api.detachWindow(payload, baseName(path));
      ed?.forget(path);
      setGroups((gs) => removeTab(gs, path));
      detached.current.set(path, label);
    } catch (e) {
      onError(`切り離せませんでした: ${errorMessage(e)}`);
    }
  };

  const onTabDrop = (path: string, r: DropResult) => {
    if (r.outside) {
      void detachTab(path);
      return;
    }
    const gid = dropGroupId(r.target);
    if (gid === null) return;
    if (r.split && r.target?.startsWith("editor:")) {
      const gs = groupsRef.current;
      void moveTab(path, gs.length === 1 ? "new" : gs[1].id);
      return;
    }
    void moveTab(path, gid);
  };

  const tabMenu = (path: string, x: number, y: number) => {
    const gs = groupsRef.current;
    const gid = groupIdOf(path);
    const g = gs.find((x2) => x2.id === gid);
    const other = gs.find((x2) => x2.id !== gid);
    const items: MenuItem[] = [];
    if (other) {
      items.push({ label: gs[0].id === gid ? "右のグループへ移動" : "左のグループへ移動", onSelect: () => void moveTab(path, other.id) });
    } else if ((g?.tabs.length ?? 0) > 1) {
      items.push({ label: "右に分割して移動(Ctrl+\\)", onSelect: () => void moveTab(path, "new") });
    }
    items.push({ label: "別ウィンドウに切り離す", onSelect: () => void detachTab(path) });
    const tab = g?.tabs.find((t) => t.path === path);
    if (tab?.kind === "doc") {
      items.push({ label: "既定のアプリで開く", onSelect: () => void api.openExternal(path).catch((e) => onError(errorMessage(e))) });
    }
    items.push({ label: "フォルダで表示", onSelect: () => void api.revealInFolder(path).catch((e) => onError(errorMessage(e))) });
    items.push({ label: "閉じる", separatorBefore: true, onSelect: () => void closeTab(path) });
    setMenu({ x, y, items });
  };

  const setMdMode = (path: string, mode: MdMode) =>
    setGroups((gs) => gs.map((g) => ({ ...g, tabs: g.tabs.map((t) => (t.path === path ? { ...t, mdMode: mode } : t)) })));

  const onDirtyChange = useCallback((path: string, dirty: boolean) => {
    setGroups((gs) =>
      gs.map((g) =>
        g.tabs.some((t) => t.path === path && t.dirty !== dirty)
          ? { ...g, tabs: g.tabs.map((t) => (t.path === path ? { ...t, dirty } : t)) }
          : g,
      ),
    );
  }, []);

  /** パスの付け替え(rename・カテゴリのフォルダ変更)をタブ・エディタ・別ウィンドウへ反映 */
  const applyRename = (from: string, to: string) => {
    editors.current.forEach((e) => e.rename(from, to));
    setGroups((gs) =>
      gs.map((g) => ({
        ...g,
        tabs: g.tabs.map((t) => ({ ...t, path: remapPath(t.path, from, to) })),
        active: g.active ? remapPath(g.active, from, to) : g.active,
      })),
    );
    for (const [p, label] of [...detached.current]) {
      const np = remapPath(p, from, to);
      if (np !== p) {
        detached.current.delete(p);
        detached.current.set(np, label);
      }
    }
    void emit("paths-moved", { from, to });
  };

  const openEntry = async (e: DirEntry) => {
    if (e.kind === "text") void openTab(e.path);
    else {
      try {
        await api.openExternal(e.path);
      } catch (err) {
        onError(errorMessage(err));
      }
    }
  };

  const openSearch = () => {
    setSideView("search");
    setSidebarOpen(true);
    setSearchFocus((n) => n + 1);
  };

  /** アクティビティバー: 表示中のビューのアイコンなら閉じる、それ以外は切り替えて開く */
  const toggleView = (v: SideView) => {
    if (sidebarOpen && sideView === v) {
      setSidebarOpen(false);
      return;
    }
    setSidebarOpen(true);
    setSideView(v);
    if (v === "search") setSearchFocus((n) => n + 1);
  };

  /** 検索結果をメインウィンドウで開く。テキストはエディタの該当行へ、Office / PDF は閲覧タブの該当箇所へ */
  const openHit = (f: FileHit, i: number, terms: string[]) => {
    const h = f.hits[i];
    if (isTextKind(f.kind)) {
      if (h?.line) editorOf(targetGroup(f.path))?.placeCursor(f.path, h.line);
      void openTab(f.path);
    } else {
      void openDocTab(f.path, f.kind, h?.chunk_id ?? null, terms);
    }
  };

  // ---- 別ウィンドウからの通知 ------------------------------------------------------

  const handlers = useRef({ openTab, notify });
  handlers.current = { openTab, notify };
  useEffect(() => {
    const subs = [
      listen<DetachedPayload>("reattach", (e) => {
        const p = e.payload;
        if (p.kind === "editor") {
          detached.current.delete(p.tab.path);
          void handlers.current.openTab(p.tab.path, { ...p.tab, dirty: false });
        } else {
          pendingAttach.current.push({ id: p.id, title: p.title, restore: p.restore });
          setTermMounted(true);
          setTermOpen((open) => {
            // 既に開いていれば effect が走らないので、ここで付ける
            if (open && terminal.current) {
              const queued = pendingAttach.current.splice(0);
              queued.forEach((q) => terminal.current?.attach(q.id, q.title, q.restore));
            }
            return true;
          });
        }
      }),
      listen<{ path: string }>("detached-closed", (e) => detached.current.delete(e.payload.path)),
      listen<{ path: string }>("open-file", (e) => void handlers.current.openTab(e.payload.path)),
    ];
    // メインを閉じる前に保存(切り離したウィンドウは Rust 側でそれぞれ閉じる)
    const close = getCurrentWindow().onCloseRequested(async () => {
      await Promise.all([...editors.current.values()].map((e) => e.saveAll()));
    });
    return () => {
      subs.forEach((s) => void s.then((f) => f()));
      void close.then((f) => f());
    };
  }, []);

  // ---- サイドバーの幅 --------------------------------------------------------------

  const startResize = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    window.getSelection()?.removeAllRanges();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    setResizing(true);
    document.body.classList.add("dragging", "col-resize");
  };
  const doResize = (e: React.PointerEvent) => {
    if (!resizing) return;
    const max = sidebarMax(window.innerWidth);
    setSidebarWidth(Math.round(Math.min(max, Math.max(SIDEBAR_MIN, e.clientX - ACTIVITY_W))));
  };
  const endResize = () => {
    if (!resizing) return;
    setResizing(false);
    document.body.classList.remove("dragging", "col-resize");
  };

  // ---- 作業フォルダ ----------------------------------------------------------------

  const chooseWorkspace = async () => {
    const dir = await openDialog({ directory: true, multiple: false, title: "作業フォルダを選択" });
    if (typeof dir !== "string") return;
    await saveAll();
    try {
      const ws = await api.openWorkspace(dir);
      await api.closeDetachedWindows();
      detached.current.clear();
      editors.current.forEach((ed) => groupsRef.current.forEach((g) => g.tabs.forEach((t) => ed.forget(t.path))));
      const id = nextGroupId.current++;
      setGroups([{ id, tabs: [], active: null }]);
      setFocusedGroup(id);
      setWorkspace(ws);
      refreshTree();
    } catch (e) {
      onError(errorMessage(e));
    }
  };

  const openDaily = async () => {
    try {
      const path = await api.openDailyNote();
      refreshTree();
      editorOf(targetGroup(path))?.placeCursor(path, "end");
      void openTab(path);
    } catch (e) {
      onError(errorMessage(e));
    }
  };

  // ---- ファイル操作 ----------------------------------------------------------------

  const newFile = (dir: string) =>
    setPrompt({
      title: `新しいファイル(${dir || "ルート"})`,
      initial: "untitled.md",
      selectUntil: "untitled".length,
      onSubmit: async (name) => {
        try {
          const path = await api.createFile(joinPath(dir, name.includes(".") ? name : `${name}.md`));
          refreshTree();
          void openTab(path);
        } catch (e) {
          onError(errorMessage(e));
        }
      },
    });

  const newFolder = (dir: string) =>
    setPrompt({
      title: `新しいフォルダ(${dir || "ルート"})`,
      initial: "",
      onSubmit: async (name) => {
        try {
          await api.createDir(joinPath(dir, name));
          refreshTree();
        } catch (e) {
          onError(errorMessage(e));
        }
      },
    });

  const rename = (e: DirEntry) => {
    const dot = e.is_dir ? -1 : e.name.lastIndexOf(".");
    setPrompt({
      title: "名前を変更",
      initial: e.name,
      selectUntil: dot > 0 ? dot : e.name.length,
      onSubmit: async (name) => {
        if (name === e.name) return;
        await saveAll();
        try {
          const to = await api.renamePath(e.path, joinPath(parentOf(e.path), name));
          applyRename(e.path, to);
          refreshTree();
        } catch (err) {
          onError(errorMessage(err));
        }
      },
    });
  };

  const trash = async (e: DirEntry) => {
    const ok = await ask(`「${e.name}」をゴミ箱に移動しますか?`, { title: "削除", kind: "warning" });
    if (!ok) return;
    try {
      await api.trashPath(e.path);
      const gone = (p: string) => p === e.path || p.startsWith(`${e.path}/`);
      groupsRef.current.forEach((g) => g.tabs.filter((t) => gone(t.path)).forEach((t) => editorOf(g.id)?.forget(t.path)));
      setGroups((gs) => {
        let next = gs;
        gs.forEach((g) => g.tabs.filter((t) => gone(t.path)).forEach((t) => (next = removeTab(next, t.path))));
        return next;
      });
      for (const p of [...detached.current.keys()]) if (gone(p)) detached.current.delete(p);
      void emit("paths-removed", { path: e.path });
      refreshTree();
    } catch (err) {
      onError(errorMessage(err));
    }
  };

  const showMenu = (e: DirEntry | null, x: number, y: number) => {
    const dir = e ? (e.is_dir ? e.path : parentOf(e.path)) : "";
    const items: MenuItem[] = [
      { label: "新しいファイル", onSelect: () => newFile(dir) },
      { label: "新しいフォルダ", onSelect: () => newFolder(dir) },
    ];
    if (e) {
      if (!e.is_dir && e.kind !== "text")
        items.unshift({ label: "既定のアプリで開く", onSelect: () => void api.openExternal(e.path).catch((er) => onError(errorMessage(er))) });
      if (!e.is_dir && e.kind === "text") items.unshift({ label: "別ウィンドウで開く", onSelect: () => void openDetached(e.path) });
      items.push({
        label: "フォルダで表示",
        separatorBefore: true,
        onSelect: () => void api.revealInFolder(e.path).catch((er) => onError(errorMessage(er))),
      });
      if (!e.category_id) {
        items.push({ label: "名前を変更", onSelect: () => rename(e) });
        items.push({ label: "ゴミ箱へ移動", danger: true, onSelect: () => void trash(e) });
      }
    }
    setMenu({ x, y, items });
  };

  /** ファイルツリーから直接別ウィンドウで開く */
  const openDetached = async (path: string) => {
    if (groupIdOf(path) !== null) {
      await detachTab(path);
      return;
    }
    const label = detached.current.get(path);
    if (label) {
      void api.focusWindow(label).catch(() => {});
      return;
    }
    try {
      const tab: Tab = { path, dirty: false, kind: "text" };
      detached.current.set(path, await api.detachWindow({ kind: "editor", tab } satisfies DetachedPayload, baseName(path)));
    } catch (err) {
      onError(errorMessage(err));
    }
  };

  const openCapture = async () => {
    // クイックメモの追記先を開いている場合に備えて、先に保存しておく
    await saveAll();
    setShowCapture(true);
  };

  /** 設定画面でカテゴリのフォルダが変わったら、開いているタブのパスを付け替える */
  const onWorkspaceSaved = (ws: WorkspaceInfo) => {
    const prev = workspace?.categories.categories ?? [];
    for (const c of ws.categories.categories) {
      const old = prev.find((p) => p.id === c.id);
      if (!old || old.dir === c.dir) continue;
      applyRename(old.dir.replace(/\/+$/, ""), c.dir.replace(/\/+$/, ""));
    }
    setWorkspace(ws);
    refreshTree();
  };

  // ---- ショートカット --------------------------------------------------------------

  const focusedTabs = groups.find((g) => g.id === focusedGroup) ?? groups[0];
  const active = focusedTabs.active;
  const activeTab = focusedTabs.tabs.find((t) => t.path === active);

  const keyHandler = useRef<(e: KeyboardEvent) => void>(() => {});
  keyHandler.current = (e: KeyboardEvent) => {
    if (e.key === "Escape" && preview && !showSettings && !showCapture && !showAbout && !prompt) {
      setPreview(null);
      return;
    }
    const mod = e.ctrlKey || e.metaKey;
    if (!mod) return;
    const k = e.key.toLowerCase();
    if (k === "b" && !e.shiftKey) {
      e.preventDefault();
      setSidebarOpen((o) => !o);
    } else if (k === "j" && !e.shiftKey) {
      e.preventDefault();
      toggleTerminal();
    } else if (k === "e" && !e.shiftKey && workspace) {
      e.preventDefault();
      openFolder();
    } else if (k === "\\") {
      e.preventDefault();
      splitEditor();
    } else if (k === "v" && e.shiftKey && activeTab && isMarkdown(activeTab.path)) {
      e.preventDefault();
      setMdMode(activeTab.path, (activeTab.mdMode ?? "edit") === "edit" ? "split" : "edit");
    } else if (k === "s" && !e.shiftKey && active) {
      e.preventDefault();
      void editorOf(focusedGroup)?.save(active);
    } else if (k === "w" && !e.shiftKey) {
      e.preventDefault();
      if (active) void closeTab(active);
      else if (groups.length > 1) closeGroup(focusedGroup);
    } else if (k === "o" && !e.shiftKey) {
      e.preventDefault();
      void chooseWorkspace();
    } else if (k === "d" && e.shiftKey && workspace) {
      e.preventDefault();
      void openDaily();
    } else if (k === "n" && e.shiftKey && workspace) {
      e.preventDefault();
      void openCapture();
    } else if (k === "f" && e.shiftKey && workspace) {
      e.preventDefault();
      openSearch();
    } else if (k === ",") {
      e.preventDefault();
      setShowSettings(true);
    }
  };
  useEffect(() => {
    const h = (e: KeyboardEvent) => keyHandler.current(e);
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);

  // ---- 描画 ----------------------------------------------------------------------

  const showPreview = preview && sidebarOpen && sideView === "search";
  const split = groups.length === 2;
  const allEmpty = groups.every((g) => g.tabs.length === 0);

  const welcome = (
    <div className="welcome">
      <h1>memo-editor</h1>
      <dl>
        <dt>Ctrl+Shift+D</dt><dd>今日のメモ</dd>
        <dt>Ctrl+Shift+N</dt><dd>クイックメモ</dd>
        <dt>Ctrl+Shift+F</dt><dd>自然言語検索</dd>
        <dt>Ctrl+B</dt><dd>サイドバーの表示 / 非表示</dd>
        <dt>Ctrl+J</dt><dd>ターミナルの表示 / 非表示</dd>
        <dt>Ctrl+\</dt><dd>エディタを左右に分割</dd>
        <dt>Ctrl+Shift+V</dt><dd>Markdown プレビュー</dd>
        <dt>Ctrl+E</dt><dd>作業フォルダをエクスプローラーで開く</dd>
        <dt>Ctrl+O</dt><dd>作業フォルダを開く</dd>
        <dt>Ctrl+,</dt><dd>設定</dd>
        <dt>F11</dt><dd>フルスクリーン表示 / 解除</dd>
      </dl>
    </div>
  );

  return (
    <div
      className="app"
      style={{ gridTemplateColumns: sidebarOpen ? `${ACTIVITY_W}px ${sideW}px minmax(0, 1fr)` : `${ACTIVITY_W}px minmax(0, 1fr)` }}
    >
      <nav className="activity" aria-label="ビュー切り替え">
        <button
          className={sidebarOpen && sideView === "files" ? "on" : ""}
          title="ファイル(クリックで表示 / 非表示、Ctrl+B)"
          onClick={() => toggleView("files")}
        >
          <svg viewBox="0 0 24 24" aria-hidden><path d="M3 6h7l2 2h9v11H3z" /></svg>
        </button>
        <button
          className={sidebarOpen && sideView === "search" ? "on" : ""}
          title="自然言語検索(Ctrl+Shift+F。もう一度クリックで非表示)"
          onClick={() => toggleView("search")}
        >
          <svg viewBox="0 0 24 24" aria-hidden><circle cx="10.5" cy="10.5" r="6" /><path d="m15 15 6 6" /></svg>
        </button>
        <span className="spacer" />
        <button title="このアプリについて・ライセンス" onClick={() => setShowAbout(true)}>
          <svg viewBox="0 0 24 24" aria-hidden><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7.5v.5" /></svg>
        </button>
        <button title="設定 (Ctrl+,)" onClick={() => setShowSettings(true)}>
          <svg viewBox="0 0 24 24" aria-hidden><circle cx="12" cy="12" r="3" /><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9 7 7M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1" /></svg>
        </button>
      </nav>

      <aside className="sidebar" hidden={!sidebarOpen}>
        <div className="side-view" hidden={sideView !== "files"}>
          <header className="side-head">
            <span className="ws-name" title={workspace?.root}>
              {workspace?.name ?? "フォルダ未選択"}
            </span>
            {workspace && (
              <>
                <button className="icon" title="エクスプローラーで開く(Ctrl+E)" onClick={openFolder}>↗</button>
                <button className="icon" title="新しいファイル" onClick={() => newFile("")}>＋</button>
                <button className="icon" title="再読み込み" onClick={refreshTree}>↻</button>
              </>
            )}
          </header>
          {workspace ? (
            <>
              <div className="side-actions">
                <button onClick={() => void openDaily()} title="Ctrl+Shift+D">今日のメモ</button>
                <button onClick={() => void openCapture()} title="Ctrl+Shift+N">クイックメモ</button>
              </div>
              <FileTree
                categories={workspace.categories}
                activePath={active}
                reloadKey={reloadKey}
                onOpen={(e) => void openEntry(e)}
                onContextMenu={showMenu}
                onError={onError}
              />
            </>
          ) : (
            <div className="side-empty">
              <button className="primary" onClick={() => void chooseWorkspace()}>
                作業フォルダを開く
              </button>
            </div>
          )}
        </div>
        <div className="side-view" hidden={sideView !== "search"}>
          <header className="side-head">
            <span className="ws-name">自然言語検索</span>
          </header>
          {workspace && settings ? (
            <SearchPanel
              status={indexStatus}
              categories={workspace.categories}
              llmDefault={settings.search.use_llm_query}
              focusKey={searchFocus}
              selected={preview ? { path: preview.file.path, index: preview.index } : null}
              onSelect={(f, i, terms) => setPreview(f ? { file: f, index: i, terms } : null)}
              onOpenHit={openHit}
              onError={onError}
            />
          ) : (
            <div className="side-empty">作業フォルダを開くと検索できます</div>
          )}
        </div>
        <div
          className={resizing ? "side-resizer on" : "side-resizer"}
          title="ドラッグで幅を変更(ダブルクリックで元に戻す)"
          onMouseDown={(e) => e.preventDefault()}
          onPointerDown={startResize}
          onPointerMove={doResize}
          onPointerUp={endResize}
          onPointerCancel={endResize}
          onDoubleClick={() => setSidebarWidth(SIDEBAR_DEFAULT)}
        />
      </aside>

      <main className="main">
        <div className="editor-groups">
          {settings &&
            groups.map((g, gi) => (
              <div
                key={g.id}
                className="egroup-wrap"
                style={split ? { flexBasis: `${(gi === 0 ? splitRatio : 1 - splitRatio) * 100}%` } : undefined}
              >
                <EditorGroup
                  ref={(h) => {
                    if (h) editors.current.set(g.id, h);
                    else editors.current.delete(g.id);
                  }}
                  group={g}
                  focused={split && g.id === focusedGroup}
                  settings={settings.editor}
                  root={workspace?.root ?? ""}
                  placeholder={
                    allEmpty && !split ? (
                      welcome
                    ) : (
                      <div className="group-empty">
                        <p>ファイルを開くか、タブをここへドラッグ</p>
                        {split && <button onClick={() => closeGroup(g.id)}>このグループを閉じる</button>}
                      </div>
                    )
                  }
                  actions={
                    gi === groups.length - 1 && !split ? (
                      <button className="icon" title="右に分割(Ctrl+\)" onClick={splitEditor}>
                        ◫
                      </button>
                    ) : undefined
                  }
                  onFocus={() => setFocusedGroup(g.id)}
                  onActivate={(p) => activateTab(g.id, p)}
                  onClose={(p) => void closeTab(p)}
                  onDirtyChange={onDirtyChange}
                  onSetMode={setMdMode}
                  onTabMenu={tabMenu}
                  onTabDrop={onTabDrop}
                  onOpenFile={(p) => void openTab(p)}
                  onError={onError}
                />
                {split && gi === 0 && <SplitDivider ratio={splitRatio} onChange={setSplitRatio} />}
              </div>
            ))}
        </div>
        {termMounted && (
          <TerminalPanel
            ref={terminal}
            visible={termOpen}
            height={termHeight}
            fontSize={Math.max(11, (settings?.editor.font_size ?? 14) - 1)}
            onHeightChange={setTermHeight}
            onHide={() => setTermOpen(false)}
            onEmpty={() => setTermOpen(false)}
            onError={onError}
            onMenu={(x, y, items) => setMenu({ x, y, items })}
          />
        )}
        <footer className="statusbar">
          <button className={termOpen ? "status-btn on" : "status-btn"} title="ターミナル(Ctrl+J)" onClick={toggleTerminal}>
            ターミナル
          </button>
          <span>{active ?? ""}</span>
          <span className="spacer" />
          {activeTab?.kind === "text" && <span>{activeTab.dirty ? "未保存" : "保存済み"}</span>}
          {activeTab?.kind === "doc" && <span>読み取り専用</span>}
          {indexStatus && indexStatus.progress.phase !== "idle" && (
            <span className="busy-dot">
              {indexStatus.progress.phase === "embedding" ? "ベクトル作成中" : "インデックス更新中"}
              {indexStatus.progress.total > 0 && ` ${indexStatus.progress.done}/${indexStatus.progress.total}`}
            </span>
          )}
          {settings && <span title="LLM">{settings.roles.chat.model} / {settings.roles.embedding.model}</span>}
        </footer>
      </main>

      {showPreview && (
        <PreviewPopup
          file={preview.file}
          index={preview.index}
          terms={preview.terms}
          defaultLeft={ACTIVITY_W + sideW + 12}
          onOpen={() => openHit(preview.file, preview.index, preview.terms)}
          onClose={() => setPreview(null)}
        />
      )}
      {menu && <ContextMenu {...menu} onClose={() => setMenu(null)} />}
      {prompt && <PromptDialog state={prompt} onClose={() => setPrompt(null)} />}
      {showCapture && workspace && (
        <QuickCapture
          categories={workspace.categories}
          onClose={() => setShowCapture(false)}
          onSaved={(s) => {
            notify(`${s.path} に${s.appended ? "追記" : "保存"}しました`);
            refreshTree();
            editors.current.forEach((ed) => void ed.reload(s.path));
            void emit("file-changed", { path: s.path });
          }}
        />
      )}
      {showSettings && settings && (
        <SettingsDialog
          workspace={workspace}
          settings={settings}
          onClose={() => setShowSettings(false)}
          onWorkspaceSaved={onWorkspaceSaved}
          onSettingsSaved={(s) => {
            setSettings(s);
            void emit("settings-changed", s);
          }}
        />
      )}
      {showAbout && <AboutDialog onClose={() => setShowAbout(false)} />}
      {toast && (
        <div className={toast.error ? "toast error" : "toast"} role="status">
          {toast.msg}
        </div>
      )}
    </div>
  );
}
