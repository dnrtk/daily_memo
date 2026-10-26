// 設定: カテゴリ定義(作業フォルダごと) / LLM 接続 / エディタ(マシンごと)
import { useState, type ReactNode } from "react";
import { api, errorMessage } from "../api";
import type { AppSettings, Category, CategoryConfig, Provider, WorkspaceInfo } from "../types";

type Tab = "categories" | "llm" | "search" | "editor";

interface Props {
  workspace: WorkspaceInfo | null;
  settings: AppSettings;
  onClose(): void;
  onWorkspaceSaved(ws: WorkspaceInfo): void;
  onSettingsSaved(s: AppSettings): void;
}

export function SettingsDialog({ workspace, settings, onClose, onWorkspaceSaved, onSettingsSaved }: Props) {
  const [tab, setTab] = useState<Tab>(workspace ? "categories" : "llm");
  return (
    <div className="backdrop" onMouseDown={onClose}>
      <div
        className="dialog settings"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onClose()}
      >
        <header className="settings-head">
          <h2>設定</h2>
          <nav className="seg" role="tablist">
            {(
              [
                ["categories", "カテゴリ"],
                ["llm", "LLM 接続"],
                ["search", "検索"],
                ["editor", "エディタ"],
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                role="tab"
                aria-selected={tab === id}
                className={tab === id ? "on" : ""}
                disabled={id === "categories" && !workspace}
                onClick={() => setTab(id)}
              >
                {label}
              </button>
            ))}
          </nav>
          <button className="icon" onClick={onClose} aria-label="閉じる">
            ×
          </button>
        </header>
        {tab === "categories" && workspace && (
          <CategoriesTab config={workspace.categories} onSaved={onWorkspaceSaved} />
        )}
        {tab === "llm" && <LlmTab settings={settings} onSaved={onSettingsSaved} />}
        {tab === "search" && <SearchTab settings={settings} onSaved={onSettingsSaved} />}
        {tab === "editor" && <EditorTab settings={settings} onSaved={onSettingsSaved} />}
      </div>
    </div>
  );
}

// ---- カテゴリ ------------------------------------------------------------------

