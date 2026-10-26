// CodeMirror 6 のエディタ。EditorView は 1 つだけ作り、タブごとの EditorState を差し替える
// (タブ切り替えで undo 履歴やカーソル位置が保たれる)。
import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import { basicSetup } from "codemirror";
import { EditorView, keymap } from "@codemirror/view";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { markdown } from "@codemirror/lang-markdown";
import { languages } from "@codemirror/language-data";
import { oneDark } from "@codemirror/theme-one-dark";
import { indentWithTab } from "@codemirror/commands";
import { api, errorMessage } from "../api";
import type { EditorSettings, TextFile } from "../types";

interface Doc {
  /** rename 時に書き換えるため、パスはオブジェクトで持つ */
  key: { path: string };
  state: EditorState;
  meta: Omit<TextFile, "content">;
  savedText: string;
  timer?: number;
}

export interface EditorHandle {
  save(path: string): Promise<void>;
  saveAll(): Promise<void>;
  forget(path: string): void;
  rename(from: string, to: string): void;
  reload(path: string): Promise<void>;
  /**
   * このファイルを表示したときのカーソル位置を指定する。
   * "end" は末尾(日次ノートへの追記用)、数値はその行(検索結果からのジャンプ用)。
   * 既に表示中ならすぐ移動する。
   */
  placeCursor(path: string, target: CursorTarget): void;
  focus(): void;
}

export type CursorTarget = "end" | number;

interface Props {
  activePath: string | null;
  settings: EditorSettings;
  onDirtyChange(path: string, dirty: boolean): void;
  onError(message: string): void;
  /** 表示中ファイルの本文(読み込み時と編集のたび。Markdown プレビュー用) */
  onText?(path: string, text: string): void;
  /** 表示先頭の行番号(スクロールのたび。プレビューの追従用) */
  onTopLine?(path: string, line: number): void;
}

const themeC = new Compartment();
const fontC = new Compartment();
const wrapC = new Compartment();
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

const fontTheme = (size: number) =>
  EditorView.theme({
    "&": { fontSize: `${size}px`, height: "100%" },
    ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.65" },
    ".cm-content": { padding: "16px 0" },
    ".cm-line": { padding: "0 24px" },
  });

function languageFor(path: string): Extension {
  return /\.(md|markdown)$/i.test(path) ? markdown({ codeLanguages: languages }) : [];
}

