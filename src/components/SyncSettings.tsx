// 設定 → 同期。未設定なら「種類を選ぶ → 場所を入れて確認 → 開始」、設定済みなら接続先と自動同期の設定。
import { useEffect, useRef, useState } from "react";
import { ask, open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errorMessage } from "../api";
import type { AppSettings, Probe, RemoteKind, SyncError, SyncOutcome, SyncSettings, SyncStatus } from "../types";
import type { SyncApi } from "../useSync";
import { Field } from "./Field";
import { REMOTE_LABEL, relTime, shortRemote, SyncIcon } from "./SyncIcons";
import { AuthHint } from "./SyncIndicator";

interface Props {
  settings: AppSettings;
  hasWorkspace: boolean;
  sync: SyncApi;
  /** 接続の前にエディタを保存する */
  flush(): Promise<void>;
  /** 接続・同期の結果を画面(タブ・ツリー)に反映する */
  onOutcome(o: SyncOutcome): void;
  onSaved(s: AppSettings): void;
}

const KINDS: { id: RemoteKind; placeholder: string; help: string }[] = [
  {
    id: "github",
    placeholder: "https://github.com/ユーザー名/memo.git",
    help: "GitHub で空のプライベートリポジトリを作り、その URL を貼り付けてください。HTTPS なら初回にブラウザでサインインを求められます。SSH(git@github.com:ユーザー名/memo.git)も使えます。",
  },
  {
    id: "gitlab",
    placeholder: "https://gitlab.com/ユーザー名/memo.git",
    help: "GitLab(社内の GitLab も可)で空のプロジェクトを作り、その URL を貼り付けてください。HTTPS / SSH のどちらでも使えます。",
  },
  {
    id: "file",
    placeholder: navigator.userAgent.includes("Windows") ? "\\\\server\\share\\memo.git" : "/mnt/nas/memo.git",
    help: "共有フォルダ上の場所を指定します。まだ無いフォルダ名か空のフォルダなら、そこにリポジトリを作ります。Windows は \\\\server\\share\\… やネットワークドライブ(Z:\\…)、Linux はマウント先のパスを指定してください。",
  },
  {
    id: "other",
    placeholder: "ssh://git@git.example.com/memo.git",
    help: "Gitea・Bitbucket・自前の Git サーバーなど。git clone に使う URL をそのまま貼り付けてください。",
  },
];

const INTERVALS = [1, 3, 5, 10, 15, 30];
const AFTER_SAVE = [
  [10, "10 秒"],
  [30, "30 秒"],
  [60, "1 分"],
  [180, "3 分"],
  [300, "5 分"],
] as const;

