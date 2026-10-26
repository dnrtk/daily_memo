# memo-editor

日々のメモに特化した軽量エディタ。Tauri 2(Rust)+ React + CodeMirror 6。Windows / Ubuntu 対応。
作業フォルダ内の Markdown・テキスト・Word・Excel・PowerPoint・PDF を、ローカル GPU 上の LLM(Ollama / vLLM)を使って自然言語で検索できる。

## できること

**メモ**

- 作業フォルダを開いてファイルツリー表示・タブ編集・自動保存
- 作業フォルダを開くと `日々` `技術` `調査` `Todo` `アイデア` `教訓` のカテゴリフォルダを自動作成
- カテゴリの追加・削除・並べ替え・定義編集(設定 → カテゴリ)。フォルダ名を変えると実フォルダも移動。カテゴリを外してもフォルダとファイルは残る
- 「今日のメモ」: `日々/yyyy/MM/yyyy-MM-dd.md` を開いて末尾にカーソル
- クイックメモ: カテゴリを選んで保存(日々は時刻見出し付きで追記、Todo は `inbox.md` にチェックボックスで追記、他は新規ファイル)
- Shift_JIS・BOM 付き UTF-8・CRLF のファイルは元の形式のまま保存
- 削除は OS のゴミ箱へ。作業フォルダ外へのアクセスと `.memo/` への書き込みは Rust 側で拒否

**自然言語検索**

- 「先月の見積もり関係のパワポ」のような文で検索。LLM が「本題 / 完全一致させたい語 / ファイル種類 / 期間 / カテゴリ」に分解し、条件はチップで表示
- 全文検索(文字バイグラム、辞書不要)と意味検索(埋め込みベクトル)を RRF で統合。各結果に「語」「意味」どちらで当たったかを表示
- 結果はファイル単位で、該当箇所(Markdown の見出し・行 / Word の見出し / Excel のシートと行 / PowerPoint のスライド / PDF のページ)を最大 3 件。テキストは該当行へジャンプ、Office・PDF は既定のアプリで開く
- 条件で絞りすぎて 0 件なら「条件を外して検索する」
- インデックスは作業フォルダの `.memo/index.sqlite`。フォルダ監視で、保存・追加・削除・移動を数秒で反映
- LLM や埋め込みサーバーが落ちていても全文検索は動く。ベクトル化は 1 分ごとに自動再試行
- 埋め込みモデルを変えたら「作り直す」ボタンでベクトルだけ再作成(抽出結果は再利用)

**その他**

- LLM 接続先(Ollama ネイティブ / OpenAI 互換 = vLLM 等)を複数登録し、埋め込み用・LLM 用で別サーバーを指定可能。接続テスト付き
- ruri-v3 のように接頭辞が必要な埋め込みモデルにも対応(設定 → LLM 接続)
- 「このアプリについて」(左下 ⓘ)に同梱 OSS のライセンス条文を全文表示
- Markdown プレビュー(編集 / 分割 / プレビュー。分割時はエディタのスクロールに追従、作業フォルダ内の画像・相対リンク対応)
- エディタを左右 2 分割。タブはドラッグ / 右クリックで反対側へ移動、ウィンドウの外へドラッグで別ウィンドウに切り離し、「メインに戻す」で戻る
- 組み込みターミナル(Windows は PowerShell、Linux はログインシェル。設定で変更可)。タブ複数、左右 2 分割、別ウィンドウへの切り離し / 戻すも可(シェルと画面はそのまま引き継ぐ)。隠してもセッション維持、× でシェル終了
- 作業フォルダをエクスプローラー(Linux は既定のファイルマネージャ)で開く

| ショートカット | 動作 |
|---|---|
| Ctrl+Shift+D | 今日のメモ |
| Ctrl+Shift+N | クイックメモ(ダイアログ内 Ctrl+Enter で保存) |
| Ctrl+Shift+F | 自然言語検索 |
| Ctrl+B | サイドバーの表示 / 非表示 |
| Ctrl+J | ターミナルの表示 / 非表示 |
| Ctrl+\\ | エディタを左右に分割(2 分割中は反対側へ移動) |
| Ctrl+Shift+V | Markdown の編集 ⇔ 分割プレビュー |
| Ctrl+E | 作業フォルダをエクスプローラーで開く |
| Ctrl+S / Ctrl+W | 保存 / タブを閉じる |
| Ctrl+O | 作業フォルダを開く |
| Ctrl+, | 設定 |
| F11 | フルスクリーン表示 / 解除 |

## セットアップ

前提: Rust(stable)、Node.js 20 以上、Ollama(または OpenAI 互換サーバー)。

**Windows**: WebView2 は Windows 10/11 に標準搭載。Visual Studio Build Tools(C++)が必要。

**Ubuntu 22.04 / 24.04**:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

**モデル**(既定の設定の場合。VRAM 6GB で両方常駐できる):

```bash
ollama pull bge-m3      # 埋め込み(約 1.2GB)
ollama pull qwen3:4b    # クエリ解釈(Q4 で約 2.5〜3GB)
```

**ビルド・起動**:

```bash
npm install
npm run tauri dev      # 開発起動(初回は PDFium を自動取得)
npm run tauri build    # インストーラ生成(Windows: NSIS / Ubuntu: deb, AppImage)
cargo test --workspace # テスト(PDF のテストは PDFium 取得後に有効)
```

PDFium は `npm run fetch-pdfium` で `src-tauri/pdfium/` に取得される(`tauri dev` / `tauri build` の前に未取得なら自動で取得)。別の場所に置く場合は環境変数 `MEMO_PDFIUM_DIR` で指定できる。

依存を追加・更新したら `npm run licenses` でライセンス一覧と条文を再生成する(要 `cargo install cargo-about --locked --features cli`)。

## 構成

```
crates/memo-core/    UI 非依存のロジック
  workspace.rs         作業フォルダ・ファイル操作・カテゴリへの保存
  category.rs          カテゴリ定義 (.memo/categories.yaml)
  settings.rs          アプリ設定(LLM 接続先・エディタ・検索)
  llm.rs               Ollama / OpenAI 互換クライアント(モデル一覧・埋め込み・JSON チャット)
  paths.rs / text.rs / template.rs
crates/memo-index/   検索エンジン
  extract/             形式ごとの抽出(md / txt / csv / html / docx / xlsx / pptx / pdf)
  chunk.rs             チャンク分割(行番号を保持)
  tokenize.rs          文字バイグラム化・検索語の分割
  store.rs             SQLite(FTS5 + sqlite-vec)
  indexer.rs           差分インデックス・埋め込み
  search.rs            LLM クエリ解釈・ハイブリッド検索(RRF)
  watch.rs             フォルダ監視
src-tauri/           Tauri コマンド層とバックグラウンドのインデックスサービス
src/                 画面(React + CodeMirror 6)
scripts/             PDFium 取得・ライセンス生成・テスト用フィクスチャ生成
docs/                ライセンス整理・ロードマップ
```

設定ファイルの場所:

- カテゴリ定義・インデックス: `<作業フォルダ>/.memo/`(作業フォルダと一緒に持ち運べる。インデックスは消しても再生成される)
- アプリ設定: Windows `%APPDATA%\dev.taka.memo-editor\settings.yaml` / Ubuntu `~/.config/dev.taka.memo-editor/settings.yaml`

## ライセンス

本体は MIT(`LICENSE`)。依存 OSS は [`docs/LICENSES.md`](docs/LICENSES.md)(業務利用観点の整理)、[`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md)(自動生成の一覧)、`public/licenses.html`(条文全文、アプリ内で表示)を参照。
