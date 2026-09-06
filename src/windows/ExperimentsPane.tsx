import type React from "react";
import { useEffect, useMemo, useState } from "react";
import type { AiConfig } from "../bindings/AiConfig";
import type { Experiment } from "../bindings/Experiment";
import type { RunRecord } from "../bindings/RunRecord";
import type { Settings } from "../bindings/Settings";
import type { TrackedPr } from "../bindings/TrackedPr";
import { ipc, onExperimentChanged, onExperimentProgress } from "../lib/ipc";
import { EffortSlider, StepSlider, TOKEN_STEPS, Toggle, fmtTokens as fmtCeiling } from "../components/Controls";
import { ExperimentDashboard } from "../components/ExperimentDashboard";

type Props = { settings: Settings; prs: TrackedPr[]; refreshSettings: () => void };
type Result = NonNullable<RunRecord["result"]>;
type Pillar = Result["assessment"]["wellArchitected"][number];
type Code = Result["codeFindings"][number];

/** Field labels in the order a diff reads best: what runs, how hard, how much, what it's told. */
const CONFIG_LABELS: Record<keyof AiConfig, string> = {
  bedrockModelId: "main model",
  bedrockDrillModelId: "drill model",
  bedrockChatModelId: "assistant model",
  bedrockScoutModelId: "scout model",
  bedrockEffortArch: "architecture effort",
  bedrockEffortDrill: "drill effort",
  bedrockEffortCode: "code-pass effort",
  archMaxOutputTokens: "architecture ceiling",
  codeMaxOutputTokens: "code-pass ceiling",
  routeRoutinePrsToDrillModel: "routine PRs on drill model",
  codeFindingsPass: "code pass",
  customSystemPrompt: "system prompt",
  reviewConventions: "conventions",
};

function captureConfig(s: Settings): AiConfig {
  return {
    bedrockModelId: s.bedrockModelId,
    bedrockDrillModelId: s.bedrockDrillModelId,
    bedrockChatModelId: s.bedrockChatModelId,
    bedrockScoutModelId: s.bedrockScoutModelId,
    bedrockEffortArch: s.bedrockEffortArch,
    bedrockEffortDrill: s.bedrockEffortDrill,
    bedrockEffortCode: s.bedrockEffortCode,
    archMaxOutputTokens: s.archMaxOutputTokens,
    codeMaxOutputTokens: s.codeMaxOutputTokens,
    routeRoutinePrsToDrillModel: s.routeRoutinePrsToDrillModel,
    codeFindingsPass: s.codeFindingsPass,
    customSystemPrompt: s.customSystemPrompt,
    reviewConventions: s.reviewConventions,
  };
}

const fmtTokens = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n % 1000 ? 1 : 0)}k` : String(n));
const fmtMs = (ms: number) =>
  ms < 1000
    ? `${ms}ms`
    : ms < 60_000
      ? `${Math.round(ms / 1000)}s`
      : `${Math.floor(ms / 60_000)}m${String(Math.round((ms % 60_000) / 1000)).padStart(2, "0")}s`;
const fmtUsd = (n: number | null | undefined) =>
  n == null ? "—" : n >= 1 ? `$${n.toFixed(2)}` : `$${n.toFixed(3)}`;

function shortModel(id: string): string {
  const seg = id.split("/").pop() ?? id;
  return seg.length > 26 ? `${seg.slice(0, 11)}…${seg.slice(-10)}` : seg;
}

function describe(key: keyof AiConfig, v: AiConfig[keyof AiConfig]): string {
  if (key === "customSystemPrompt") return String(v).trim() ? `custom, ${String(v).length} chars` : "built-in";
  if (key === "reviewConventions") return String(v).trim() ? `${String(v).length} chars` : "none";
  if (typeof v === "boolean") return v ? "on" : "off";
  if (typeof v === "number") return fmtTokens(v);
  if (key.startsWith("bedrockEffort")) return String(v) || "default";
  return String(v).trim() ? shortModel(String(v)) : "empty";
}

/** What applying `a` would change, read against `b`. */
function configDiff(a: AiConfig, b: AiConfig) {
  return (Object.keys(CONFIG_LABELS) as (keyof AiConfig)[])
    .filter((k) => a[k] !== b[k])
    .map((k) => ({ key: k, label: CONFIG_LABELS[k], from: describe(k, b[k]), to: describe(k, a[k]) }));
}

function words(s: string): Set<string> {
  return new Set(
    s
      .toLowerCase()
      .split(/[^a-z0-9_]+/)
      .filter((w) => w.length >= 4),
  );
}

/** Two findings about the same thing, worded differently. */
function similar(a: string, b: string): boolean {
  const A = words(a);
  const B = words(b);
  if (!A.size || !B.size) return false;
  let inter = 0;
  for (const w of A) if (B.has(w)) inter++;
  return inter / (A.size + B.size - inter) >= 0.35;
}

function prLabel(pr: TrackedPr | undefined, id: string): string {
  if (!pr) return `…${id.slice(-6)}`;
  return `${pr.repo.split("/")[1] ?? pr.repo}#${pr.number}`;
}