export function SyncTab({ settings, hasWorkspace, sync, flush, onOutcome, onSaved }: Props) {
  const status = sync.status;
  const [editing, setEditing] = useState(false);
  const [defaultDevice, setDefaultDevice] = useState("");

  useEffect(() => {
    void sync.refresh();
    api.syncDefaultDevice().then(setDefaultDevice).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!hasWorkspace) {
    return (
      <div className="settings-body">
        <section className="form narrow">
          <p className="field-hint">作業フォルダを開くと、同期を設定できます。</p>
        </section>
      </div>
    );
  }
  if (!status) {
    return (
      <div className="settings-body">
        <section className="form narrow">
          <p className="field-hint">状態を確認しています…</p>
        </section>
      </div>
    );
  }
  if (status.state === "git_missing") return <GitMissing />;

  const ready = status.state === "ready";
  return ready && !editing ? (
    <ConfiguredView
      status={status}
      settings={settings}
      defaultDevice={defaultDevice}
      sync={sync}
      onChangeRemote={() => setEditing(true)}
      onSaved={onSaved}
    />
  ) : (
    <SetupView
      status={status}
      settings={settings}
      defaultDevice={defaultDevice}
      changing={ready}
      onCancel={ready ? () => setEditing(false) : undefined}
      flush={flush}
      onSaved={onSaved}
      onDone={(o) => {
        sync.apply(o);
        onOutcome(o);
        if (!o.error) setEditing(false);
      }}
    />
  );
}

// ---- Git がない ----------------------------------------------------------------

function GitMissing() {
  const win = navigator.userAgent.includes("Windows");
  return (
    <div className="settings-body">
      <section className="form narrow">
        <div className="sync-callout danger">
          <SyncIcon name="error" />
          <div className="sync-callout-body">
            <strong>Git が見つかりません</strong>
            <span>同期には Git を使います。インストールしてから、このアプリを再起動してください。</span>
          </div>
        </div>
        {win ? (
          <p>
            <button onClick={() => void api.openUrl("https://git-scm.com/download/win")}>Git for Windows のダウンロードページを開く</button>
          </p>
        ) : (
          <pre className="sync-code">sudo apt install git</pre>
        )}
      </section>
    </div>
  );
}

// ---- 初回設定 / 接続先の変更 -----------------------------------------------------

type ProbeView = { state: "idle" } | { state: "checking" } | { state: "ok"; probe: Probe } | { state: "error"; error: SyncError };

function SetupView({
  status,
  settings,
  defaultDevice,
  changing,
  onCancel,
  flush,
  onSaved,
  onDone,
}: {
  status: SyncStatus;
  settings: AppSettings;
  defaultDevice: string;
  changing: boolean;
  onCancel?(): void;
  flush(): Promise<void>;
  onSaved(s: AppSettings): void;
  onDone(o: SyncOutcome): void;
}) {
  const existing = status.state === "disabled" ? status.remote_url : changing ? status.remote_url : null;
  const [url, setUrl] = useState(existing ?? "");
  const [kind, setKind] = useState<RemoteKind>(status.remote_kind ?? "github");
  const [probe, setProbe] = useState<ProbeView>({ state: "idle" });
  const [device, setDevice] = useState(settings.sync.device_name);
  const [connecting, setConnecting] = useState(false);
  const [result, setResult] = useState<SyncOutcome | null>(null);
  const seq = useRef(0);
  const meta = KINDS.find((k) => k.id === kind)!;

  // 入力から種類を推定してカードを合わせる
  const onUrl = (v: string) => {
    setUrl(v);
    setProbe({ state: "idle" });
    setResult(null);
    const t = v.trim();
    if (!t) return;
    if (/^([a-zA-Z]:[\\/]|\\\\|\/\/|\/|~\/|file:)/.test(t)) setKind("file");
    else if (/github/i.test(t)) setKind("github");
    else if (/gitlab/i.test(t)) setKind("gitlab");
    else if (/[:/]/.test(t)) setKind("other");
  };

  const check = async () => {
    if (!url.trim()) return;
    const n = ++seq.current;
    setProbe({ state: "checking" });
    try {
      const r = await api.syncProbe(url);
      if (n !== seq.current) return;
      setProbe("ok" in r ? { state: "ok", probe: r.ok } : { state: "error", error: r.err });
      if ("ok" in r) setKind(r.ok.kind);
    } catch (e) {
      if (n === seq.current) setProbe({ state: "error", error: { kind: "other", message: errorMessage(e) } });
    }
  };

  const pickFolder = async () => {
    const dir = await openDialog({ directory: true, multiple: false, title: "同期先のフォルダ(共有フォルダ)を選択" });
    if (typeof dir !== "string") return;
    onUrl(dir);
    setKind("file");
  };

  const start = async () => {
    if (probe.state !== "ok") return;
    setConnecting(true);
    setResult(null);
    try {
      if (device !== settings.sync.device_name) {
        onSaved(await api.saveSettings({ ...settings, sync: { ...settings.sync, device_name: device.trim() } }));
      }
      await flush();
      const o = await api.syncConnect(probe.probe.url);
      setResult(o);
      onDone(o);
    } catch (e) {
      setResult({ report: null, error: { kind: "other", message: errorMessage(e) }, status, workspace: null });
    } finally {
      setConnecting(false);
    }
  };

  const startLabel =
    probe.state !== "ok"
      ? "同期を開始"
      : probe.probe.state === "has_data"
        ? "取り込んで同期を開始"
        : probe.probe.state === "will_create"
          ? "作成して同期を開始"
          : "アップロードして同期を開始";

  return (
    <div className="settings-body">
      <section className="form sync-setup">
        {!changing && (
          <p className="sync-lead">
            この作業フォルダのメモを、Git リポジトリを経由して他の端末(Windows / Linux)と同期します。
            変更は自動で記録・送信され、同じ箇所を複数の端末で編集したときは、どちらを残すか画面で選べます。
          </p>
        )}
        {status.state === "nested" && (
          <div className="sync-callout note">
            <SyncIcon name="dot" />
            <span>
              上位のフォルダ({status.top})も Git で管理されていますが、この作業フォルダは独立したリポジトリとして同期します。
            </span>
          </div>
        )}
        {status.state === "disabled" && (
          <div className="sync-callout note">
            <SyncIcon name="dot" />
            <span>
              このフォルダは既に Git リポジトリです{status.remote_url ? `(リモート: ${status.remote_url})` : ""}。既存の履歴はそのまま使います。
            </span>
          </div>
        )}

        <h3>
          <span className="step">1</span> 同期先の種類
        </h3>
        <div className="kind-cards" role="radiogroup" aria-label="同期先の種類">
          {KINDS.map((k) => (
            <button
              key={k.id}
              role="radio"
              aria-checked={kind === k.id}
              className={kind === k.id ? "kind-card on" : "kind-card"}
              onClick={() => setKind(k.id)}
            >
              <SyncIcon name={k.id} />
              <span>{REMOTE_LABEL[k.id]}</span>
            </button>
          ))}
        </div>
        <p className="field-hint kind-help">{meta.help}</p>

        <h3>
          <span className="step">2</span> 場所(URL またはパス)
        </h3>
        <form
          className="url-row"
          onSubmit={(e) => {
            e.preventDefault();
            void check();
          }}
        >
          <input
            className="mono"
            value={url}
            placeholder={meta.placeholder}
            spellCheck={false}
            autoFocus
            onChange={(e) => onUrl(e.target.value)}
            onBlur={() => url.trim() && probe.state === "idle" && void check()}
          />
          {kind === "file" && (
            <button type="button" onClick={() => void pickFolder()}>
              フォルダを選択…
            </button>
          )}
          <button type="submit" disabled={!url.trim() || probe.state === "checking"}>
            確認
          </button>
        </form>
        <ProbeResult view={probe} />

        <h3>
          <span className="step">3</span> この端末の名前
        </h3>
        <Field label="" hint="履歴や競合の画面で、どの端末の変更かを表示するのに使います">
          <input value={device} placeholder={defaultDevice || "例: 自宅デスクトップ"} onChange={(e) => setDevice(e.target.value)} />
        </Field>

        {result?.error && <ConnectError error={result.error} url={probe.state === "ok" ? probe.probe.url : url} />}
      </section>
      <footer className="settings-foot">
        {connecting && (
          <span className="sync-progress">
            <SyncIcon name="syncing" className="spin" /> 同期しています…(初回はファイル数に応じて時間がかかります)
          </span>
        )}
        {onCancel && (
          <button onClick={onCancel} disabled={connecting}>
            キャンセル
          </button>
        )}
        <button className="primary" disabled={probe.state !== "ok" || connecting} onClick={() => void start()}>
          {startLabel}
        </button>
      </footer>
    </div>
  );
}

function ProbeResult({ view }: { view: ProbeView }) {
  if (view.state === "idle") return null;
  if (view.state === "checking")
    return (
      <div className="probe checking" role="status">
        <SyncIcon name="syncing" className="spin" /> 接続を確認しています…
      </div>
    );
  if (view.state === "error")
    return (
      <div className="probe error" role="status">
        <SyncIcon name="error" />
        <div>
          <strong>{view.error.message}</strong>
          {view.error.kind === "auth" && <AuthHint url={null} />}
          {view.error.detail && (
            <details>
              <summary>詳細</summary>
              <pre>{view.error.detail}</pre>
            </details>
          )}
        </div>
      </div>
    );
  const p = view.probe;
  const text =
    p.state === "empty"
      ? "空のリポジトリです。このフォルダのメモをアップロードして同期を始めます。"
      : p.state === "will_create"
        ? "ここに新しいリポジトリを作り、このフォルダのメモをアップロードします。"
        : `既にメモがあります(ブランチ ${p.branch})。この端末のメモと合わせます。同じ名前で内容が違うファイルは、この端末の版を別名で残すので消えません。`;
  return (
    <div className="probe ok" role="status">
      <SyncIcon name="check" />
      <div>
        <strong>
          {REMOTE_LABEL[p.kind]} に接続できました
        </strong>
        <span>{text}</span>
      </div>
    </div>
  );
}

function ConnectError({ error, url }: { error: SyncError; url: string }) {
  return (
    <div className="sync-callout danger">
      <SyncIcon name="error" />
      <div className="sync-callout-body">
        <strong>{error.message}</strong>
        {error.kind === "auth" && <AuthHint url={url} />}
        {error.detail && (
          <details>
            <summary>詳細</summary>
            <pre>{error.detail}</pre>
          </details>
        )}
      </div>
    </div>
  );
}

// ---- 設定済み --------------------------------------------------------------------

function ConfiguredView({
  status,
  settings,
  defaultDevice,
  sync,
  onChangeRemote,
  onSaved,
}: {
  status: SyncStatus;
  settings: AppSettings;
  defaultDevice: string;
  sync: SyncApi;
  onChangeRemote(): void;
  onSaved(s: AppSettings): void;
}) {
  const [sx, setSx] = useState<SyncSettings>(settings.sync);
  const [saved, setSaved] = useState<{ ok: boolean; msg: string } | null>(null);
  const kind = status.remote_kind ?? "other";
  const syncing = sync.phase === "syncing";

  const save = async () => {
    try {
      const s = await api.saveSettings({ ...settings, sync: { ...sx, device_name: sx.device_name.trim() } });
      onSaved(s);
      setSaved({ ok: true, msg: "保存しました" });
    } catch (e) {
      setSaved({ ok: false, msg: errorMessage(e) });
    }
  };

  const stop = async () => {
    const ok = await ask(
      "この端末での同期を止めますか?\n作業フォルダのファイルと Git の履歴はそのまま残ります。あとから同じ接続先で再開できます。",
      { title: "同期を停止", kind: "warning" },
    );
    if (!ok) return;
    try {
      await api.syncDisable();
      await sync.refresh();
    } catch (e) {
      setSaved({ ok: false, msg: errorMessage(e) });
    }
  };

  return (
    <div className="settings-body">
      <section className="form sync-setup">
        <div className="remote-card">
          <span className="remote-icon">
            <SyncIcon name={kind} />
          </span>
          <div className="remote-text">
            <strong>
              {REMOTE_LABEL[kind]} · {shortRemote(status.remote_url)}
            </strong>
            <span className="mono">{status.remote_url}</span>
            <span>
              ブランチ {status.branch ?? "-"} ・ 最終同期 {relTime(status.last_sync)}
            </span>
          </div>
          <div className="remote-actions">
            <button className="primary" disabled={syncing} onClick={() => void sync.run({ manual: true })}>
              <SyncIcon name="syncing" className={syncing ? "spin" : undefined} />
              {syncing ? "同期中…" : "今すぐ同期"}
            </button>
            <button onClick={onChangeRemote}>接続先を変更</button>
            <button className="danger-text" onClick={() => void stop()}>
              同期を停止
            </button>
          </div>
        </div>
        {sync.lastError && (
          <div className="sync-callout danger">
            <SyncIcon name="error" />
            <div className="sync-callout-body">
              <span>{sync.lastError.message}</span>
              {sync.lastError.kind === "auth" && <AuthHint url={status.remote_url} />}
            </div>
          </div>
        )}

        <h3>自動同期</h3>
        <label className="check">
          <input type="checkbox" checked={sx.auto} onChange={(e) => setSx({ ...sx, auto: e.target.checked })} />
          自動で同期する(オフにするとステータスバーの「今すぐ同期」か Ctrl+Shift+S でのみ同期)
        </label>
        <fieldset className="sync-auto" disabled={!sx.auto}>
          <div className="grid2">
            <Field label="他の端末の変更を確認する間隔">
              <select value={sx.interval_min} onChange={(e) => setSx({ ...sx, interval_min: Number(e.target.value) })}>
                {INTERVALS.map((m) => (
                  <option key={m} value={m}>
                    {m} 分ごと
                  </option>
                ))}
              </select>
            </Field>
            <Field label="保存してから送るまで" hint="この間に続けて編集すると、まとめて 1 回で送ります">
              <select value={sx.after_save_sec} onChange={(e) => setSx({ ...sx, after_save_sec: Number(e.target.value) })}>
                {AFTER_SAVE.map(([v, l]) => (
                  <option key={v} value={v}>
                    {l}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          <label className="check">
            <input type="checkbox" checked={sx.on_startup} onChange={(e) => setSx({ ...sx, on_startup: e.target.checked })} />
            起動時に同期する
          </label>
          <label className="check">
            <input type="checkbox" checked={sx.on_exit} onChange={(e) => setSx({ ...sx, on_exit: e.target.checked })} />
            終了時、未送信の変更があれば送ってから閉じる(最大 15 秒)
          </label>
        </fieldset>

        <h3>この端末</h3>
        <Field label="端末名" hint="履歴や競合の画面で、どの端末の変更かを表示するのに使います">
          <input value={sx.device_name} placeholder={defaultDevice} onChange={(e) => setSx({ ...sx, device_name: e.target.value })} />
        </Field>
        <p className="field-hint">
          同期の対象外: 検索インデックス(.memo/index.sqlite)・一時ファイル・Office のロックファイル(~$*)。
          改行コードと文字コードは変換せずにそのまま同期します。
        </p>
      </section>
      <footer className="settings-foot">
        {saved && <span className={saved.ok ? "ok" : "error"}>{saved.msg}</span>}
        <button className="primary" onClick={() => void save()}>
          同期設定を保存
        </button>
      </footer>
    </div>
  );
}
