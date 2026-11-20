// 同期まわりのアイコン(アクティビティバーと同じ線画スタイル。色は currentColor に従う)
import type { RemoteKind } from "../types";

export type SyncIconName =
  | "synced"
  | "upload"
  | "syncing"
  | "offline"
  | "warning"
  | "error"
  | "off"
  | "github"
  | "gitlab"
  | "file"
  | "other"
  | "check"
  | "plus"
  | "minus"
  | "dot"
  | "history";

const CLOUD = "M7 18.5h10.2a4.3 4.3 0 0 0 .5-8.56A6.2 6.2 0 0 0 5.8 9.6 4.5 4.5 0 0 0 7 18.5z";

const PATHS: Record<SyncIconName, string[]> = {
  synced: [CLOUD, "m9.4 13.6 1.9 1.9 3.5-3.6"],
  upload: [CLOUD, "M12 16.2v-5.4", "m9.7 12.9 2.3-2.3 2.3 2.3"],
  syncing: ["M19.5 12a7.5 7.5 0 0 1-12.9 5.2", "M4.5 12a7.5 7.5 0 0 1 12.9-5.2", "M17.6 3.4v3.5h-3.5", "M6.4 20.6v-3.5h3.5"],
  offline: [CLOUD, "M4 4l16 16"],
  warning: ["M12 3.8 2.8 19.6h18.4z", "M12 9.8v4.4", "M12 16.9v.2"],
  error: ["M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z", "M12 7.6v5.2", "M12 15.9v.2"],
  off: [CLOUD, "M9.5 13.7h5"],
  // ブランドロゴは使わず、種類が分かる記号にする
  github: ["M5 4.5h11.5a1.5 1.5 0 0 1 1.5 1.5v13.5H6.5A1.5 1.5 0 0 1 5 18z", "M5 16.5h13", "M9 8.5h5"],
  gitlab: ["M12 20.5 3.5 13l2.4-8.3 2.6 5.3h7l2.6-5.3 2.4 8.3z"],
  file: ["M4 5.5h16v5H4z", "M4 13.5h16v5H4z", "M7.5 8h.1", "M7.5 16h.1"],
  other: ["M7 4.5v10", "M7 14.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z", "M17 9.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z", "M17 9.5c0 4-10 2-10 5"],
  check: ["m5 12.5 4.5 4.5L19 7.5"],
  plus: ["M12 5v14", "M5 12h14"],
  minus: ["M5 12h14"],
  dot: ["M12 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4z"],
  history: ["M4.5 12a7.5 7.5 0 1 0 2.2-5.3", "M4.5 4.5v3.2h3.2", "M12 8v4.3l2.8 1.7"],
};

export function SyncIcon({ name, className, title }: { name: SyncIconName; className?: string; title?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className ? `sicon ${className}` : "sicon"} aria-hidden={title ? undefined : true} role={title ? "img" : undefined}>
      {title && <title>{title}</title>}
      {PATHS[name].map((d, i) => (
        <path key={i} d={d} />
      ))}
    </svg>
  );
}

export const REMOTE_LABEL: Record<RemoteKind, string> = {
  github: "GitHub",
  gitlab: "GitLab",
  file: "ファイルサーバ",
  other: "Git サーバー",
};

/** 表示用に短くした接続先(GitHub なら owner/repo、パスなら末尾 2 階層) */
export function shortRemote(url: string | null | undefined): string {
  if (!url) return "";
  const u = url.replace(/\.git$/, "").replace(/\/+$/, "");
  const m = u.match(/^(?:https?:\/\/|ssh:\/\/)?(?:[^@/]+@)?[^/:]+[:/](.+)$/);
  if (m && !/^[a-zA-Z]:[\\/]/.test(u) && !u.startsWith("/") && !u.startsWith("\\")) return m[1];
  const parts = u.split(/[\\/]/).filter(Boolean);
  return parts.slice(-2).join("/");
}

/** 「3 分前」「昨日 14:05」のような相対表記 */
export function relTime(sec: number | null | undefined, now = Date.now()): string {
  if (!sec) return "まだ同期していません";
  const diff = Math.max(0, Math.round(now / 1000 - sec));
  if (diff < 45) return "たった今";
  if (diff < 3600) return `${Math.round(diff / 60)} 分前`;
  const d = new Date(sec * 1000);
  const hm = `${d.getHours()}:${String(d.getMinutes()).padStart(2, "0")}`;
  if (diff < 6 * 3600) return `${Math.round(diff / 3600)} 時間前`;
  const today = new Date(now);
  const yesterday = new Date(now - 86400000);
  const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
  if (same(d, today)) return `今日 ${hm}`;
  if (same(d, yesterday)) return `昨日 ${hm}`;
  return `${d.getMonth() + 1}/${d.getDate()} ${hm}`;
}
