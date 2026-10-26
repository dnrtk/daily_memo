#!/usr/bin/env node
// 配布物に含まれる OSS のライセンス一覧と条文を生成する。
//   node scripts/gen-licenses.mjs
//     → THIRD_PARTY_LICENSES.md(一覧・件数・要確認)
//     → public/licenses.html(条文全文。アプリの「このアプリについて」で表示)
//
// 条文の収集:
// - Rust: cargo-about(`cargo install cargo-about --locked --features cli`)。設定は about.toml
// - npm : 各パッケージ同梱の LICENSE / COPYING / NOTICE
// - PDFium: src-tauri/pdfium/licenses/(npm run fetch-pdfium で取得)
// - Apache-2.0 の NOTICE ファイルも拾って掲載する
//
// - Rust: `cargo metadata` の依存解決結果から、アプリ本体(memo-editor)が実行時にリンクする
//         通常依存だけを辿る(build-dependencies / dev-dependencies は配布物に入らないので除外)。
//         Windows / Linux 両ターゲットの和集合。
// - npm : package.json の dependencies から辿る(devDependencies はビルド時のみ)。
//
// ライセンス文字列が許可リストの組み合わせで表せないものは「要確認」に出す。

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const TARGETS = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"];
const APP_CRATE = "memo-editor";

// 業務利用・バイナリ配布で追加義務が「著作権表示とライセンス文の同梱」程度で済むもの
const PERMISSIVE = new Set([
  "MIT", "MIT-0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause",
  "ISC", "Zlib", "0BSD", "Unlicense", "CC0-1.0", "BSL-1.0", "Unicode-3.0", "Unicode-DFS-2016",
  "CDLA-Permissive-2.0", "BlueOak-1.0.0",
]);
// 弱いコピーレフト(ファイル単位)。改変しなければ実務上の追加義務はほぼない
const WEAK_COPYLEFT = new Set(["MPL-2.0"]);

/** "MIT/Apache-2.0" や "(MIT OR Apache-2.0) AND Unicode-3.0" を評価する簡易パーサ */
function classify(expr) {
  if (!expr) return "unknown";
  const norm = expr.replace(/\s*\/\s*/g, " OR ").trim();
  const strip = (s) => s.trim().replace(/^\((.*)\)$/, "$1").trim();
  const evalExpr = (s) => {
    s = strip(s);
    const ands = splitTop(s, " AND ");
    if (ands.length > 1) {
      const r = ands.map(evalExpr);
      return r.includes("unknown") ? "unknown" : r.includes("weak") ? "weak" : "ok";
    }
    const ors = splitTop(s, " OR ");
    if (ors.length > 1) {
      const r = ors.map(evalExpr);
      return r.includes("ok") ? "ok" : r.includes("weak") ? "weak" : "unknown";
    }
    if (PERMISSIVE.has(s)) return "ok";
    if (WEAK_COPYLEFT.has(s)) return "weak";
    return "unknown";
  };
  return evalExpr(norm);
}

function splitTop(s, sep) {
  const out = [];
  let depth = 0, cur = "";
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "(") depth++;
    if (s[i] === ")") depth--;
    if (depth === 0 && s.startsWith(sep, i)) {
      out.push(cur);
      cur = "";
      i += sep.length - 1;
      continue;
    }
    cur += s[i];
  }
  out.push(cur);
  return out;
}

// ---- Rust ----------------------------------------------------------------------

