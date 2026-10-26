#!/usr/bin/env node
// PDFium のビルド済みバイナリ(bblanchon/pdfium-binaries)を取得して src-tauri/pdfium/ に置く。
//   node scripts/fetch-pdfium.mjs            … 実行中の OS 用
//   node scripts/fetch-pdfium.mjs win-x64    … 指定(win-x64 / win-arm64 / linux-x64 / linux-arm64 / mac-arm64 / mac-x64)
//   node scripts/fetch-pdfium.mjs --if-missing … 既に置いてあれば何もしない(tauri dev / build の前処理用)
//
// バージョンは pdfium-render の feature(pdfium_7881)と一致させること。
// 展開には OS 標準の tar を使う(Windows 10 以降は tar.exe が標準搭載)。

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PDFIUM_BUILD = "7881";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dest = join(root, "src-tauri", "pdfium");

const detect = () => {
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  if (process.platform === "win32") return `win-${arch}`;
  if (process.platform === "darwin") return `mac-${arch}`;
  return `linux-${arch}`;
};
const args = process.argv.slice(2);
const ifMissing = args.includes("--if-missing");
const platform = args.find((a) => !a.startsWith("--")) ?? detect();

if (ifMissing && existsSync(dest) && readdirSync(dest).some((f) => /^(lib)?pdfium\.(dll|so|dylib)$/.test(f))) {
  process.exit(0);
}
const url = `https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_BUILD}/pdfium-${platform}.tgz`;

console.log(`PDFium ${PDFIUM_BUILD} (${platform}) を取得: ${url}`);
const res = await fetch(url);
if (!res.ok) {
  console.error(`ダウンロード失敗: HTTP ${res.status}`);
  process.exit(1);
}
const work = mkdtempSync(join(tmpdir(), "pdfium-"));
const archive = join(work, "pdfium.tgz");
writeFileSync(archive, Buffer.from(await res.arrayBuffer()));
execFileSync("tar", ["-xzf", archive, "-C", work]);

rmSync(dest, { recursive: true, force: true });
mkdirSync(join(dest, "licenses"), { recursive: true });

// ライブラリ本体: Windows は bin/pdfium.dll、それ以外は lib/libpdfium.*
const libDir = platform.startsWith("win") ? join(work, "bin") : join(work, "lib");
const lib = readdirSync(libDir).find((f) => /^(lib)?pdfium\.(dll|so|dylib)$/.test(f));
if (!lib) {
  console.error(`ライブラリが見つかりません: ${libDir}`);
  process.exit(1);
}
cpSync(join(libDir, lib), join(dest, lib));

// ライセンス(PDFium 本体と同梱サードパーティ)と配布元の LICENSE
cpSync(join(work, "licenses"), join(dest, "licenses"), { recursive: true });
cpSync(join(work, "LICENSE"), join(dest, "licenses", "pdfium-binaries.txt"));
if (existsSync(join(work, "VERSION"))) cpSync(join(work, "VERSION"), join(dest, "VERSION"));
rmSync(work, { recursive: true, force: true });

console.log(`配置しました: ${join(dest, lib)}`);
console.log(`ライセンス: ${readdirSync(join(dest, "licenses")).length} ファイル`);
