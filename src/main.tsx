import { StrictMode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createRoot } from "react-dom/client";
import App from "./App";
import { Detached, type DetachedView } from "./Detached";
import "./styles.css";

// 切り離したウィンドウは Rust 側の初期化スクリプトで __MEMO_VIEW__ を受け取る
const view = (window as unknown as { __MEMO_VIEW__?: DetachedView }).__MEMO_VIEW__;

// F11: フルスクリーン表示の切り替え(メイン / 切り離しウィンドウ共通)。
// エディタやターミナルより先に受け取るためキャプチャ段階で拾う
window.addEventListener(
  "keydown",
  (e) => {
    if (e.key !== "F11" || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.repeat) return;
    const w = getCurrentWindow();
    void w
      .isFullscreen()
      .then((on) => w.setFullscreen(!on))
      .catch(() => {});
  },
  true,
);

createRoot(document.getElementById("root")!).render(
  <StrictMode>{view ? <Detached view={view} /> : <App />}</StrictMode>,
);
