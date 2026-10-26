# OSS ライセンス整理(業務利用向け)

最終確認: 2026-10-26。法的助言ではなく、エンジニアとしての一次整理。会社に OSS 利用規程がある場合はそちらを優先すること。

## 結論

- **自分の PC で業務に使う分には問題なし。** 本体に組み込まれる依存は MIT / Apache-2.0 / BSD 系などの寛容ライセンスで、GPL・AGPL 系は含まれない(PDFium 同梱分を含む)。
- 寛容ライセンスの義務は基本的に「再配布するときに著作権表示とライセンス文を添付する」だけ。アプリ内の「このアプリについて」(左下の ⓘ)で、同梱する全 OSS の条文と著作権表示を表示しているので、社内配布時もこの画面で義務を満たせる。
- 例外として MPL-2.0 が 5 件あるが、いずれも Tauri の内部依存で無改変利用のため、追加義務は実質なし(後述)。

## 1. 本体に組み込まれる依存

依存ツリーから自動抽出した全件は [`../THIRD_PARTY_LICENSES.md`](../THIRD_PARTY_LICENSES.md)、条文全文は `public/licenses.html`(アプリ内で表示)。どちらも `npm run licenses` で再生成する。

| 区分 | 件数 | 内訳 |
|---|---|---|
| Rust crates(Windows / Linux 実行時依存の和集合) | 434 | MIT 系・Apache-2.0 系が大半。他に Unicode-3.0 / Unlicense OR MIT / BSD-3-Clause / ISC / CC0-1.0 / CDLA-Permissive-2.0 / MPL-2.0(5) |
| npm packages(画面に同梱されるもの) | 66 | MIT 63、MIT OR Apache-2.0 2、MPL-2.0 OR Apache-2.0 1(DOMPurify。Apache-2.0 を選択) |
| PDFium(ビルド済みバイナリ) | 1 + 同梱 14 | 下記 |

主要なもの:

| コンポーネント | 用途 | ライセンス |
|---|---|---|
| Tauri / wry / tao | アプリ基盤・WebView 制御 | Apache-2.0 OR MIT(tao は Apache-2.0) |
| CodeMirror 6 | エディタ | MIT |
| xterm.js(@xterm/xterm, addon-fit, addon-web-links, addon-serialize) | 組み込みターミナルの画面 | MIT |
| marked | Markdown プレビュー | MIT |
| DOMPurify | プレビュー HTML の無害化 | MPL-2.0 OR Apache-2.0(Apache-2.0 を選択できるので寛容扱い) |
| portable-pty | 組み込みターミナルの疑似端末(Windows: ConPTY) | MIT |
| React / React DOM | UI | MIT |
| reqwest / rustls / ring | LLM サーバーとの HTTP 通信 | MIT OR Apache-2.0 / Apache-2.0 OR ISC OR MIT / Apache-2.0 AND ISC |
| rusqlite / SQLite | インデックス(FTS5 全文検索) | MIT / パブリックドメイン |
| sqlite-vec | ベクトル検索 | MIT OR Apache-2.0 |
| calamine | Excel 読み取り | MIT |
| zip / quick-xml | Word / PowerPoint 読み取り | MIT / MIT |
| pdfium-render | PDF 読み取り(PDFium のバインディング) | MIT OR Apache-2.0 |
| html2text | HTML のテキスト化 | MIT |
| notify / notify-debouncer-full | フォルダ監視 | CC0-1.0 / MIT OR Apache-2.0 |
| blake3 | 差分検出用ハッシュ | CC0-1.0 OR Apache-2.0 |
| unicode-normalization | 検索語の正規化 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| webpki-roots | TLS ルート証明書 | CDLA-Permissive-2.0(寛容) |
| encoding_rs | Shift_JIS CSV の読み書き | (Apache-2.0 OR MIT) AND BSD-3-Clause |

### MPL-2.0 の 5 件について

`cssparser` `cssparser-macros` `dtoa-short` `selectors`(Tauri のビルド時 HTML 処理 `dom_query` 経由)と `option-ext`(設定ディレクトリ取得の `dirs` 経由)。MPL-2.0 はファイル単位のコピーレフトで、**そのファイル自体を改変して配布する場合のみ**改変部分のソース公開義務が生じる。無改変で使っているので追加義務はなく、条文を添付すれば足りる(アプリ内表示に含まれている)。

### PDFium(PDF 読み取り)