function CategoriesTab({ config, onSaved }: { config: CategoryConfig; onSaved(ws: WorkspaceInfo): void }) {
  const [draft, setDraft] = useState<CategoryConfig>(() => structuredClone(config));
  const [selected, setSelected] = useState(0);
  const [status, setStatus] = useState<{ ok: boolean; msg: string } | null>(null);
  const cats = draft.categories;
  const cur = cats[selected];

  const patch = (p: Partial<Category>) =>
    setDraft((d) => ({
      ...d,
      categories: d.categories.map((c, i) => (i === selected ? { ...c, ...p } : c)),
    }));

  const add = () => {
    const n = cats.length + 1;
    const c: Category = {
      id: `cat${n}`,
      name: `新しいカテゴリ${n}`,
      dir: `新しいカテゴリ${n}`,
      description: "",
      examples: [],
      filename: "{yyyy}{MM}{dd}-{slug}.md",
      mode: "new_file",
      file_template: "# {title}\n\n",
    };
    setDraft((d) => ({ ...d, categories: [...d.categories, c] }));
    setSelected(cats.length);
  };

  const remove = () => {
    if (!cur) return;
    setDraft((d) => ({
      ...d,
      daily_category: d.daily_category === cur.id ? null : d.daily_category,
      categories: d.categories.filter((_, i) => i !== selected),
    }));
    setSelected((s) => Math.max(0, s - 1));
  };

  const move = (delta: number) => {
    const to = selected + delta;
    if (to < 0 || to >= cats.length) return;
    setDraft((d) => {
      const list = [...d.categories];
      [list[selected], list[to]] = [list[to], list[selected]];
      return { ...d, categories: list };
    });
    setSelected(to);
  };

  const save = async () => {
    try {
      const ws = await api.saveCategories(draft);
      setDraft(structuredClone(ws.categories));
      onSaved(ws);
      setStatus({ ok: true, msg: "保存しました" });
    } catch (e) {
      setStatus({ ok: false, msg: errorMessage(e) });
    }
  };

  const isNew = cur && !config.categories.some((c) => c.id === cur.id);

  return (
    <div className="settings-body split">
      <aside className="cat-list">
        <ul>
          {cats.map((c, i) => (
            <li key={`${c.id}-${i}`}>
              <button className={i === selected ? "on" : ""} onClick={() => setSelected(i)}>
                <span>{c.name}</span>
                {draft.daily_category === c.id && <em>日次</em>}
              </button>
            </li>
          ))}
        </ul>
        <div className="row-actions">
          <button onClick={add}>＋ 追加</button>
          <button onClick={() => move(-1)} aria-label="上へ">
            ↑
          </button>
          <button onClick={() => move(1)} aria-label="下へ">
            ↓
          </button>
        </div>
      </aside>

      {cur ? (
        <section className="form">
          <div className="grid2">
            <Field label="表示名">
              <input value={cur.name} onChange={(e) => patch({ name: e.target.value })} />
            </Field>
            <Field label="ID" hint={isNew ? "英数字・-・_。作成後は変えない" : "既存カテゴリの ID は変更不可"}>
              <input value={cur.id} disabled={!isNew} onChange={(e) => patch({ id: e.target.value })} />
            </Field>
          </div>
          <Field label="フォルダ" hint="作業フォルダからの相対パス。変更するとフォルダごと移動します">
            <input value={cur.dir} onChange={(e) => patch({ dir: e.target.value })} />
          </Field>
          <Field label="定義" hint="このカテゴリに入れるべき内容。LLM の振り分け判断に使います">
            <textarea
              rows={3}
              value={cur.description}
              onChange={(e) => patch({ description: e.target.value })}
            />
          </Field>
          <Field label="例(1行1件)" hint="典型的なメモの例。振り分け精度が上がります">
            <textarea
              rows={3}
              value={cur.examples.join("\n")}
              onChange={(e) => patch({ examples: e.target.value.split("\n").filter((l) => l.trim()) })}
            />
          </Field>
          <div className="grid2">
            <Field label="保存方式">
              <select value={cur.mode} onChange={(e) => patch({ mode: e.target.value as Category["mode"] })}>
                <option value="new_file">毎回新しいファイル</option>
                <option value="append">1ファイルに追記</option>
              </select>
            </Field>
            <Field label="ファイル名" hint="{yyyy} {MM} {dd} {HH} {mm} {slug} {title}">
              <input value={cur.filename} onChange={(e) => patch({ filename: e.target.value })} />
            </Field>
          </div>
          <Field label="新規ファイルの雛形">
            <textarea
              rows={2}
              className="mono"
              value={cur.file_template ?? ""}
              onChange={(e) => patch({ file_template: e.target.value || null })}
            />
          </Field>
          {cur.mode === "append" && (
            <Field label="追記エントリの書式" hint="{content} が本文">
              <textarea
                rows={2}
                className="mono"
                value={cur.entry_template ?? ""}
                onChange={(e) => patch({ entry_template: e.target.value || null })}
              />
            </Field>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={draft.daily_category === cur.id}
              onChange={(e) => setDraft((d) => ({ ...d, daily_category: e.target.checked ? cur.id : null }))}
            />
            「今日のメモ」で開くカテゴリにする
          </label>
          <div className="form-foot">
            <button className="danger-text" onClick={remove}>
              このカテゴリを外す(フォルダとファイルは残ります)
            </button>
          </div>
        </section>
      ) : (
        <section className="form empty">カテゴリがありません</section>
      )}

      <footer className="settings-foot">
        {status && <span className={status.ok ? "ok" : "error"}>{status.msg}</span>}
        <button className="primary" onClick={() => void save()}>
          カテゴリを保存
        </button>
      </footer>
    </div>
  );
}

// ---- LLM 接続 ------------------------------------------------------------------

function LlmTab({ settings, onSaved }: { settings: AppSettings; onSaved(s: AppSettings): void }) {
  const [draft, setDraft] = useState<AppSettings>(() => structuredClone(settings));
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [tests, setTests] = useState<Record<string, { ok: boolean; msg: string }>>({});
  const [status, setStatus] = useState<{ ok: boolean; msg: string } | null>(null);

  const patchProvider = (i: number, p: Partial<Provider>) =>
    setDraft((d) => ({ ...d, providers: d.providers.map((x, j) => (j === i ? { ...x, ...p } : x)) }));

  const addProvider = () =>
    setDraft((d) => ({
      ...d,
      providers: [
        ...d.providers,
        {
          id: `server${d.providers.length + 1}`,
          kind: "openai_compat",
          base_url: "http://192.168.0.10:8000/v1",
          api_key: null,
          timeout_secs: 60,
        },
      ],
    }));

  const test = async (p: Provider) => {
    setTests((t) => ({ ...t, [p.id]: { ok: true, msg: "接続中…" } }));
    try {
      const list = await api.testProvider(p);
      setModels((m) => ({ ...m, [p.id]: list }));
      setTests((t) => ({ ...t, [p.id]: { ok: true, msg: `接続OK・${list.length} モデル` } }));
    } catch (e) {
      setTests((t) => ({ ...t, [p.id]: { ok: false, msg: errorMessage(e) } }));
    }
  };

  const save = async () => {
    try {
      // エディタ設定は別タブで変わっている可能性があるので、自分の担当分だけ上書きする
      const s = await api.saveSettings({ ...settings, providers: draft.providers, roles: draft.roles });
      onSaved(s);
      setStatus({ ok: true, msg: "保存しました" });
    } catch (e) {
      setStatus({ ok: false, msg: errorMessage(e) });
    }
  };

  const roleRow = (role: "embedding" | "chat", label: string, hint: string) => {
    const r = draft.roles[role];
    const listId = `models-${role}`;
    return (
      <div className="grid2">
        <Field label={`${label}: 接続先`} hint={hint}>
          <select
            value={r.provider}
            onChange={(e) =>
              setDraft((d) => ({ ...d, roles: { ...d.roles, [role]: { ...r, provider: e.target.value } } }))
            }
          >
            {draft.providers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.id}
              </option>
            ))}
          </select>
        </Field>
        <Field label={`${label}: モデル`}>
          <input
            list={listId}
            value={r.model}
            onChange={(e) =>
              setDraft((d) => ({ ...d, roles: { ...d.roles, [role]: { ...r, model: e.target.value } } }))
            }
          />
          <datalist id={listId}>
            {(models[r.provider] ?? []).map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
        </Field>
      </div>
    );
  };

  return (
    <div className="settings-body">
      <section className="form">
        <h3>接続先</h3>
        {draft.providers.map((p, i) => (
          <div className="provider" key={i}>
            <div className="grid3">
              <Field label="ID">
                <input value={p.id} onChange={(e) => patchProvider(i, { id: e.target.value })} />
              </Field>
              <Field label="種類">
                <select
                  value={p.kind}
                  onChange={(e) => patchProvider(i, { kind: e.target.value as Provider["kind"] })}
                >
                  <option value="ollama">Ollama</option>
                  <option value="openai_compat">OpenAI 互換(vLLM 等)</option>
                </select>
              </Field>
              <Field label="タイムアウト(秒)">
                <input
                  type="number"
                  min={1}
                  value={p.timeout_secs}
                  onChange={(e) => patchProvider(i, { timeout_secs: Number(e.target.value) || 60 })}
                />
              </Field>
            </div>
            <div className="grid2">
              <Field label="URL" hint={p.kind === "ollama" ? "例: http://127.0.0.1:11434" : "例: http://192.168.0.10:8000/v1"}>
                <input value={p.base_url} onChange={(e) => patchProvider(i, { base_url: e.target.value })} />
              </Field>
              <Field label="API キー" hint="設定ファイルに平文で保存されます">
                <input
                  type="password"
                  value={p.api_key ?? ""}
                  onChange={(e) => patchProvider(i, { api_key: e.target.value || null })}
                />
              </Field>
            </div>
            <div className="row-actions">
              <button onClick={() => void test(p)}>接続テスト</button>
              {tests[p.id] && <span className={tests[p.id].ok ? "ok" : "error"}>{tests[p.id].msg}</span>}
              <span className="spacer" />
              {draft.providers.length > 1 && (
                <button
                  className="danger-text"
                  onClick={() => setDraft((d) => ({ ...d, providers: d.providers.filter((_, j) => j !== i) }))}
                >
                  削除
                </button>
              )}
            </div>
          </div>
        ))}
        <button onClick={addProvider}>＋ 接続先を追加</button>

        <h3>役割ごとのモデル</h3>
        {roleRow("embedding", "埋め込み", "変更するとベクトルの作り直しが必要になります")}
        <div className="grid2">
          <Field label="埋め込み: クエリ接頭辞" hint="ruri-v3 なら「検索クエリ: 」。bge-m3 は空のまま">
            <input
              value={draft.roles.embedding.query_prefix ?? ""}
              onChange={(e) =>
                setDraft((d) => ({ ...d, roles: { ...d.roles, embedding: { ...d.roles.embedding, query_prefix: e.target.value } } }))
              }
            />
          </Field>
          <Field label="埋め込み: 文書接頭辞" hint="ruri-v3 なら「検索文書: 」">
            <input
              value={draft.roles.embedding.document_prefix ?? ""}
              onChange={(e) =>
                setDraft((d) => ({ ...d, roles: { ...d.roles, embedding: { ...d.roles.embedding, document_prefix: e.target.value } } }))
              }
            />
          </Field>
        </div>
        {roleRow("chat", "LLM", "保存先の振り分け・検索クエリの解釈に使います")}
      </section>
      <footer className="settings-foot">
        {status && <span className={status.ok ? "ok" : "error"}>{status.msg}</span>}
        <button className="primary" onClick={() => void save()}>
          LLM 設定を保存
        </button>
      </footer>
    </div>
  );
}