export const Editor = forwardRef<EditorHandle, Props>(function Editor(
  { activePath, settings, onDirtyChange, onError, onText, onTopLine },
  ref,
) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const docs = useRef(new Map<string, Doc>());
  const active = useRef<string | null>(null);
  const pendingCursor = useRef(new Map<string, CursorTarget>());

  const applyCursor = (v: EditorView, target: CursorTarget) => {
    const doc = v.state.doc;
    const pos = target === "end" ? doc.length : doc.line(Math.min(Math.max(1, target), doc.lines)).from;
    v.dispatch({
      selection: { anchor: pos },
      effects: EditorView.scrollIntoView(pos, { y: target === "end" ? "nearest" : "start", yMargin: 48 }),
    });
    v.focus();
  };
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const cb = useRef({ onDirtyChange, onError, onText, onTopLine });
  cb.current = { onDirtyChange, onError, onText, onTopLine };

  const emitTopLine = () => {
    const v = view.current;
    const path = active.current;
    if (!v || !path || !cb.current.onTopLine) return;
    const block = v.lineBlockAtHeight(Math.max(0, v.scrollDOM.scrollTop - v.documentPadding.top));
    cb.current.onTopLine(path, v.state.doc.lineAt(block.from).number);
  };

  const save = async (path: string) => {
    const doc = docs.current.get(path);
    if (!doc) return;
    window.clearTimeout(doc.timer);
    // アクティブなら View の最新 state を取り込む
    if (active.current === path && view.current) doc.state = view.current.state;
    const text = doc.state.doc.toString();
    if (text === doc.savedText) return;
    try {
      await api.writeFile(path, { ...doc.meta, content: text });
      doc.savedText = text;
      const current = active.current === path && view.current ? view.current.state.doc.toString() : text;
      cb.current.onDirtyChange(path, current !== text);
    } catch (e) {
      cb.current.onError(`保存に失敗しました: ${errorMessage(e)}`);
    }
  };

  const extensions = (key: { path: string }): Extension[] => [
    basicSetup,
    keymap.of([indentWithTab]),
    languageFor(key.path),
    themeC.of(darkQuery.matches ? oneDark : []),
    fontC.of(fontTheme(settingsRef.current.font_size)),
    wrapC.of(settingsRef.current.line_wrapping ? EditorView.lineWrapping : []),
    EditorView.updateListener.of((u) => {
      const path = key.path;
      if (!u.docChanged || active.current !== path) return;
      const doc = docs.current.get(path);
      if (!doc) return;
      doc.state = u.state;
      const text = u.state.doc.toString();
      cb.current.onDirtyChange(path, text !== doc.savedText);
      cb.current.onText?.(path, text);
      const ms = settingsRef.current.autosave_ms;
      if (ms > 0) {
        window.clearTimeout(doc.timer);
        doc.timer = window.setTimeout(() => void save(path), ms);
      }
    }),
  ];

  const load = async (path: string): Promise<Doc> => {
    const file = await api.readFile(path);
    const { content, ...meta } = file;
    const key = { path };
    const doc: Doc = {
      key,
      state: EditorState.create({ doc: content, extensions: extensions(key) }),
      meta,
      savedText: content,
    };
    docs.current.set(path, doc);
    return doc;
  };

  // View の生成/破棄
  useEffect(() => {
    view.current = new EditorView({ parent: host.current! });
    const onScheme = () =>
      view.current?.dispatch({ effects: themeC.reconfigure(darkQuery.matches ? oneDark : []) });
    darkQuery.addEventListener("change", onScheme);
    let raf = 0;
    const onScroll = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(emitTopLine);
    };
    view.current.scrollDOM.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(raf);
      darkQuery.removeEventListener("change", onScheme);
      view.current?.destroy();
      view.current = null;
    };
  }, []);

  // タブ切り替え
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const prev = active.current;
    if (prev && docs.current.has(prev)) docs.current.get(prev)!.state = v.state;
    active.current = activePath;
    if (!activePath) return;
    let cancelled = false;
    (async () => {
      try {
        const doc = docs.current.get(activePath) ?? (await load(activePath));
        if (cancelled || active.current !== activePath) return;
        v.setState(doc.state);
        cb.current.onText?.(activePath, doc.state.doc.toString());
        const target = pendingCursor.current.get(activePath);
        if (target !== undefined) {
          pendingCursor.current.delete(activePath);
          applyCursor(v, target);
        }
        // 他タブで変えた設定を反映
        v.dispatch({
          effects: [
            themeC.reconfigure(darkQuery.matches ? oneDark : []),
            fontC.reconfigure(fontTheme(settingsRef.current.font_size)),
            wrapC.reconfigure(settingsRef.current.line_wrapping ? EditorView.lineWrapping : []),
          ],
        });
        v.focus();
        requestAnimationFrame(emitTopLine);
      } catch (e) {
        cb.current.onError(`開けませんでした: ${errorMessage(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activePath]);

  // 設定変更の反映
  useEffect(() => {
    view.current?.dispatch({
      effects: [
        fontC.reconfigure(fontTheme(settings.font_size)),
        wrapC.reconfigure(settings.line_wrapping ? EditorView.lineWrapping : []),
      ],
    });
  }, [settings.font_size, settings.line_wrapping]);

  useImperativeHandle(ref, () => ({
    save,
    async saveAll() {
      await Promise.all([...docs.current.keys()].map(save));
    },
    forget(path) {
      const doc = docs.current.get(path);
      if (doc) window.clearTimeout(doc.timer);
      docs.current.delete(path);
    },
    rename(from, to) {
      // フォルダごと rename された場合も配下のパスを付け替える
      for (const [p, doc] of [...docs.current]) {
        if (p === from || p.startsWith(`${from}/`)) {
          const next = to + p.slice(from.length);
          docs.current.delete(p);
          doc.key.path = next;
          docs.current.set(next, doc);
        }
      }
      if (active.current && (active.current === from || active.current.startsWith(`${from}/`))) {
        active.current = to + active.current.slice(from.length);
      }
    },
    placeCursor(path, target) {
      const v = view.current;
      if (v && active.current === path && docs.current.has(path)) applyCursor(v, target);
      else pendingCursor.current.set(path, target);
    },
    async reload(path) {
      docs.current.delete(path);
      if (active.current === path && view.current) {
        const doc = await load(path);
        view.current.setState(doc.state);
        cb.current.onText?.(path, doc.state.doc.toString());
      }
    },
    focus() {
      view.current?.focus();
    },
  }));

  return <div className="editor-host" ref={host} hidden={!activePath} />;
});
