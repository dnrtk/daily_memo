// ステータスバーの同期ボタンと、クリックで開くパネル(状態・未送信の変更・メモ付き同期・最近の更新)
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { ChangeKind, CommitInfo, SyncError, SyncStatus } from "../types";
import type { SyncApi } from "../useSync";
import { REMOTE_LABEL, relTime, shortRemote, SyncIcon, type SyncIconName } from "./SyncIcons";

type Tone = "muted" | "ok" | "busy" | "warn" | "danger";

interface View {
  icon: SyncIconName;
  label: string;
  tone: Tone;
  headline: string;
}

const ERROR_VIEW: Partial<Record<SyncError["kind"], Pick<View, "icon" | "label" | "tone">>> = {
  network: { icon: "offline", label: "オフライン", tone: "warn" },
  timeout: { icon: "offline", label: "応答なし", tone: "warn" },
  auth: { icon: "error", label: "認証エラー", tone: "danger" },
  not_found: { icon: "error", label: "接続先なし", tone: "danger" },
};

export function viewOf(status: SyncStatus | null, syncing: boolean, err: SyncError | null): View | null {
  if (!status) return null;
  if (status.state === "git_missing") return { icon: "off", label: "同期: Git なし", tone: "muted", headline: "Git が見つかりません" };
  if (status.state !== "ready") return { icon: "off", label: "同期: 未設定", tone: "muted", headline: "同期は設定されていません" };
  if (syncing) return { icon: "syncing", label: "同期中…", tone: "busy", headline: "同期しています…" };
  if (status.conflicts.length > 0)
    return { icon: "warning", label: `競合 ${status.conflicts.length}`, tone: "danger", headline: `他の端末との競合が ${status.conflicts.length} 件あります` };
  if (err) {
    const v = ERROR_VIEW[err.kind] ?? { icon: "error" as const, label: "同期エラー", tone: "danger" as const };
    return { ...v, headline: err.message };
  }
  const unsent = status.pending.length;
  if (unsent > 0 || status.ahead > 0)
    return {
      icon: "upload",
      label: unsent > 0 ? `未送信 ${unsent}` : "未送信",
      tone: "muted",
      headline: unsent > 0 ? `未送信の変更が ${unsent} 件あります` : "送信待ちの変更があります",
    };
  return { icon: "synced", label: "同期済み", tone: "ok", headline: "すべての端末と同期済みです" };
}

const KIND_MARK: Record<ChangeKind, [string, string]> = {
  added: ["A", "追加"],
  modified: ["M", "変更"],
  deleted: ["D", "削除"],
};

interface Props {
  sync: SyncApi;
  /** 自動同期の説明(「5 分ごと・保存の 1 分後」など)。null なら手動のみ */
  autoNote: string | null;
  onOpenSettings(): void;
  onOpenConflicts(): void;
}

export function SyncIndicator({ sync, autoNote, onOpenSettings, onOpenConflicts }: Props) {
  const { status, phase, lastError, doneAt } = sync;
  const [open, setOpen] = useState(false);
  const [flash, setFlash] = useState(false);
  const view = viewOf(status, phase === "syncing", lastError);

  // 同期が終わった瞬間だけアイコンを小さく弾ませる
  useEffect(() => {
    if (!doneAt) return;
    setFlash(true);
    const t = window.setTimeout(() => setFlash(false), 700);
    return () => window.clearTimeout(t);
  }, [doneAt]);

  if (!view || !status) return null;
  const ready = status.state === "ready";
  const title = ready
    ? `${view.headline}\n最終同期: ${relTime(status.last_sync)}\nクリックで詳細(Ctrl+Shift+S で今すぐ同期)`
    : "クリックして同期を設定";

  return (
    <>
      <button
        className={`status-btn sync-btn tone-${view.tone}${open ? " on" : ""}${flash ? " flash" : ""}`}
        title={title}
        aria-haspopup={ready ? "dialog" : undefined}
        aria-expanded={ready ? open : undefined}
        onClick={() => (ready ? setOpen((o) => !o) : onOpenSettings())}
      >
        <SyncIcon name={view.icon} className={view.icon === "syncing" ? "spin" : undefined} />
        <span>{view.label}</span>
      </button>
      {open && ready && (
        <SyncPanel
          sync={sync}
          view={view}
          autoNote={autoNote}
          onClose={() => setOpen(false)}
          onOpenSettings={() => {
            setOpen(false);
            onOpenSettings();
          }}
          onOpenConflicts={() => {
            setOpen(false);
            onOpenConflicts();
          }}
        />
      )}
    </>
  );
}

