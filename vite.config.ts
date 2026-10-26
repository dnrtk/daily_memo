import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 用の Vite 設定
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    // language-data の各言語は遅延ロードされるので本体サイズの警告は緩める
    chunkSizeWarningLimit: 1600,
  },
});