function rustDeps() {
  const all = new Map();
  for (const target of TARGETS) {
    const meta = JSON.parse(
      execFileSync("cargo", ["metadata", "--format-version", "1", "--locked", "--filter-platform", target], {
        cwd: root,
        maxBuffer: 256 * 1024 * 1024,
      }).toString(),
    );
    const pkgs = new Map(meta.packages.map((p) => [p.id, p]));
    const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
    const app = meta.packages.find((p) => p.name === APP_CRATE);
    const seen = new Set();
    const stack = [app.id];
    while (stack.length) {
      const id = stack.pop();
      if (seen.has(id)) continue;
      seen.add(id);
      for (const d of nodes.get(id)?.deps ?? []) {
        // kind: null = 通常依存
        if (d.dep_kinds.some((k) => k.kind === null)) stack.push(d.pkg);
      }
    }
    for (const id of seen) {
      const p = pkgs.get(id);
      if (!p || p.source === null) continue; // 自作クレートは除外
      const key = `${p.name}@${p.version}`;
      const e = all.get(key) ?? {
        name: p.name,
        version: p.version,
        license: p.license ?? (p.license_file ? `file:${p.license_file}` : null),
        repository: p.repository ?? "",
        targets: new Set(),
      };
      e.targets.add(target.includes("windows") ? "win" : "linux");
      all.set(key, e);
    }
  }
  return [...all.values()].sort((a, b) => a.name.localeCompare(b.name));
}

// ---- npm -----------------------------------------------------------------------

function npmDeps() {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const out = new Map();
  const visit = (name, fromDir) => {
    // Node の解決規則どおり、親ディレクトリの node_modules を遡って探す
    let dir = fromDir;
    let found = null;
    while (true) {
      const candidate = join(dir, "node_modules", name, "package.json");
      if (existsSync(candidate)) {
        found = candidate;
        break;
      }
      const parent = dirname(dir);
      if (parent === dir) break;
      dir = parent;
    }
    if (!found) return;
    const p = JSON.parse(readFileSync(found, "utf8"));
    const key = `${p.name}@${p.version}`;
    if (out.has(key)) return;
    const lic = typeof p.license === "string" ? p.license : p.license?.type ?? (p.licenses?.map((l) => l.type).join(" OR ") || null);
    const repo = typeof p.repository === "string" ? p.repository : p.repository?.url ?? "";
    out.set(key, { name: p.name, version: p.version, license: lic, repository: repo.replace(/^git\+/, ""), dir: dirname(found) });
    for (const d of Object.keys({ ...p.dependencies, ...p.optionalDependencies })) visit(d, dirname(found));
  };
  for (const d of Object.keys(pkg.dependencies ?? {})) visit(d, root);
  return [...out.values()].sort((a, b) => a.name.localeCompare(b.name));
}

// ---- 出力 ------------------------------------------------------------------------

function summarize(list) {
  const m = new Map();
  for (const e of list) m.set(e.license ?? "(不明)", (m.get(e.license ?? "(不明)") ?? 0) + 1);
  return [...m.entries()].sort((a, b) => b[1] - a[1]);
}

const rust = rustDeps();
const npm = npmDeps();
const flagged = [...rust, ...npm].filter((e) => classify(e.license) !== "ok");

const lines = [];
lines.push("# Third-party licenses (自動生成)");
lines.push("");
lines.push(`生成: ${new Date().toISOString().slice(0, 10)} / \`npm run licenses\` で再生成`);
lines.push("");
lines.push("配布バイナリに含まれる依存のみ(Rust: 実行時依存 / npm: dependencies)。");
lines.push("ビルド専用ツールや OS 側コンポーネント・外部サービス・モデルは `docs/LICENSES.md` を参照。");
lines.push("");
lines.push(`- Rust crates: ${rust.length} 件`);
lines.push(`- npm packages: ${npm.length} 件`);
lines.push(`- 許可リスト外(要確認): ${flagged.length} 件`);
lines.push("");
if (flagged.length) {
  lines.push("## 要確認");
  lines.push("");
  lines.push("| パッケージ | バージョン | ライセンス | 判定 |");
  lines.push("|---|---|---|---|");
  for (const e of flagged) {
    const c = classify(e.license);
    lines.push(`| ${e.name} | ${e.version} | ${e.license ?? "(記載なし)"} | ${c === "weak" ? "弱コピーレフト" : "不明・要確認"} |`);
  }
  lines.push("");
}
for (const [title, list, withTargets] of [
  ["Rust crates", rust, true],
  ["npm packages", npm, false],
]) {
  lines.push(`## ${title}`);
  lines.push("");
  lines.push("### ライセンス別件数");
  lines.push("");
  lines.push("| ライセンス | 件数 |");
  lines.push("|---|---|");
  for (const [l, n] of summarize(list)) lines.push(`| ${l} | ${n} |`);
  lines.push("");
  lines.push("### 一覧");
  lines.push("");
  lines.push(withTargets ? "| パッケージ | バージョン | ライセンス | 対象 |" : "| パッケージ | バージョン | ライセンス |");
  lines.push(withTargets ? "|---|---|---|---|" : "|---|---|---|");
  for (const e of list) {
    const row = [e.name, e.version, e.license ?? "(記載なし)"];
    if (withTargets) row.push([...e.targets].sort().join(", "));
    lines.push(`| ${row.join(" | ")} |`);
  }
  lines.push("");
}