// ---- 検索 ----------------------------------------------------------------------

function SearchTab({ settings, onSaved }: { settings: AppSettings; onSaved(s: AppSettings): void }) {
  const [sx, setSx] = useState(settings.search);
  const [status, setStatus] = useState<{ ok: boolean; msg: string } | null>(null);
  const save = async () => {
    try {
      const s = await api.saveSettings({ ...settings, search: sx });
      onSaved(s);
      setStatus({ ok: true, msg: "保存しました(除外設定は次の再スキャンから反映)" });
    } catch (e) {
      setStatus({ ok: false, msg: errorMessage(e) });
    }
  };
  return (
    <div className="settings-body">
      <section className="form narrow">
        <label className="check">
          <input
            type="checkbox"
            checked={sx.use_llm_query}
            onChange={(e) => setSx({ ...sx, use_llm_query: e.target.checked })}
          />
          検索文を LLM で解釈する(期間・ファイル種類・カテゴリを自動で絞り込む)
        </label>
        <Field label="インデックスするファイルの上限(MB)">
          <input
            type="number"
            min={1}
            value={sx.max_file_mb}
            onChange={(e) => setSx({ ...sx, max_file_mb: Math.max(1, Number(e.target.value) || 50) })}
          />
        </Field>
        <Field label="除外するフォルダ名(1行1件)" hint="どの階層でも名前が一致したら除外。「.」で始まるフォルダは常に除外">
          <textarea
            rows={4}
            className="mono"
            value={sx.exclude_dirs.join("\n")}
            onChange={(e) => setSx({ ...sx, exclude_dirs: e.target.value.split("\n").map((l) => l.trim()).filter(Boolean) })}
          />
        </Field>
        <p className="field-hint">
          対象: Markdown / テキスト / CSV / HTML / Word / Excel / PowerPoint / PDF。インデックスは作業フォルダの
          .memo/index.sqlite に保存され、ファイルの変更は自動で反映されます。
        </p>
      </section>
      <footer className="settings-foot">
        {status && <span className={status.ok ? "ok" : "error"}>{status.msg}</span>}
        <button className="primary" onClick={() => void save()}>
          検索設定を保存
        </button>
      </footer>
    </div>
  );
}