bblanchon/pdfium-binaries の chromium/7881 ビルド(pdfium-render の `pdfium_7881` と対応)を `npm run fetch-pdfium` で取得し、インストーラに同梱する。配布アーカイブ同梱のライセンスファイルを確認した結果:

| 対象 | ライセンス |
|---|---|
| PDFium 本体 | BSD-3-Clause + Apache-2.0 |
| pdfium-binaries(ビルド・配布スクリプト) | MIT |
| FreeType | FTL(FreeType License。GPL との選択制で FTL を選択)。ドキュメントへのクレジット表記が条件 → ライセンス画面に記載済み |
| ICU | Unicode License v3 |
| abseil / llvm-libc | Apache-2.0(llvm-libc は LLVM 例外付き) |
| libjpeg-turbo | IJG + BSD-3-Clause |
| libpng | PNG Reference Library License v2 |
| libtiff / LittleCMS / AGG 2.3 | 各独自の寛容ライセンス(MIT 類似) |
| OpenJPEG | BSD-2-Clause |
| zlib | zlib |
| fast_float / simdutf | MIT ほか(選択制) |

いずれも寛容ライセンス。条文 16 ファイルをすべてライセンス画面に掲載している。

### 日本語の全文検索について(辞書を使わない理由)

当初は lindera + IPADIC(形態素解析)の予定だったが、**文字バイグラム**に変更した。IPADIC を埋め込むとバイナリが数十 MB 増える、全文検索側の役割(固有名詞・型番の確実な一致)にはバイグラムの方が取りこぼしが少ない、辞書のライセンス表示も不要、の 3 点が理由。

## 2. OS 側のコンポーネント(アプリには同梱しない)

| コンポーネント | ライセンス | 備考 |
|---|---|---|
| Microsoft Edge WebView2 Runtime(Windows) | Microsoft のランタイム条項 | Windows 10/11 に標準搭載。Tauri はこれを呼び出すだけ |
| WebKitGTK(Ubuntu) | LGPL-2.1 ほか BSD / MPL / Apache 等の組み合わせ | OS パッケージを動的リンク。`.deb` 配布なら同梱しないので義務なし。**AppImage は WebKitGTK を同梱する**ため、社内配布するなら LGPL の表示・ソース入手方法の案内が必要 → Ubuntu は `.deb` 推奨 |
| フォント | — | OS のフォントを参照するだけで同梱しない |

## 3. 外部プロセス・サーバー(HTTP で呼ぶだけ)

| ソフトウェア | ライセンス | 備考 |
|---|---|---|
| Ollama | MIT | 別プロセス。アプリにリンクしないのでアプリ側に義務は及ばない |
| vLLM | Apache-2.0 | 同上(LAN 内サーバー想定) |

## 4. モデル(重み)

モデルはソフトウェアとは別にライセンスがある。業務で使うなら**モデルごとに確認が必要**。今の既定値はどちらも業務利用可。

| モデル | 用途 | ライセンス |
|---|---|---|
| BAAI/bge-m3 | 埋め込み(既定) | MIT |
| Qwen3-4B | クエリ解釈・振り分け(既定) | Apache-2.0 |
| cl-nagoya/ruri-v3(候補) | 日本語特化埋め込み | Apache-2.0 |

注意: Gemma 系は「Gemma Terms of Use」、Llama 系は「Llama Community License」で、Apache/MIT とは条件が異なる(利用規約への同意・禁止用途の遵守など)。モデルを差し替えるときは Hugging Face のモデルカードでライセンスを確認すること。Ollama の量子化版は元モデルのライセンスを引き継ぐ。

## 5. 依存を変えたときの手順

1. `npm run licenses` を実行(要 `cargo install cargo-about --locked --features cli`)
   - `THIRD_PARTY_LICENSES.md` と `public/licenses.html` が更新される
   - 許可リスト(`about.toml` と `scripts/gen-licenses.mjs` の `PERMISSIVE`)外のライセンスがあると「要確認」に出る / cargo-about がエラーにする
2. 「要確認」が出たら内容を確認してから許可リストに追加する
3. PDFium のバージョンを上げたら `npm run fetch-pdfium` → `npm run licenses`

## 6. 社内の他の人にも配る場合

- ライセンス画面(ⓘ)で条文と著作権表示は満たせる。念のため `THIRD_PARTY_LICENSES.md` を配布物に添えるとよい
- Ubuntu 向けは `.deb` で配る(AppImage は WebKitGTK 同梱で LGPL 対応が要る)
- 本アプリ自体のライセンス(`LICENSE`、現在 MIT)は社内規程に合わせて変更してよい
