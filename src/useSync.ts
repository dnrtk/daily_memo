// 同期(Git)の状態とスケジュール。
// - 起動時 / 一定間隔 / 保存して編集が止まったとき / ウィンドウに戻ったとき に自動で同期する
// - 実行前に開いているエディタを保存し、結果(他の端末の変更)は onApplied で画面に反映する
// - 自動同期の失敗(オフライン等)は状態表示だけにして、トーストで邪魔をしない
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage } from "./api";
import type { SyncError, SyncOutcome, SyncSettings, SyncStatus } from "./types";

export type SyncPhase = "idle" | "syncing";

interface Args {
  /** 作業フォルダのルート(切り替えたら状態を取り直す) */
  root: string | null;
  settings: SyncSettings | undefined;
  /** 同期前にエディタを保存する */
  flush(): Promise<void>;
  onApplied(o: SyncOutcome, manual: boolean): void;
}

export interface SyncApi {
  status: SyncStatus | null;
  phase: SyncPhase;
  lastError: SyncError | null;
  /** 直前の同期が成功した時刻(完了アニメーション用) */
  doneAt: number;
  run(opts?: { manual?: boolean; message?: string }): Promise<SyncOutcome | null>;
  refresh(): Promise<void>;
  /** 保存のたびに呼ぶ(未送信数の更新と、編集が止まったあとの自動同期) */
  noteSaved(): void;
  /** 終了時: 未送信の変更があれば送る(最大 timeoutMs 待つ) */
  flushOnExit(timeoutMs: number): Promise<void>;
  apply(o: SyncOutcome): void;
}

const FOCUS_MIN_GAP_MS = 2 * 60 * 1000;

export function useSync({ root, settings, flush, onApplied }: Args): SyncApi {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [phase, setPhase] = useState<SyncPhase>("idle");
  const [lastError, setLastError] = useState<SyncError | null>(null);
  const [doneAt, setDoneAt] = useState(0);

  const inFlight = useRef<Promise<SyncOutcome | null> | null>(null);
  const lastRun = useRef(0);
  const saveTimer = useRef(0);
  const statusTimer = useRef(0);
  const statusRef = useRef(status);
  statusRef.current = status;
  const cb = useRef({ flush, onApplied, settings });
  cb.current = { flush, onApplied, settings };

  const ready = status?.state === "ready";
  const auto = !!settings?.auto && ready;

  const refresh = useCallback(async () => {
    if (!root) {
      setStatus(null);
      return;
    }
    try {
      setStatus(await api.syncStatus());
    } catch {
      /* 状態が取れなくても編集は続けられる */
    }
  }, [root]);

  const apply = useCallback((o: SyncOutcome) => {
    setStatus(o.status);
    if (o.error && o.error.kind !== "busy") setLastError(o.error);
    else if (!o.error) setLastError(null);
  }, []);

  const run = useCallback(
    (opts: { manual?: boolean; message?: string } = {}): Promise<SyncOutcome | null> => {
      if (!root) return Promise.resolve(null);
      // 実行中なら、その結果を待つ(手動と自動が重なっても 1 回にまとめる)
      if (inFlight.current) return inFlight.current;
      const st = statusRef.current;
      // 競合の解消待ちのあいだは自動では動かさない(解消画面から手動で続ける)
      if (!opts.manual && st?.merging && st.conflicts.length > 0) return Promise.resolve(null);
      setPhase("syncing");
      window.clearTimeout(saveTimer.current);
      const p = (async () => {
        try {
          await cb.current.flush();
          const o = await api.syncRun(opts.message ?? null);
          lastRun.current = Date.now();
          apply(o);
          if (!o.error) setDoneAt(Date.now());
          cb.current.onApplied(o, !!opts.manual);
          return o;
        } catch (e) {
          setLastError({ kind: "other", message: errorMessage(e) });
          return null;
        } finally {
          inFlight.current = null;
          setPhase("idle");
        }
      })();
      inFlight.current = p;
      return p;
    },
    [root, apply],
  );

  const runRef = useRef(run);
  runRef.current = run;

  // 作業フォルダが変わったら状態を取り直し、起動時同期
  useEffect(() => {
    setLastError(null);
    let cancelled = false;
    (async () => {
      if (!root) {
        setStatus(null);
        return;
      }
      try {
        const st = await api.syncStatus();
        if (cancelled) return;
        setStatus(st);
        const s = cb.current.settings;
        if (st.state === "ready" && s?.auto && s.on_startup) void runRef.current();
      } catch {
        /* noop */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [root]);

  // 一定間隔で他の端末の変更を取りに行く
  useEffect(() => {
    if (!auto || !settings) return;
    const ms = Math.max(1, settings.interval_min) * 60 * 1000;
    const id = window.setInterval(() => void runRef.current(), ms);
    return () => window.clearInterval(id);
  }, [auto, settings?.interval_min]);

  // ウィンドウに戻ってきたとき(しばらく同期していなければ)
  useEffect(() => {
    if (!auto) return;
    const onFocus = () => {
      if (Date.now() - lastRun.current > FOCUS_MIN_GAP_MS) void runRef.current();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [auto]);

  useEffect(
    () => () => {
      window.clearTimeout(saveTimer.current);
      window.clearTimeout(statusTimer.current);
    },
    [],
  );

  const noteSaved = useCallback(() => {
    if (statusRef.current?.state !== "ready") return;
    // 未送信の数は少し待ってからまとめて取り直す(連続保存で git を何度も呼ばない)
    window.clearTimeout(statusTimer.current);
    statusTimer.current = window.setTimeout(() => {
      if (!inFlight.current) void refresh();
    }, 1500);
    const s = cb.current.settings;
    if (!s?.auto) return;
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => void runRef.current(), Math.max(5, s.after_save_sec) * 1000);
  }, [refresh]);

  const flushOnExit = useCallback(
    async (timeoutMs: number) => {
      const s = cb.current.settings;
      const st = statusRef.current;
      if (!s?.on_exit || st?.state !== "ready" || !st.remote_url) return;
      await cb.current.flush();
      let fresh: SyncStatus;
      try {
        fresh = await api.syncStatus();
      } catch {
        return;
      }
      if (fresh.merging || (fresh.pending.length === 0 && fresh.ahead === 0)) return;
      await Promise.race([runRef.current(), new Promise((r) => window.setTimeout(r, timeoutMs))]);
    },
    [],
  );

  return { status, phase, lastError, doneAt, run, refresh, noteSaved, flushOnExit, apply };
}
