// 他の端末との競合を解消する画面。
// 左: 競合しているファイル / 右: 箇所ごとに「この端末」「相手」「両方」を選ぶ。
// 選び終わったら結果を確認(必要なら手で直して)解消し、全部片付いたら同期を続ける。
// キーボード: J / K で箇所を移動、1 = この端末、2 = 相手、3 = 両方(この端末が先)、4 = 両方(相手が先)
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage } from "../api";
import type { ConflictDetail, ConflictInfo, ConflictKind, Resolution, Segment } from "../types";
import { relTime, SyncIcon } from "./SyncIcons";

type Choice = "ours" | "theirs" | "ours_first" | "theirs_first";

interface Props {
  onClose(): void;
  /** すべて解消できたら呼ぶ(同期を続ける) */
  onAllResolved(): void;
  /** 解消でファイルが書き換わったとき(開いているタブの再読み込み) */
  onFileChanged(path: string): void;
}

const KIND_TEXT: Record<ConflictKind, string> = {
  both_modified: "両方で編集",
  deleted_by_us: "この端末で削除",
  deleted_by_them: "相手が削除",
};

const ensureNl = (s: string) => (s === "" || s.endsWith("\n") ? s : `${s}\n`);

function compose(segments: Segment[], choices: (Choice | undefined)[]): string | null {
  let out = "";
  let ci = 0;
  for (const s of segments) {
    if (s.type === "same") {
      out += s.text;
      continue;
    }
    const c = choices[ci++];
    if (!c) return null;
    if (c === "ours") out += s.ours;
    else if (c === "theirs") out += s.theirs;
    else if (c === "ours_first") out += ensureNl(s.ours) + s.theirs;
    else out += ensureNl(s.theirs) + s.ours;
  }
  return out;
}