writeFileSync(join(root, "THIRD_PARTY_LICENSES.md"), lines.join("\n"));
console.log(`rust=${rust.length} npm=${npm.length} flagged=${flagged.length}`);
for (const e of flagged) console.log(`  ! ${e.name}@${e.version}: ${e.license}`);

// ---- 条文 HTML -------------------------------------------------------------------

const LICENSE_FILE = /^(licen[sc]e|copying|notice|unlicense)([-_.].*)?$/i;
const NOTICE_FILE = /^notice([-_.].*)?$/i;
const esc = (t) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const hash = (t) => createHash("sha1").update(t.replace(/\s+/g, " ").trim()).digest("hex");

function cargoAbout() {
  try {
    const out = execFileSync(
      "cargo",
      ["about", "generate", "--format", "json", "-c", "about.toml", "-m", "src-tauri/Cargo.toml"],
      { cwd: root, maxBuffer: 512 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] },
    );
    return JSON.parse(out.toString());
  } catch (e) {
    console.error("cargo-about の実行に失敗しました。`cargo install cargo-about --locked --features cli` でインストールしてください。");
    console.error(String(e.stderr ?? e).slice(0, 2000));
    process.exit(1);
  }
}

/** 同じ条文はまとめて「使用パッケージ」を列挙する */
const groups = new Map(); // hash -> { title, text, users:Set, section }
function addText(section, title, text, user) {
  const key = hash(text);
  const g = groups.get(key) ?? { section, title, text: text.trim(), users: new Set() };
  g.users.add(user);
  groups.set(key, g);
}

// Rust
const about = cargoAbout();
for (const lic of about.licenses) {
  const users = lic.used_by.map((u) => u.crate).filter((c) => c.source); // 自作クレートは除外
  for (const c of users) addText("Rust", `${lic.name}(${lic.id})`, lic.text, `${c.name} ${c.version}`);
}
// NOTICE(Apache-2.0 で同梱が求められる)
for (const k of about.crates) {
  const pkg = k.package;
  if (!pkg.source || !pkg.manifest_path) continue;
  const dir = dirname(pkg.manifest_path);
  for (const f of readdirSync(dir).filter((f) => NOTICE_FILE.test(f))) {
    addText("Rust", `NOTICE: ${pkg.name}`, readFileSync(join(dir, f), "utf8"), `${pkg.name} ${pkg.version}`);
  }
}
// npm
for (const p of npm) {
  const files = readdirSync(p.dir).filter((f) => LICENSE_FILE.test(f));
  if (files.length === 0) {
    addText("npm", p.license ?? "(記載なし)", `(パッケージにライセンスファイルが同梱されていません。package.json の宣言: ${p.license})`, `${p.name} ${p.version}`);
  }
  for (const f of files) addText("npm", `${p.license ?? ""} ${f}`.trim(), readFileSync(join(p.dir, f), "utf8"), `${p.name} ${p.version}`);
}
// PDFium
const pdfiumLic = join(root, "src-tauri", "pdfium", "licenses");
const PDFIUM_TITLES = {
  "pdfium.txt": "PDFium",
  "pdfium-binaries.txt": "pdfium-binaries(ビルド済みバイナリの配布元)",
  "freetype.txt": "FreeType(PDFium 同梱。Portions of this software are copyright © The FreeType Project (www.freetype.org). All rights reserved.)",
};
if (existsSync(pdfiumLic)) {
  for (const f of readdirSync(pdfiumLic).sort()) {
    const title = PDFIUM_TITLES[f] ?? `PDFium 同梱: ${f.replace(/\.(txt|md|ijg)$/, "")}`;
    addText("PDFium", title, readFileSync(join(pdfiumLic, f), "utf8"), f === "pdfium-binaries.txt" ? "pdfium-binaries" : "pdfium");
  }
} else {
  console.warn("! src-tauri/pdfium/licenses がありません(npm run fetch-pdfium を先に実行)");
}