export function ExperimentsPane({ settings, prs, refreshSettings }: Props) {
  const [experiments, setExperiments] = useState<Experiment[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, string>>({});
  const [detailPr, setDetailPr] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [prQuery, setPrQuery] = useState("");
  const [newName, setNewName] = useState("");
  const [force, setForce] = useState(false);
  const [editor, setEditor] = useState<{ variantId: string | null; name: string; config: AiConfig } | null>(null);
  const [renaming, setRenaming] = useState<{ kind: "experiment" | "variant"; id: string; value: string } | null>(null);

  useEffect(() => {
    void ipc
      .listExperiments()
      .then((list) => {
        setExperiments(list);
        setSelectedId((id) => id ?? list[0]?.id ?? null);
      })
      .catch((e) => setError(String(e)));
    const un1 = onExperimentChanged((e) =>
      setExperiments((list) => (list.some((x) => x.id === e.id) ? list.map((x) => (x.id === e.id ? e : x)) : [e, ...list])),
    );
    const un2 = onExperimentProgress((p) => setProgress((m) => ({ ...m, [p.prId]: p.message })));
    return () => {
      void un1.then((f) => f());
      void un2.then((f) => f());
    };
  }, []);

  const exp = experiments.find((e) => e.id === selectedId) ?? null;
  const live = useMemo(() => captureConfig(settings), [settings]);
  const prById = useMemo(() => new Map(prs.map((p) => [p.id, p])), [prs]);

  const call = async (fn: () => Promise<unknown>) => {
    try {
      await fn();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const runFor = (variantId: string, prId: string) =>
    exp?.runs.find((r) => r.variantId === variantId && r.prId === prId);
  const busy = exp?.runs.some((r) => r.status === "running" || r.status === "queued") ?? false;

  // The queue: every variant with something queued or running, across the
  // whole bench. Done counts what those variants already have, so a single
  // variant's Run shows that column filling in and Run all shows the lot.
  const queue = useMemo(() => {
    if (!exp || !busy) return null;
    const involved = new Set(
      exp.runs.filter((r) => r.status === "running" || r.status === "queued").map((r) => r.variantId),
    );
    const total = involved.size * exp.prIds.length;
    const pending = exp.runs.filter(
      (r) => involved.has(r.variantId) && (r.status === "running" || r.status === "queued"),
    ).length;
    const current = exp.runs.find((r) => r.status === "running");
    return {
      total,
      done: Math.max(0, total - pending),
      current,
      variantName: current ? exp.variants.find((v) => v.id === current.variantId)?.name ?? "" : "",
    };
  }, [exp, busy]);

  // A one-second tick while something runs, for the elapsed readout.
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!busy) return;
    const t = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [busy]);

  // -- bench picker ---------------------------------------------------------
  const q = prQuery.trim().toLowerCase();
  const exact = q.match(/^(.*?)#(\d+)$/);
  const candidates = q
    ? prs
        .filter((pr) => !exp?.prIds.includes(pr.id))
        .filter((pr) =>
          exact
            ? String(pr.number) === exact[2] && (!exact[1] || pr.repo.toLowerCase().includes(exact[1]))
            : pr.repo.toLowerCase().includes(q) ||
              pr.title.toLowerCase().includes(q) ||
              String(pr.number).includes(q),
        )
        .slice(0, 8)
    : [];

  const create = () =>
    void call(async () => {
      const e = await ipc.createExperiment(newName);
      setNewName("");
      setSelectedId(e.id);
    });

  const remove = () => {
    if (!exp) return;
    void call(async () => {
      await ipc.deleteExperiment(exp.id);
      setExperiments((list) => list.filter((x) => x.id !== exp.id));
      setSelectedId(null);
    });
  };

  const commitRename = () => {
    if (!renaming || !exp) return;
    const { kind, id, value } = renaming;
    setRenaming(null);
    void call(() => (kind === "experiment" ? ipc.renameExperiment(exp.id, value) : ipc.renameVariant(exp.id, id, value)));
  };

  return (
    <section className="pane-section pane-wide experiments">
      <h2>Experiments</h2>
      <p className="pane-intro">
        Change the AI settings, run the same PRs, and see what moved — time, tokens, cost, and the
        findings themselves. A variant is a snapshot of the AI pane; runs here never replace the
        analyses on the review screen, and each is a fresh read so variants compare like for like.
      </p>

      <div className="exp-toolbar">
        <select value={selectedId ?? ""} onChange={(e) => setSelectedId(e.target.value || null)}>
          {experiments.length === 0 && <option value="">No experiments yet</option>}
          {experiments.map((e) => (
            <option key={e.id} value={e.id}>
              {e.name}
            </option>
          ))}
        </select>
        {exp && renaming?.kind === "experiment" ? (
          <input
            autoFocus
            value={renaming.value}
            onChange={(e) => setRenaming({ ...renaming, value: e.target.value })}
            onBlur={commitRename}
            onKeyDown={(e) => e.key === "Enter" && commitRename()}
          />
        ) : (
          exp && (
            <button
              className="icon-btn"
              title="Rename"
              onClick={() => setRenaming({ kind: "experiment", id: exp.id, value: exp.name })}
            >
              ✎
            </button>
          )
        )}
        {exp && (
          <button className="icon-btn" title="Delete experiment" onClick={remove}>
            ✕
          </button>
        )}
        <span className="spacer" />
        <input
          placeholder="New experiment name"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && create()}
        />
        <button className="action-btn" onClick={create}>
          New
        </button>
      </div>
      {error && <div className="settings-error">{error}</div>}

      {exp && (
        <>
          <h3 className="pane-subhead">Bench</h3>
          <p className="exp-hint">
            The PRs every variant runs. A spread of sizes — one file, a handful, twenty — shows
            more than four of the same shape.
          </p>
          <div className="exp-picker">
              <input
                placeholder="Add a PR — title, repo, #123, or repo#123"
                value={prQuery}
                onChange={(e) => setPrQuery(e.target.value)}
              />
              {candidates.length > 0 && (
                <div className="exp-picker-list">
                  {candidates.map((pr) => (
                    <button
                      key={pr.id}
                      onClick={() => {
                        setPrQuery("");
                        void call(() => ipc.setExperimentPrs(exp.id, [...exp.prIds, pr.id]));
                      }}
                    >
                      <span className="mono">{prLabel(pr, pr.id)}</span> {pr.title}
                      <span className="exp-chip-meta"> · {pr.changedFiles} files</span>
                    </button>
                  ))}
                </div>
              )}
          </div>
          <div className="exp-chips">
            {exp.prIds.map((id) => {
              const pr = prById.get(id);
              return (
                <span key={id} className="exp-chip">
                  <span className="mono">{prLabel(pr, id)}</span>
                  {pr && (
                    <span className="exp-chip-meta">
                      {pr.changedFiles} files · +{pr.additions} −{pr.deletions}
                    </span>
                  )}
                  <button
                    className="icon-btn"
                    title="Remove from bench"
                    onClick={() =>
                      void call(() => ipc.setExperimentPrs(exp.id, exp.prIds.filter((x) => x !== id)))
                    }
                  >
                    ✕
                  </button>
                </span>
              );
            })}
          </div>

          {exp.notice && <div className="exp-notice">{exp.notice}</div>}
          <h3 className="pane-subhead">Variants</h3>
          <div className="row exp-variants-bar">
            <button
              className="action-btn"
              onClick={() => setEditor({ variantId: null, name: "", config: live })}
            >
              New variant…
            </button>
            <button
              className="action-btn"
              disabled={busy || exp.variants.length === 0 || exp.prIds.length === 0}
              title={force ? "Re-run every PR under every variant" : "Run every PR that has no result yet, under every variant"}
              onClick={() => void call(() => ipc.runExperiment(exp.id, force))}
            >
              {busy ? "Running…" : force ? "Re-run all" : "Run all"}
            </button>
            <Toggle checked={force} onChange={setForce} label="Force re-run" />
          </div>
          {queue && (
            <div className="exp-progress">
              <div className="exp-progress-track">
                <div
                  className="exp-progress-bar"
                  style={{ width: `${queue.total ? ((queue.done + 0.5) / queue.total) * 100 : 0}%` }}
                />
              </div>
              <div className="exp-progress-text">
                <span className="sync-dot live" />
                <span>
                  {queue.done + 1} of {queue.total}
                  {queue.current && (
                    <>
                      {" · "}
                      <span className="mono">{prLabel(prById.get(queue.current.prId), queue.current.prId)}</span>
                      {" under "}
                      {queue.variantName}
                      {" · "}
                      {fmtMs(Math.max(0, Date.now() - Date.parse(queue.current.startedAt)))}
                    </>
                  )}
                </span>
                {queue.current && progress[queue.current.prId] && (
                  <span className="exp-chip-meta exp-progress-step">{progress[queue.current.prId]}</span>
                )}
              </div>
            </div>
          )}
          {exp.variants.length === 0 && (
            <p className="exp-hint">
              A variant is one AI configuration. Start from the live settings — that is a capture
              of what runs today — or from another variant, change what you want to test, and
              run the bench. Each variant becomes a column.
            </p>
          )}

          {exp.variants.length > 0 && exp.runs.some((r) => r.status === "ok") && (
            <>
              <h3 className="pane-subhead exp-results-head">Results</h3>
              <ExperimentDashboard exp={exp} label={(id) => prLabel(prById.get(id), id)} />
            </>
          )}

          {exp.variants.length > 0 && (
            <h3 className="pane-subhead exp-runs-head">Runs</h3>
          )}
          {exp.variants.length > 0 && (
            <div className="exp-grid-wrap">
              <table className="repo-table exp-grid">
                <thead>
                  <tr>
                    <th>PR</th>
                    {exp.variants.map((v) => {
                      const diff = configDiff(v.config, live);
                      const runs = exp.runs.filter((r) => r.variantId === v.id);
                      const allDone =
                        exp.prIds.length > 0 &&
                        exp.prIds.every((p) => runs.some((r) => r.prId === p && r.status === "ok"));
                      return (
                        <th key={v.id} className="exp-variant">
                          {(
                            <div className="exp-variant-name">
                              {v.name}
                              <button
                                className="icon-btn"
                                title="Edit this variant's configuration"
                                onClick={() => setEditor({ variantId: v.id, name: v.name, config: v.config })}
                              >
                                ✎
                              </button>
                            </div>
                          )}
                          <div className="exp-variant-diff mono">
                            {diff.length === 0 ? (
                              <span className="exp-live">same as the live settings</span>
                            ) : (
                              diff.map((d) => (
                                <div key={d.key} title={`${d.label}: live is ${d.from}`}>
                                  {d.label}: {d.to}
                                </div>
                              ))
                            )}
                          </div>
                          <div className="row exp-variant-actions">
                            <button
                              className="action-btn"
                              disabled={busy || exp.prIds.length === 0 || (allDone && !force)}
                              title={allDone && !force ? "Every PR has a result — turn on force re-run" : ""}
                              onClick={() => void call(() => ipc.runVariant(exp.id, v.id, force))}
                            >
                              {force ? "Re-run" : "Run"}
                            </button>
                            <button
                              className="action-btn"
                              disabled={diff.length === 0}
                              title="Make this the live configuration"
                              onClick={() =>
                                void call(async () => {
                                  await ipc.applyVariant(exp.id, v.id);
                                  refreshSettings();
                                })
                              }
                            >
                              Apply
                            </button>
                            <button
                              className="icon-btn"
                              title="Remove variant and its runs"
                              onClick={() => void call(() => ipc.removeVariant(exp.id, v.id))}
                            >
                              ✕
                            </button>
                          </div>
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody>
                  {exp.prIds.map((prId) => {
                    const pr = prById.get(prId);
                    return (
                      <tr
                        key={prId}
                        className={`exp-row${detailPr === prId ? " selected" : ""}`}
                        onClick={() => setDetailPr((d) => (d === prId ? null : prId))}
                      >
                        <td>
                          <div className="mono">{prLabel(pr, prId)}</div>
                          {pr && (
                            <div className="exp-chip-meta">
                              {pr.changedFiles} files · +{pr.additions} −{pr.deletions}
                            </div>
                          )}
                        </td>
                        {exp.variants.map((v) => (
                          <td key={v.id}>
                            <Cell run={runFor(v.id, prId)} progress={progress[prId]} />
                          </td>
                        ))}
                      </tr>
                    );
                  })}
                  {exp.prIds.length > 1 && (
                    <tr className="exp-totals">
                      <td>Bench total</td>
                      {exp.variants.map((v) => {
                        const ok = exp.runs.filter((r) => r.variantId === v.id && r.status === "ok");
                        if (ok.length === 0) return <td key={v.id}>—</td>;
                        const cost = ok.every((r) => r.costUsd != null)
                          ? ok.reduce((s, r) => s + (r.costUsd ?? 0), 0)
                          : null;
                        return (
                          <td key={v.id}>
                            <div>
                              {fmtMs(ok.reduce((s, r) => s + r.elapsedMs, 0))} ·{" "}
                              {fmtTokens(ok.reduce((s, r) => s + r.outputTokens, 0))} out
                            </div>
                            <div className="exp-chip-meta">
                              {fmtUsd(cost)} ·{" "}
                              {ok.reduce((s, r) => s + (r.result?.assessment.wellArchitected.length ?? 0), 0)} pillar ·{" "}
                              {ok.reduce((s, r) => s + (r.result?.codeFindings.length ?? 0), 0)} code
                              {ok.length < exp.prIds.length && ` · ${ok.length}/${exp.prIds.length} PRs`}
                            </div>
                          </td>
                        );
                      })}
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}

          {detailPr && exp.variants.length > 0 && (
            <Detail exp={exp} prId={detailPr} label={prLabel(prById.get(detailPr), detailPr)} />
          )}
          {editor && (
            <VariantEditor
              initialName={editor.name}
              initialConfig={editor.config}
              live={live}
              others={exp.variants.filter((v) => v.id !== editor.variantId)}
              editing={editor.variantId !== null}
              onCancel={() => setEditor(null)}
              onSave={(name, config) => {
                const { variantId } = editor;
                setEditor(null);
                void call(() =>
                  variantId ? ipc.updateVariant(exp.id, variantId, name, config) : ipc.addVariant(exp.id, name, config),
                );
              }}
            />
          )}
        </>
      )}
    </section>
  );
}

/** One collapsible group of the variant form. The header carries the
 *  group's current values and a mark when they differ from the live
 *  settings, so a closed section still tells you what is in it. */
function Section({
  id,
  title,
  summary,
  changed,
  open,
  onToggle,
  children,
}: {
  id: string;
  title: string;
  summary: string;
  changed: boolean;
  open: string | null;
  onToggle: (id: string | null) => void;
  children: React.ReactNode;
}) {
  const isOpen = open === id;
  return (
    <div className={`exp-acc${isOpen ? " open" : ""}${changed ? " changed" : ""}`}>
      <button type="button" className="exp-acc-head" onClick={() => onToggle(isOpen ? null : id)}>
        <span className="exp-acc-chevron">{isOpen ? "▾" : "▸"}</span>
        <span className="exp-acc-title">{title}</span>
        <span className="exp-acc-summary mono">{summary}</span>
        {changed && <span className="exp-only">changed</span>}
      </button>
      {isOpen && <div className="exp-acc-body">{children}</div>}
    </div>
  );
}

/** Compose a variant: start from the live settings (a capture of what runs
 *  today) or from another variant, then change what you want to test. */
function VariantEditor({
  initialName,
  initialConfig,
  live,
  others,
  editing,
  onSave,
  onCancel,
}: {
  initialName: string;
  initialConfig: AiConfig;
  live: AiConfig;
  others: { id: string; name: string; config: AiConfig }[];
  editing: boolean;
  onSave: (name: string, config: AiConfig) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initialName);
  const [cfg, setCfg] = useState<AiConfig>(initialConfig);
  const [builtIn, setBuiltIn] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const set = <K extends keyof AiConfig>(key: K, value: AiConfig[K]) => setCfg((c) => ({ ...c, [key]: value }));
  const diff = configDiff(cfg, live);
  const changedIn = (keys: (keyof AiConfig)[]) => diff.some((d) => keys.includes(d.key));

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <>
      <div className="drawer-backdrop" onClick={onCancel} />
      <div className="exp-modal" role="dialog" aria-modal="true">
        <header className="exp-modal-head">
          <span className="exp-modal-title">{editing ? "Edit variant" : "New variant"}</span>
          <span className="spacer" />
          <label className="exp-startfrom">
            start from
            <select
              value=""
              onChange={(e) => {
                const v = e.target.value;
                if (v === "live") setCfg(live);
                else {
                  const o = others.find((x) => x.id === v);
                  if (o) setCfg(o.config);
                }
              }}
            >
              <option value="">…</option>
              <option value="live">Live settings (capture)</option>
              {others.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
          </label>
        </header>
        <div className="exp-modal-body">
          <label className="exp-field">
            <span>Name</span>
            <input autoFocus placeholder="e.g. arch medium, code pass off" value={name} onChange={(e) => setName(e.target.value)} />
          </label>


          <Section
            id="models"
            title="Models"
            summary={[cfg.bedrockModelId, cfg.bedrockDrillModelId, cfg.bedrockScoutModelId]
              .map((m) => (m.trim() ? shortModel(m) : "—"))
              .join(" · ")}
            changed={changedIn(["bedrockModelId", "bedrockDrillModelId", "bedrockChatModelId", "bedrockScoutModelId"])}
            open={open}
            onToggle={setOpen}
          >
          <label className="exp-field">
            <span>Main model</span>
            <input value={cfg.bedrockModelId} onChange={(e) => set("bedrockModelId", e.target.value)} />
          </label>
          <label className="exp-field">
            <span>Drill model</span>
            <input value={cfg.bedrockDrillModelId} onChange={(e) => set("bedrockDrillModelId", e.target.value)} />
          </label>
          <label className="exp-field">
            <span>Assistant model</span>
            <input value={cfg.bedrockChatModelId} onChange={(e) => set("bedrockChatModelId", e.target.value)} />
          </label>
          <label className="exp-field">
            <span>Scout model</span>
            <input value={cfg.bedrockScoutModelId} onChange={(e) => set("bedrockScoutModelId", e.target.value)} />
          </label>

          </Section>
          <Section
            id="effort"
            title="Effort"
            summary={`arch ${cfg.bedrockEffortArch || "default"} · drill ${cfg.bedrockEffortDrill || "default"} · code ${cfg.bedrockEffortCode || "default"}`}
            changed={changedIn(["bedrockEffortArch", "bedrockEffortDrill", "bedrockEffortCode"])}
            open={open}
            onToggle={setOpen}
          >
          <div className="exp-field">
            <span>Architecture — {cfg.bedrockEffortArch || "default"}</span>
            <EffortSlider value={cfg.bedrockEffortArch} onChange={(v) => set("bedrockEffortArch", v)} />
          </div>
          <div className="exp-field">
            <span>Drill — {cfg.bedrockEffortDrill || "default"}</span>
            <EffortSlider value={cfg.bedrockEffortDrill} onChange={(v) => set("bedrockEffortDrill", v)} />
          </div>
          <div className="exp-field">
            <span>Code pass — {cfg.bedrockEffortCode || "default"}</span>
            <EffortSlider value={cfg.bedrockEffortCode} onChange={(v) => set("bedrockEffortCode", v)} />
          </div>

          </Section>
          <Section
            id="ceilings"
            title="Output ceilings"
            summary={`arch ${fmtCeiling(cfg.archMaxOutputTokens || 16384)} · code ${fmtCeiling(cfg.codeMaxOutputTokens || 16384)}`}
            changed={changedIn(["archMaxOutputTokens", "codeMaxOutputTokens"])}
            open={open}
            onToggle={setOpen}
          >
          <div className="exp-field">
            <span>Architecture — {fmtCeiling(cfg.archMaxOutputTokens || 16384)} tokens</span>
            <StepSlider
              steps={TOKEN_STEPS}
              value={cfg.archMaxOutputTokens || 16384}
              onChange={(v) => set("archMaxOutputTokens", v)}
              fmt={fmtCeiling}
            />
          </div>
          <div className="exp-field">
            <span>Code pass — {fmtCeiling(cfg.codeMaxOutputTokens || 16384)} tokens</span>
            <StepSlider
              steps={TOKEN_STEPS}
              value={cfg.codeMaxOutputTokens || 16384}
              onChange={(v) => set("codeMaxOutputTokens", v)}
              fmt={fmtCeiling}
            />
          </div>

          </Section>
          <Section
            id="behavior"
            title="Behavior"
            summary={`code pass ${cfg.codeFindingsPass ? "on" : "off"} · routine PRs on drill ${cfg.routeRoutinePrsToDrillModel ? "on" : "off"}`}
            changed={changedIn(["codeFindingsPass", "routeRoutinePrsToDrillModel"])}
            open={open}
            onToggle={setOpen}
          >
          <Toggle
            checked={cfg.codeFindingsPass}
            onChange={(v) => set("codeFindingsPass", v)}
            label="Run the code-level pass beside each analysis"
          />
          <Toggle
            checked={cfg.routeRoutinePrsToDrillModel}
            onChange={(v) => set("routeRoutinePrsToDrillModel", v)}
            label="Routine PRs on the drill model"
          />

          </Section>
          <Section
            id="prompts"
            title="Prompts"
            summary={`${cfg.customSystemPrompt.trim() ? `custom prompt, ${cfg.customSystemPrompt.length} chars` : "built-in prompt"} · conventions ${cfg.reviewConventions.trim() ? `${cfg.reviewConventions.length} chars` : "none"}`}
            changed={changedIn(["customSystemPrompt", "reviewConventions"])}
            open={open}
            onToggle={setOpen}
          >
          <label className="exp-field">
            <span>
              System prompt — {cfg.customSystemPrompt.trim() ? `custom, ${cfg.customSystemPrompt.length} chars` : "built-in"}
              {!cfg.customSystemPrompt.trim() && (
                <button
                  className="link-btn"
                  onClick={(e) => {
                    e.preventDefault();
                    if (builtIn) set("customSystemPrompt", builtIn);
                    else
                      void ipc.getDefaultSystemPrompt().then((p) => {
                        setBuiltIn(p);
                        set("customSystemPrompt", p);
                      });
                  }}
                >
                  load the built-in text to edit
                </button>
              )}
            </span>
            <textarea
              className="prompt-editor exp-prompt"
              spellCheck={false}
              placeholder="Empty = the built-in prompt"
              value={cfg.customSystemPrompt}
              onChange={(e) => set("customSystemPrompt", e.target.value)}
            />
          </label>
          <label className="exp-field">
            <span>Review conventions</span>
            <textarea
              className="globs-editor"
              spellCheck={false}
              placeholder="Team conventions the reviewer is told about"
              value={cfg.reviewConventions}
              onChange={(e) => set("reviewConventions", e.target.value)}
            />
          </label>
          </Section>
        </div>
        <footer className="exp-modal-foot">
          <span className="exp-chip-meta mono">
            {diff.length === 0 ? "same as the live settings" : diff.map((d) => `${d.label}: ${d.to}`).join(" · ")}
          </span>
          <span className="spacer" />
          <button className="action-btn" onClick={onCancel}>
            Cancel
          </button>
          <button className="action-btn auth-primary" onClick={() => onSave(name, cfg)}>
            {editing ? "Save" : "Add variant"}
          </button>
        </footer>
      </div>
    </>
  );
}

function Cell({ run, progress }: { run: RunRecord | undefined; progress: string | undefined }) {
  if (!run) return <span className="exp-none">—</span>;
  if (run.status === "queued") return <span className="exp-chip-meta">queued</span>;
  if (run.status === "running")
    return (
      <div className="exp-running">
        <span className="sync-dot live" />
        <span className="exp-chip-meta">{progress ?? "starting"}</span>
      </div>
    );
  if (run.status === "failed") {
    const [headline, ...rest] = (run.error ?? "failed").split("\n");
    return (
      <div className="settings-error exp-failed" title={rest.join("\n")}>
        {headline}
      </div>
    );
  }
  const r = run.result;
  return (
    <div className="exp-cell">
      <div>
        <strong>{fmtMs(run.elapsedMs)}</strong>
        {r && r.usage.elapsedMs > 0 && (
          <span className="exp-chip-meta" title="Both passes run side by side; this is the architecture pass alone">
            {" "}· arch {fmtMs(r.usage.elapsedMs)}, {r.usage.turns} turns
          </span>
        )}
      </div>
      <div className="exp-chip-meta">
        {fmtTokens(run.outputTokens)} out · {fmtUsd(run.costUsd)}
      </div>
      {r && (
        <div className="exp-chip-meta">
          {r.assessment.wellArchitected.length} pillar · {r.codeFindings.length} code ·{" "}
          {r.graph.nodes.length} nodes · <span className={`exp-fit exp-fit-${r.assessment.fit}`}>{r.assessment.fit}</span>
        </div>
      )}
    </div>
  );
}

/** The write-ups for one PR, one column per variant. A finding no other
 *  variant has a counterpart for is marked — that is what a setting cost or bought. */
function Detail({ exp, prId, label }: { exp: Experiment; prId: string; label: string }) {
  const columns = exp.variants.map((v) => ({
    variant: v,
    result: exp.runs.find((r) => r.variantId === v.id && r.prId === prId && r.status === "ok")?.result ?? null,
  }));
  const withResults = columns.filter((c) => c.result);
  const unique = (mine: string, others: string[][]) =>
    withResults.length > 1 && !others.some((list) => list.some((t) => similar(t, mine)));

  return (
    <div className="exp-detail">
      <h3 className="pane-subhead">{label} — side by side</h3>
      <div className="exp-detail-cols">
        {columns.map(({ variant, result }) => {
          const otherPillars = withResults
            .filter((c) => c.variant.id !== variant.id)
            .map((c) => c.result!.assessment.wellArchitected.map((w: Pillar) => w.finding));
          const otherCode = withResults
            .filter((c) => c.variant.id !== variant.id)
            .map((c) => c.result!.codeFindings.map((f: Code) => f.finding));
          return (
            <div key={variant.id} className="exp-detail-col">
              <h4>{variant.name}</h4>
              {!result ? (
                <p className="exp-chip-meta">no result yet</p>
              ) : (
                <>
                  <p className="exp-summary">{result.assessment.summary}</p>
                  <div className="exp-chip-meta mono">
                    fit: {result.assessment.fit} · {result.graph.nodes.length} nodes / {result.graph.edges.length} edges ·{" "}
                    {result.assessment.boundaryImpacts.length} boundary impacts
                  </div>
                  <h5>Pillar findings</h5>
                  <ul className="exp-findings">
                    {result.assessment.wellArchitected.map((w: Pillar, i: number) => (
                      <li key={i} className={unique(w.finding, otherPillars) ? "only-here" : ""}>
                        <span className="mono exp-tag">
                          {w.pillar}/{w.severity}
                        </span>{" "}
                        {w.finding}
                        {unique(w.finding, otherPillars) && <span className="exp-only">only here</span>}
                      </li>
                    ))}
                    {result.assessment.wellArchitected.length === 0 && <li className="exp-chip-meta">none</li>}
                  </ul>
                  <h5>Code findings</h5>
                  <ul className="exp-findings">
                    {result.codeFindings.map((f: Code, i: number) => (
                      <li key={i} className={unique(f.finding, otherCode) ? "only-here" : ""}>
                        <span className="mono exp-tag">
                          {f.kind} · {f.path.split("/").pop()}:{f.line}
                        </span>{" "}
                        {f.finding}
                        {unique(f.finding, otherCode) && <span className="exp-only">only here</span>}
                      </li>
                    ))}
                    {result.codeFindings.length === 0 && <li className="exp-chip-meta">none</li>}
                  </ul>
                  <h5>Review plan</h5>
                  <ul className="exp-findings exp-plan">
                    {result.assessment.reviewPlan
                      .filter((e) => e.significance !== "mechanical")
                      .map((e, i) => (
                        <li key={i}>
                          <span className="mono exp-tag">{e.significance}</span> {e.path.split("/").pop()}
                        </li>
                      ))}
                  </ul>
                </>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
