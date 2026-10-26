// Word / Excel / PowerPoint / PDF の閲覧タブ(抽出テキストを該当箇所までスクロールして表示)
import { api, errorMessage } from "../api";
import { LocationView } from "./LocationView";

interface Props {
  path: string;
  kind: string;
  chunkId: number | null;
  seq: number;
  terms: string[];
  onError(msg: string): void;
}

const KIND_NAME: Record<string, string> = { docx: "Word", excel: "Excel", pptx: "PowerPoint", pdf: "PDF" };

export function DocView({ path, kind, chunkId, seq, terms, onError }: Props) {
  return (
    <div className="docview">
      <div className="docview-bar">
        <span className="docview-note">{KIND_NAME[kind] ?? kind} の抽出テキスト(読み取り専用)</span>
        <span className="spacer" />
        <button className="small" onClick={() => void api.revealInFolder(path).catch((e) => onError(errorMessage(e)))}>
          フォルダで表示
        </button>
        <button className="small primary" onClick={() => void api.openExternal(path).catch((e) => onError(errorMessage(e)))}>
          既定のアプリで開く
        </button>
      </div>
      <div className="docview-body">
        <LocationView
          path={path}
          kind={kind}
          target={chunkId === null ? null : { chunkId, line: null, lineEnd: null }}
          terms={terms}
          seq={seq}
        />
      </div>
    </div>
  );
}
