// このアプリについて + 同梱 OSS のライセンス全文(ビルド時に生成した licenses.html を表示)
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";

export function AboutDialog({ onClose }: { onClose(): void }) {
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  return (
    <div className="backdrop" onMouseDown={onClose}>
      <div
        className="dialog about"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onClose()}
        tabIndex={-1}
      >
        <header className="settings-head">
          <h2>memo-editor {version && <span className="ver">v{version}</span>}</h2>
          <button className="icon" onClick={onClose} aria-label="閉じる">
            ×
          </button>
        </header>
        <p className="about-lead">
          このソフトウェアは以下のオープンソースソフトウェアを利用しています。各ライセンスの条文と著作権表示を掲載します。
        </p>
        <iframe className="licenses" src="/licenses.html" title="ライセンス情報" />
      </div>
    </div>
  );
}
