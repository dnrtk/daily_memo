// エディタのタブ・グループ(メイン / 切り離しウィンドウ共通)

export type MdMode = "edit" | "split" | "preview";

export interface Tab {
  path: string;
  dirty: boolean;
  /** text: エディタで編集 / doc: Office・PDF の抽出テキスト閲覧 */
  kind: "text" | "doc";
  docKind?: string;
  focusChunk?: number | null;
  focusSeq?: number;
  terms?: string[];
  /** Markdown の表示モード */
  mdMode?: MdMode;
}

export interface Group {
  /** React の key・エディタの対応付けに使う安定 ID */
  id: number;
  tabs: Tab[];
  active: string | null;
}

export const isMarkdown = (path: string) => /\.(md|markdown)$/i.test(path);

/** 切り離しウィンドウへ渡す / 戻すときの情報 */
export type DetachedPayload =
  | { kind: "editor"; tab: Tab }
  | { kind: "terminal"; id: number; title: string; restore?: TermRestore | null };

/** ターミナル画面の引き継ぎ用(シリアライズした画面と、そこまでに反映した出力の通算位置) */
export interface TermRestore {
  data: string;
  offset: number;
  /** 保存時の画面サイズ(同じサイズで描いてから新しいサイズに合わせると折り返しが崩れない) */
  cols?: number;
  rows?: number;
}