// ---- エディタ ------------------------------------------------------------------

function EditorTab({ settings, onSaved }: { settings: AppSettings; onSaved(s: AppSettings): void }) {
  const [ed, setEd] = useState(settings.editor);
  const [shell, setShell] = useState(settings.terminal?.shell ?? "");
  const [status, setStatus] = useState<{ ok: boolean; msg: string } | null>(null);
  const save = async () => {
    try {
      const s = await api.saveSettings({ ...settings, editor: ed, terminal: { shell } });
      onSaved(s);
      setStatus({ ok: true, msg: "保存しました" });
    } catch (e) {
      setStatus({ ok: false, msg: errorMessage(e) });
    }
  };
  return (
    <div className="settings-body">
      <section className="form narrow">
        <Field label="自動保存(ミリ秒)" hint="入力が止まってから保存するまでの時間。0 でオフ">
          <input
            type="number"
            min={0}
            step={250}
            value={ed.autosave_ms}
            onChange={(e) => setEd({ ...ed, autosave_ms: Math.max(0, Number(e.target.value)) })}
          />
        </Field>
        <Field label="文字サイズ(px)">
          <input
            type="number"
            min={10}
            max={28}
            value={ed.font_size}
            onChange={(e) => setEd({ ...ed, font_size: Number(e.target.value) || 14 })}
          />
        </Field>
        <label className="check">
          <input
            type="checkbox"
            checked={ed.line_wrapping}
            onChange={(e) => setEd({ ...ed, line_wrapping: e.target.checked })}
          />
          折り返して表示
        </label>
        <Field
          label="ターミナルのシェル"
          hint="空なら Windows は PowerShell(PowerShell 7 があれば優先)、Linux はログインシェル。例: cmd.exe / wsl.exe / pwsh -NoLogo。新しく開くタブから反映"
        >
          <input className="mono" value={shell} placeholder="(自動)" onChange={(e) => setShell(e.target.value)} />
        </Field>
      </section>
      <footer className="settings-foot">
        {status && <span className={status.ok ? "ok" : "error"}>{status.msg}</span>}
        <button className="primary" onClick={() => void save()}>
          エディタ設定を保存
        </button>
      </footer>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}
