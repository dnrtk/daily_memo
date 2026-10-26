// 遅延展開のファイルツリー。展開済みフォルダだけ子要素を取得する。
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { api, errorMessage } from "../api";
import type { CategoryConfig, DirEntry } from "../types";

interface Props {
  categories: CategoryConfig;
  activePath: string | null;
  reloadKey: number;
  onOpen(entry: DirEntry): void;
  onContextMenu(entry: DirEntry | null, x: number, y: number): void;
  onError(msg: string): void;
}

export function FileTree({ categories, activePath, reloadKey, onOpen, onContextMenu, onError }: Props) {
  const [children, setChildren] = useState<Map<string, DirEntry[]>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set([""]));

  const fetchDir = useCallback(
    async (path: string) => {
      try {
        const list = await api.listDir(path);
        setChildren((m) => new Map(m).set(path, list));
      } catch (e) {
        // 削除・rename 済みのフォルダは展開状態から外す
        setExpanded((s) => {
          const n = new Set(s);
          n.delete(path);
          return n;
        });
        if (path === "") onError(errorMessage(e));
      }
    },
    [onError],
  );

  // reloadKey が変わったら展開中のフォルダを全部取り直す
  useEffect(() => {
    expanded.forEach((p) => void fetchDir(p));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reloadKey, fetchDir]);

  // アクティブなファイルの親フォルダを自動展開
  useEffect(() => {
    if (!activePath) return;
    const parts = activePath.split("/").slice(0, -1);
    const ancestors = parts.map((_, i) => parts.slice(0, i + 1).join("/"));
    const missing = ancestors.filter((a) => !expanded.has(a));
    if (missing.length === 0) return;
    setExpanded((s) => new Set([...s, ...missing]));
    missing.forEach((p) => void fetchDir(p));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activePath]);

  const toggle = (entry: DirEntry) => {
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(entry.path)) n.delete(entry.path);
      else {
        n.add(entry.path);
        void fetchDir(entry.path);
      }
      return n;
    });
  };

  const categoryName = (id: string | null) =>
    id ? categories.categories.find((c) => c.id === id) : undefined;

  const render = (path: string, depth: number): ReactNode =>
    (children.get(path) ?? []).map((e) => {
      const cat = categoryName(e.category_id);
      const open = expanded.has(e.path);
      return (
        <div key={e.path} role="treeitem" aria-expanded={e.is_dir ? open : undefined}>
          <button
            className={[
              "tree-row",
              e.path === activePath ? "active" : "",
              cat ? "category" : "",
              e.kind === "document" ? "doc" : "",
            ].join(" ")}
            style={{ paddingLeft: 8 + depth * 14 }}
            title={cat?.description || e.path}
            onClick={() => (e.is_dir ? toggle(e) : onOpen(e))}
            onContextMenu={(ev) => {
              ev.preventDefault();
              ev.stopPropagation();
              onContextMenu(e, ev.clientX, ev.clientY);
            }}
          >
            <span className="tree-icon" aria-hidden>
              {e.is_dir ? (open ? "▾" : "▸") : fileGlyph(e)}
            </span>
            <span className="tree-name">{cat ? cat.name : e.name}</span>
            {cat && cat.name !== e.name && <span className="tree-hint">{e.name}</span>}
          </button>
          {e.is_dir && open && render(e.path, depth + 1)}
        </div>
      );
    });

  return (
    <div
      className="tree"
      role="tree"
      onContextMenu={(ev) => {
        ev.preventDefault();
        onContextMenu(null, ev.clientX, ev.clientY);
      }}
    >
      {render("", 0)}
    </div>
  );
}

function fileGlyph(e: DirEntry): string {
  const ext = e.name.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "md" || ext === "markdown") return "M";
  if (ext === "pdf") return "P";
  if (ext.startsWith("xls") || ext === "csv" || ext === "tsv") return "X";
  if (ext.startsWith("doc")) return "W";
  if (ext.startsWith("ppt")) return "S";
  return "·";
}