const sections = ["PDFium", "Rust", "npm"];
const sectionTitle = { PDFium: "PDFium(PDF の読み取り)", Rust: "Rust クレート", npm: "npm パッケージ(画面)" };
const own = readFileSync(join(root, "LICENSE"), "utf8");
const body = [];
body.push(`<h1>ライセンス情報</h1>`);
body.push(`<p class="lead">memo-editor 本体と、同梱しているオープンソースソフトウェアのライセンス条文です。生成日: ${new Date().toISOString().slice(0, 10)}</p>`);
body.push(`<nav>${sections.map((s) => `<a href="#${s}">${sectionTitle[s]}</a>`).join(" ・ ")}</nav>`);
body.push(`<section><h2>memo-editor</h2><pre>${esc(own)}</pre></section>`);
for (const sec of sections) {
  const items = [...groups.values()].filter((g) => g.section === sec);
  body.push(`<section id="${sec}"><h2>${sectionTitle[sec]}</h2>`);
  for (const g of items) {
    const users = [...g.users].sort();
    body.push(
      `<details><summary><strong>${esc(g.title)}</strong><span>${esc(users.slice(0, 6).join(", "))}${users.length > 6 ? ` ほか ${users.length - 6} 件` : ""}</span></summary>` +
        `<p class="users">${esc(users.join(", "))}</p><pre>${esc(g.text)}</pre></details>`,
    );
  }
  body.push(`</section>`);
}
const html = `<!doctype html>
<html lang="ja"><head><meta charset="utf-8"><title>ライセンス情報</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
:root{--bg:#f7f6f3;--fg:#2b2a27;--muted:#75716a;--border:#d9d5cc;--card:#fff;--accent:#2f6f62;color-scheme:light}
@media (prefers-color-scheme:dark){:root{--bg:#1e2023;--fg:#d9d7d2;--muted:#8b8984;--border:#34373b;--card:#26292d;--accent:#6fb5a3;color-scheme:dark}}
body{margin:0;padding:16px 20px 40px;background:var(--bg);color:var(--fg);font:13px/1.6 "BIZ UDPGothic","Yu Gothic UI","Noto Sans JP",system-ui,sans-serif}
h1{font-size:18px;margin:0 0 4px}h2{font-size:14px;margin:28px 0 8px;padding-bottom:4px;border-bottom:1px solid var(--border)}
.lead{color:var(--muted);margin:0 0 8px}nav a{color:var(--accent)}
details{background:var(--card);border:1px solid var(--border);border-radius:8px;margin:6px 0}
summary{cursor:pointer;padding:8px 12px;display:flex;flex-direction:column;gap:2px}
summary span{color:var(--muted);font-size:11px}
.users{margin:0;padding:0 12px 6px;color:var(--muted);font-size:11px}
pre{margin:0;padding:10px 12px;border-top:1px solid var(--border);white-space:pre-wrap;word-break:break-word;font:11px/1.5 ui-monospace,"BIZ UDGothic",monospace}
section>pre{border:1px solid var(--border);border-radius:8px;background:var(--card)}
</style></head><body>
${body.join("\n")}
</body></html>
`;
mkdirSync(join(root, "public"), { recursive: true });
writeFileSync(join(root, "public", "licenses.html"), html);
console.log(`licenses.html: ${groups.size} 条文 / ${(html.length / 1024).toFixed(0)} KB`);
