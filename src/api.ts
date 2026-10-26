// Rust 側コマンドの型付きラッパー。invoke の引数名は Rust 側の引数名(camelCase 変換後)と一致させる。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettings,
  CategoryConfig,
  DirEntry,
  ChunkRow,
  IndexStatus,
  Provider,
  Saved,
  SearchResult,
  TextFile,
  WorkspaceInfo,
} from "./types";

export const api = {
  openWorkspace: (path: string) => invoke<WorkspaceInfo>("open_workspace", { path }),
  currentWorkspace: () => invoke<WorkspaceInfo | null>("current_workspace"),
  saveCategories: (config: CategoryConfig) => invoke<WorkspaceInfo>("save_categories", { config }),

  listDir: (path: string) => invoke<DirEntry[]>("list_dir", { path }),
  readFile: (path: string) => invoke<TextFile>("read_file", { path }),
  writeFile: (path: string, file: TextFile) => invoke<void>("write_file", { path, file }),
  createFile: (path: string) => invoke<string>("create_file", { path }),
  createDir: (path: string) => invoke<string>("create_dir", { path }),
  renamePath: (from: string, to: string) => invoke<string>("rename_path", { from, to }),
  trashPath: (path: string) => invoke<void>("trash_path", { path }),
  openExternal: (path: string) => invoke<void>("open_external", { path }),
  revealInFolder: (path: string) => invoke<void>("reveal_in_folder", { path }),

  openDailyNote: () => invoke<string>("open_daily_note"),
  saveToCategory: (categoryId: string, request: { title: string; content: string }) =>
    invoke<Saved>("save_to_category", { categoryId, request }),

  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) => invoke<AppSettings>("save_settings", { settings }),
  testProvider: (provider: Provider) => invoke<string[]>("test_provider", { provider }),

  indexStatus: () => invoke<IndexStatus>("index_status"),
  indexRescan: () => invoke<void>("index_rescan"),
  indexRebuildVectors: () => invoke<void>("index_rebuild_vectors"),
  search: (query: string, useLlm: boolean) => invoke<SearchResult>("search", { query, useLlm }),
  docChunks: (path: string) => invoke<ChunkRow[]>("doc_chunks", { path }),
  onIndexStatus: (cb: (s: IndexStatus) => void): Promise<UnlistenFn> =>
    listen<IndexStatus>("index-status", (e) => cb(e.payload)),

  openWorkspaceFolder: () => invoke<void>("open_workspace_folder"),
  openUrl: (url: string) => invoke<void>("open_url", { url }),

  termSince: (id: number, offset: number) => invoke<{ data: string; end: number }>("term_since", { id, offset }),
  termSnapshot: (id: number) => invoke<{ data: string; end: number }>("term_snapshot", { id }),
  termSpawn: (cols: number, rows: number) => invoke<{ id: number; shell: string }>("term_spawn", { cols, rows }),
  termWrite: (id: number, data: string) => invoke<void>("term_write", { id, data }),
  termResize: (id: number, cols: number, rows: number) => invoke<void>("term_resize", { id, cols, rows }),
  termKill: (id: number) => invoke<void>("term_kill", { id }),
  onTermOutput: (cb: (id: number, data: string, start: number) => void): Promise<UnlistenFn> =>
    listen<{ id: number; data: string; start: number }>("term-output", (e) => cb(e.payload.id, e.payload.data, e.payload.start)),

  /** タブを別ウィンドウに切り離す(戻り値はウィンドウのラベル) */
  detachWindow: (payload: unknown, title: string) => invoke<string>("detach_window", { payload, title }),
  /** このウィンドウのタブをメインに戻して、このウィンドウを閉じる */
  reattach: (payload: unknown) => invoke<void>("reattach", { payload }),
  closeDetachedWindows: () => invoke<void>("close_detached_windows"),
  focusWindow: (label: string) => invoke<void>("focus_window", { label }),
  onTermExit: (cb: (id: number, code: number | null) => void): Promise<UnlistenFn> =>
    listen<{ id: number; code: number | null }>("term-exit", (e) => cb(e.payload.id, e.payload.code)),
};

/** テキストとしてエディタで開ける種類(それ以外は既定のアプリで開く) */
export const isTextKind = (kind: string) => ["markdown", "text", "csv", "html"].includes(kind);

export function errorMessage(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export const parentOf = (path: string) => path.split("/").slice(0, -1).join("/");
export const baseName = (path: string) => path.split("/").pop() ?? path;
export const joinPath = (dir: string, name: string) => (dir ? `${dir}/${name}` : name);