function SyncPanel({
  sync,
  view,
  autoNote,
  onClose,
  onOpenSettings,
  onOpenConflicts,
}: {
  sync: SyncApi;
  view: View;
  autoNote: string | null;
  onClose(): void;
  onOpenSettings(): void;
  onOpenConflicts(): void;
}) {
  const status = sync.status!;
  const ref = useRef<HTMLDivElement>(null);
  const [message, setMessage] = useState("");
  const [history, setHistory] = useState<CommitInfo[] | null>(null);
  const [, setTick] = useState(0);
  const syncing = sync.phase === "syncing";

  useEffect(() => {
    void sync.refresh();
    // 開いている間は相対時刻を更新する
    const t = window.setInterval(() => setTick((n) => n + 1), 30000);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 同期が終わるたびに最近の更新を取り直す
  useEffect(() => {
    let cancelled = false;
    api
      .syncHistory(6)
      .then((h) => !cancelled && setHistory(h))
      .catch(() => !cancelled && setHistory([]));
    return () => {
      cancelled = true;
    };
  }, [sync.doneAt]);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      if (!ref.current?.contains(t) && !t.closest(".sync-btn")) onClose();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [onClose]);

  const runNow = async () => {
    const o = await sync.run({ manual: true, message: message.trim() || undefined });
    if (o && !o.error) setMessage("");
  };

  const pending = status.pending;
  const shown = pending.slice(0, 8);
  const err = sync.lastError;

  return (
    <div className="sync-panel" ref={ref} role="dialog" aria-label="同期">
      <header className={`sync-head tone-${view.tone}`}>
        <span className="sync-badge">
          <SyncIcon name={view.icon} className={view.icon === "syncing" ? "spin" : undefined} />
        </span>
        <div className="sync-head-text">
          <strong>{view.headline}</strong>
          <span className="sync-sub">
            {status.remote_kind && <>{REMOTE_LABEL[status.remote_kind]} · </>}
            <span title={status.remote_url ?? ""}>{shortRemote(status.remote_url)}</span>
            {status.branch && <> · {status.branch}</>}
          </span>
          <span className="sync-sub">最終同期: {relTime(status.last_sync)}</span>
        </div>
      </header>

      {status.conflicts.length > 0 && (
        <div className="sync-callout danger">
          <SyncIcon name="warning" />
          <span>同じ箇所が別の端末でも編集されました。どちらを残すか選ぶまで同期を止めています。</span>
          <button className="primary small" onClick={onOpenConflicts}>
            解消する
          </button>
        </div>
      )}

      {err && status.conflicts.length === 0 && (
        <div className={`sync-callout ${err.kind === "network" || err.kind === "timeout" ? "warn" : "danger"}`}>
          <SyncIcon name={err.kind === "network" || err.kind === "timeout" ? "offline" : "error"} />
          <div className="sync-callout-body">
            <span>{err.message}</span>
            {(err.kind === "network" || err.kind === "timeout") && (
              <span className="sync-hint">変更はこの端末に記録済みです。つながったら自動で送ります。</span>
            )}
            {err.kind === "auth" && <AuthHint url={status.remote_url} />}
            {err.detail && (
              <details>
                <summary>詳細</summary>
                <pre>{err.detail}</pre>
              </details>
            )}
          </div>
        </div>
      )}

      <section className="sync-section">
        <h4>
          未送信の変更 <span className="count">{pending.length}</span>
          {status.ahead > 0 && <span className="sync-hint"> ・ 記録済み {status.ahead} 件</span>}
        </h4>
        {pending.length === 0 ? (
          <p className="sync-empty">
            <SyncIcon name="check" /> この端末の変更はすべて送信済みです
          </p>
        ) : (
          <ul className="sync-changes">
            {shown.map((c) => (
              <li key={c.path} title={c.path}>
                <span className={`chg chg-${c.kind}`} title={KIND_MARK[c.kind][1]}>
                  {KIND_MARK[c.kind][0]}
                </span>
                <span className="chg-path">{c.path}</span>
              </li>
            ))}
            {pending.length > shown.length && <li className="sync-more">ほか {pending.length - shown.length} 件</li>}
          </ul>
        )}
        <form
          className="sync-form"
          onSubmit={(e) => {
            e.preventDefault();
            void runNow();
          }}
        >
          <input
            value={message}
            placeholder="変更のメモ(省略可)"
            aria-label="変更のメモ"
            onChange={(e) => setMessage(e.target.value)}
            disabled={syncing}
          />
          <button type="submit" className="primary" disabled={syncing || status.conflicts.length > 0}>
            <SyncIcon name="syncing" className={syncing ? "spin" : undefined} />
            {syncing ? "同期中…" : "今すぐ同期"}
          </button>
        </form>
      </section>

      <section className="sync-section">
        <h4>
          <SyncIcon name="history" /> 最近の更新
        </h4>
        {history === null ? (
          <p className="sync-empty">読み込み中…</p>
        ) : history.length === 0 ? (
          <p className="sync-empty">まだ履歴がありません</p>
        ) : (
          <ul className="sync-history">
            {history.map((h) => (
              <li key={h.id}>
                <span className="who">{h.device || h.author}</span>
                <span className="when">{relTime(h.time)}</span>
                <span className="what" title={h.subject}>
                  {h.subject}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <footer className="sync-foot">
        <span>{autoNote ? `自動同期: ${autoNote}` : "自動同期はオフです(手動のみ)"}</span>
        <button className="link-btn" onClick={onOpenSettings}>
          設定…
        </button>
      </footer>
    </div>
  );
}

/** 認証エラーのときの対処(URL の形式で出し分け) */
export function AuthHint({ url }: { url: string | null }) {
  const ssh = !!url && (url.startsWith("ssh://") || (/^[^/\\]+@[^:]+:/.test(url) && !url.includes("://")));
  return (
    <span className="sync-hint">
      {ssh
        ? "SSH 鍵が ssh-agent に読み込まれているか、公開鍵がサービスに登録されているか確認してください。"
        : "HTTPS の場合、初回はブラウザでのサインインが必要です。ターミナル(Ctrl+J)で git fetch を一度実行すると認証情報が保存されます。"}
    </span>
  );
}