export function ConflictDialog({ onClose, onAllResolved, onFileChanged }: Props) {
  const [list, setList] = useState<ConflictInfo[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [doneCount, setDoneCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [finishing, setFinishing] = useState(false);

  const load = useCallback(async () => {
    try {
      const l = await api.syncConflicts();
      setList(l);
      setSelected((cur) => (cur && l.some((c) => c.path === cur) ? cur : (l[0]?.path ?? null)));
      return l;
    } catch (e) {
      setError(errorMessage(e));
      return null;
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const resolve = async (path: string, r: Resolution) => {
    setError(null);
    try {
      const left = await api.syncResolve(path, r);
      onFileChanged(path);
      setDoneCount((n) => n + 1);
      if (left === 0) {
        setFinishing(true);
        onAllResolved();
        return;
      }
      await load();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const cur = list?.find((c) => c.path === selected) ?? null;

  return (
    <div className="backdrop" onMouseDown={() => !finishing && onClose()}>
      <div
        className="dialog conflicts"
        role="dialog"
        aria-label="競合の解消"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && !finishing && onClose()}
      >
        <header className="settings-head">
          <h2>
            <SyncIcon name="warning" className="tone-danger" /> 他の端末との競合を解消
          </h2>
          {list && list.length > 0 && (
            <span className="conflict-count">
              残り {list.length} 件{doneCount > 0 && ` ・ 解消済み ${doneCount} 件`}
            </span>
          )}
          <button className="icon" onClick={onClose} aria-label="閉じる" disabled={finishing}>
            ×
          </button>
        </header>
        {finishing ? (
          <div className="conflict-finish">
            <SyncIcon name="check" className="pop" />
            <strong>すべて解消しました</strong>
            <span>続けて同期しています…</span>
          </div>
        ) : list === null ? (
          <div className="conflict-finish">
            <span>{error ?? "読み込み中…"}</span>
          </div>
        ) : list.length === 0 ? (
          <div className="conflict-finish">
            <SyncIcon name="check" />
            <strong>競合はありません</strong>
            <button onClick={onAllResolved}>同期を続ける</button>
          </div>
        ) : (
          <div className="conflict-body">
            <aside className="conflict-list">
              <ul>
                {list.map((c) => (
                  <li key={c.path}>
                    <button className={c.path === selected ? "on" : ""} onClick={() => setSelected(c.path)} title={c.path}>
                      <span className="conflict-name">{c.path.split("/").pop()}</span>
                      <span className="conflict-dir">{c.path.split("/").slice(0, -1).join("/") || "(ルート)"}</span>
                      <span className={`conflict-kind k-${c.kind}`}>{KIND_TEXT[c.kind]}</span>
                    </button>
                  </li>
                ))}
              </ul>
              <p className="conflict-tip">
                解消するまで同期は一時停止しています。作業ツリーには<strong>この端末の版</strong>が残っているので、
                閉じても編集は続けられます。
              </p>
            </aside>
            {cur && <ConflictView key={cur.path} info={cur} onResolve={(r) => void resolve(cur.path, r)} />}
          </div>
        )}
        {error && !finishing && <p className="conflict-error error">{error}</p>}
      </div>
    </div>
  );
}

function ConflictView({ info, onResolve }: { info: ConflictInfo; onResolve(r: Resolution): void }) {
  const [detail, setDetail] = useState<ConflictDetail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api
      .syncConflictDetail(info.path)
      .then(setDetail)
      .catch((e) => setErr(errorMessage(e)));
  }, [info.path]);

  if (err) return <section className="conflict-main"><p className="error">{err}</p></section>;
  if (!detail) return <section className="conflict-main"><p className="field-hint">読み込み中…</p></section>;
  return detail.kind === "both_modified" ? (
    <HunkView detail={detail} onResolve={onResolve} />
  ) : (
    <DeleteView detail={detail} onResolve={onResolve} />
  );
}

function Legend({ d }: { d: ConflictDetail }) {
  return (
    <div className="legend">
      <span className="lg ours">
        <i /> この端末({d.ours_label})
      </span>
      <span className="lg theirs">
        <i /> {d.theirs_label}
        {d.theirs_time ? `(${relTime(d.theirs_time)})` : ""}
      </span>
    </div>
  );
}

function HunkView({ detail, onResolve }: { detail: ConflictDetail; onResolve(r: Resolution): void }) {
  const hunks = useMemo(() => detail.segments.filter((s) => s.type === "conflict").length, [detail]);
  const [choices, setChoices] = useState<(Choice | undefined)[]>(() => Array(hunks).fill(undefined));
  const [focus, setFocus] = useState(0);
  const [editing, setEditing] = useState<string | null>(null);
  const refs = useRef<(HTMLDivElement | null)[]>([]);
  const remaining = choices.filter((c) => !c).length;
  const result = compose(detail.segments, choices);
  const theirs = detail.theirs_label;

  const choose = (i: number, c: Choice) => {
    setChoices((cs) => cs.map((x, j) => (j === i ? c : x)));
    // 次の未選択の箇所へ
    const next = choices.findIndex((x, j) => j > i && !x);
    if (next >= 0) {
      setFocus(next);
      refs.current[next]?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  };
  const all = (c: Choice) => setChoices(Array(hunks).fill(c));

  const onKey = (e: React.KeyboardEvent) => {
    if (editing !== null || e.ctrlKey || e.altKey || e.metaKey) return;
    const target = e.target as HTMLElement;
    if (target.tagName === "TEXTAREA" || target.tagName === "INPUT") return;
    const map: Record<string, Choice> = { "1": "ours", "2": "theirs", "3": "ours_first", "4": "theirs_first" };
    if (map[e.key]) {
      e.preventDefault();
      choose(focus, map[e.key]);
    } else if (e.key === "j" || e.key === "ArrowDown") {
      e.preventDefault();
      const n = Math.min(hunks - 1, focus + 1);
      setFocus(n);
      refs.current[n]?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    } else if (e.key === "k" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = Math.max(0, focus - 1);
      setFocus(n);
      refs.current[n]?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  };

  let hi = -1;
  return (
    <section className="conflict-main" tabIndex={0} onKeyDown={onKey}>
      <div className="conflict-toolbar">
        <strong className="conflict-path">{detail.path}</strong>
        <Legend d={detail} />
      </div>
      <div className="conflict-quick">
        <span>まとめて:</span>
        <button onClick={() => all("ours")}>すべてこの端末の版</button>
        <button onClick={() => all("theirs")}>すべて {theirs} の版</button>
        <button onClick={() => onResolve({ action: "both" })} title={`この端末の版はそのまま、${theirs} の版を別名のファイルで残します`}>
          両方のファイルとして残す
        </button>
      </div>

      {editing !== null ? (
        <textarea className="conflict-edit mono" value={editing} onChange={(e) => setEditing(e.target.value)} autoFocus spellCheck={false} />
      ) : (
        <div className="hunks">
          {detail.segments.map((s, i) => {
            if (s.type === "same") return <SameBlock key={i} text={s.text} />;
            hi++;
            const idx = hi;
            const c = choices[idx];
            return (
              <div
                key={i}
                ref={(el) => {
                  refs.current[idx] = el;
                }}
                className={`hunk${c ? " chosen" : ""}${focus === idx ? " focus" : ""}`}
                onMouseDown={() => setFocus(idx)}
              >
                <div className="hunk-head">
                  <span className="hunk-no">
                    {c ? <SyncIcon name="check" /> : idx + 1}
                  </span>
                  <span>
                    箇所 {idx + 1} / {hunks}
                  </span>
                  <span className="spacer" />
                  <button className={c === "ours_first" ? "mini on" : "mini"} onClick={() => choose(idx, "ours_first")} title="キー 3">
                    両方(この端末 → 相手)
                  </button>
                  <button className={c === "theirs_first" ? "mini on" : "mini"} onClick={() => choose(idx, "theirs_first")} title="キー 4">
                    両方(相手 → この端末)
                  </button>
                </div>
                <div className="hunk-sides">
                  <Side
                    who="ours"
                    label="この端末"
                    text={s.ours}
                    picked={c === "ours" || c === "ours_first" || c === "theirs_first"}
                    dim={c === "theirs"}
                    onPick={() => choose(idx, "ours")}
                    keyHint="1"
                  />
                  <Side
                    who="theirs"
                    label={theirs}
                    text={s.theirs}
                    picked={c === "theirs" || c === "ours_first" || c === "theirs_first"}
                    dim={c === "ours"}
                    onPick={() => choose(idx, "theirs")}
                    keyHint="2"
                  />
                </div>
              </div>
            );
          })}
          {hunks === 0 && <p className="field-hint">重なる編集はありません。この内容で解消できます。</p>}
        </div>
      )}

      <footer className="conflict-foot">
        <span className="field-hint">
          {editing !== null
            ? "結果を直接編集しています"
            : remaining > 0
              ? `あと ${remaining} 箇所を選んでください(1 / 2 / 3 / 4 キーでも選べます)`
              : "すべて選びました"}
        </span>
        <span className="spacer" />
        {editing === null ? (
          <button disabled={result === null} onClick={() => setEditing(result ?? "")}>
            結果を確認・編集
          </button>
        ) : (
          <button onClick={() => setEditing(null)}>選択に戻る</button>
        )}
        <button
          className="primary"
          disabled={editing === null && result === null}
          onClick={() => onResolve({ action: "content", text: editing ?? result ?? "" })}
        >
          この内容で解消
        </button>
      </footer>
    </section>
  );
}

function Side({
  who,
  label,
  text,
  picked,
  dim,
  onPick,
  keyHint,
}: {
  who: "ours" | "theirs";
  label: string;
  text: string;
  picked: boolean;
  dim: boolean;
  onPick(): void;
  keyHint: string;
}) {
  return (
    <button className={`side side-${who}${picked ? " picked" : ""}${dim ? " dim" : ""}`} onClick={onPick} title={`キー ${keyHint}`}>
      <span className="pick-head">
        <i className="side-dot" />
        {label}
        {picked && <SyncIcon name="check" />}
      </span>
      <pre>{text === "" ? <em>(なし)</em> : text}</pre>
    </button>
  );
}

/** 共通部分。長いときは前後 3 行だけ見せて畳む */
function SameBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const lines = text.replace(/\n$/, "").split("\n");
  const KEEP = 3;
  if (open || lines.length <= KEEP * 2 + 1) return <pre className="same">{text}</pre>;
  const hidden = lines.length - KEEP * 2;
  return (
    <div className="same-wrap">
      <pre className="same">{lines.slice(0, KEEP).join("\n")}</pre>
      <button className="same-fold" onClick={() => setOpen(true)}>
        ⋯ 変更のない {hidden} 行を表示
      </button>
      <pre className="same">{lines.slice(-KEEP).join("\n")}</pre>
    </div>
  );
}

function DeleteView({ detail, onResolve }: { detail: ConflictDetail; onResolve(r: Resolution): void }) {
  const theirs = detail.theirs_label;
  const byThem = detail.kind === "deleted_by_them";
  return (
    <section className="conflict-main">
      <div className="conflict-toolbar">
        <strong className="conflict-path">{detail.path}</strong>
        <Legend d={detail} />
      </div>
      <div className="sync-callout note">
        <SyncIcon name="warning" />
        <span>
          {byThem
            ? `この端末で編集したファイルを、${theirs} が削除しました。`
            : `この端末で削除したファイルを、${theirs} が編集していました。`}
        </span>
      </div>
      <p className="field-hint">{byThem ? "この端末の内容:" : `${theirs} の内容:`}</p>
      <pre className="remaining">{detail.remaining_text ?? ""}</pre>
      <footer className="conflict-foot">
        <span className="spacer" />
        {byThem ? (
          <>
            <button onClick={() => onResolve({ action: "theirs" })}>削除する</button>
            <button className="primary" onClick={() => onResolve({ action: "ours" })}>
              残す
            </button>
          </>
        ) : (
          <>
            <button onClick={() => onResolve({ action: "ours" })}>削除のままにする</button>
            <button className="primary" onClick={() => onResolve({ action: "theirs" })}>
              {theirs} の版で戻す
            </button>
          </>
        )}
      </footer>
    </section>
  );
}
