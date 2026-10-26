// Markdown プレビュー。
// - ブロック単位(見出し・段落・リスト・表…)で描画し、各ブロックに元の行番号を持たせてエディタのスクロールに追従する
// - 作業フォルダ内の相対パス画像は asset プロトコル経由で表示
// - リンク: http(s) はブラウザ、相対パスのファイルはエディタで開く
// - HTML は DOMPurify で無害化
import { memo, useLayoutEffect, useMemo, useRef } from "react";
import { lexer, parser, walkTokens, type Token, type TokensList } from "marked";
import DOMPurify from "dompurify";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api, parentOf } from "../api";

interface Props {
  text: string;
  /** 作業フォルダ相対のファイルパス(相対リンク・画像の基準) */
  path: string;
  /** 作業フォルダの絶対パス */
  root: string;
  /** エディタの表示先頭行(この行のブロックまでスクロールする) */
  topLine: number | null;
  onOpenFile(relPath: string): void;
}

const OPTIONS = { gfm: true, breaks: true } as const;

/** DOMPurify の既定に asset:(Linux / macOS での作業フォルダ画像)を足したもの */
const SAFE_URI = /^(?:(?:https?|mailto|tel|asset):|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i;

/** a/b/../c → a/c(作業フォルダの外に出るものは null) */
export function normalizeRel(path: string): string | null {
  const out: string[] = [];
  for (const seg of path.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (out.length === 0) return null;
      out.pop();
    } else out.push(seg);
  }
  return out.join("/");
}

const isExternal = (href: string) => /^[a-z][a-z0-9+.-]*:/i.test(href);

function resolveRel(base: string, href: string): string | null {
  let h = href.split("#")[0].split("?")[0];
  try {
    h = decodeURI(h);
  } catch {
    /* そのまま */
  }
  if (!h) return null;
  return normalizeRel(h.startsWith("/") ? h : `${parentOf(base)}/${h}`);
}

function toAssetUrl(root: string, rel: string): string {
  const sep = root.includes("\\") ? "\\" : "/";
  const abs = `${root.replace(/[\\/]+$/, "")}${sep}${rel.split("/").join(sep)}`;
  return convertFileSrc(abs);
}

interface Block {
  line: number;
  html: string;
}

function renderBlocks(text: string, path: string, root: string, cache: Map<string, string>): Block[] {
  const tokens = lexer(text, OPTIONS);
  const blocks: Block[] = [];
  let line = 1;
  for (const tok of tokens) {
    const startLine = line;
    line += (tok.raw.match(/\n/g) ?? []).length;
    if (tok.type === "space") continue;
    let html = cache.get(tok.raw);
    if (html === undefined) {
      // 相対パスの画像を asset URL に置き換える
      walkTokens([tok], (t: Token) => {
        if (t.type === "image" && !isExternal(t.href)) {
          const rel = resolveRel(path, t.href);
          if (rel) t.href = toAssetUrl(root, rel);
        }
      });
      const list = Object.assign([tok], { links: tokens.links }) as TokensList;
      html = DOMPurify.sanitize(parser(list, OPTIONS), { ADD_ATTR: ["target"], ALLOWED_URI_REGEXP: SAFE_URI });
      cache.set(tok.raw, html);
    }
    blocks.push({ line: startLine, html });
  }
  return blocks;
}

export const MarkdownPreview = memo(function MarkdownPreview({ text, path, root, topLine, onOpenFile }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<string, string>());
  const cacheKey = useRef("");
  if (cacheKey.current !== `${root}\0${path}`) {
    cacheKey.current = `${root}\0${path}`;
    cache.current = new Map();
  }
  const blocks = useMemo(() => renderBlocks(text, path, root, cache.current), [text, path, root]);

  // エディタの先頭行に合わせてスクロール
  useLayoutEffect(() => {
    const b = box.current;
    if (!b || topLine === null) return;
    const els = b.querySelectorAll<HTMLElement>("[data-line]");
    let target: HTMLElement | null = null;
    for (const el of els) {
      if (Number(el.dataset.line) <= topLine) target = el;
      else break;
    }
    b.scrollTop = target ? Math.max(0, target.offsetTop - 12) : 0;
  }, [topLine, blocks]);

  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    const href = a.getAttribute("href") ?? "";
    e.preventDefault();
    if (/^https?:/i.test(href)) {
      void api.openUrl(href).catch(() => {});
    } else if (href && !href.startsWith("#") && !isExternal(href)) {
      const rel = resolveRel(path, href);
      if (rel) onOpenFile(rel);
    }
  };

  return (
    <div className="md-preview" ref={box} onClick={onClick}>
      <article className="md-body">
        {blocks.length === 0 && <p className="md-empty">(空のドキュメント)</p>}
        {blocks.map((b, i) => (
          <div key={`${i}:${b.line}`} data-line={b.line} dangerouslySetInnerHTML={{ __html: b.html }} />
        ))}
      </article>
    </div>
  );
});
