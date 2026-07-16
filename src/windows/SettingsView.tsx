import {
  createContext,
  isValidElement,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";
import { ipc } from "../lib/ipc";
import {
  activeThemeId,
  allThemes,
  cloneTheme,
  COLOR_KEYS,
  COLOR_LABELS,
  customThemes,
  deleteCustomTheme,
  saveCustomTheme,
  setActiveTheme,
  type Theme,
} from "../lib/theme";
import { usePrStore } from "../state/prStore";
import { DeveloperPane } from "./DeveloperPane";

export type SettingsPane = "general" | "appearance" | "github" | "repos" | "aws" | "developer";
type Pane = SettingsPane;

/** Active settings-search query; Fields hide themselves unless they match. */
const SearchCtx = createContext("");
/** Human pane name, shown as a chip on matched fields during search. */
const PaneNameCtx = createContext("");

function nodeText(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join(" ");
  if (isValidElement(node)) {
    return nodeText((node.props as { children?: ReactNode }).children);
  }
  return "";
}

const PANES: { key: Pane; label: string; glyph: string; dev?: boolean }[] = [
  { key: "general", label: "General", glyph: "◐" },
  { key: "appearance", label: "Appearance", glyph: "◧" },
  { key: "github", label: "GitHub", glyph: "⎇" },
  { key: "repos", label: "Repositories", glyph: "▤" },
  { key: "aws", label: "AWS", glyph: "▲" },
  { key: "developer", label: "Developer", glyph: "⌬", dev: true },
];

const REPO_RE = /^[\w.-]+\/[\w.-]+$/;

/** Switch-style on/off control — settings never use bare checkboxes. */
function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
}) {
  return (
    <label className="toggle-row">
      <input
        type="checkbox"
        className="toggle-input"
        role="switch"
        aria-checked={checked}
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="toggle-track">
        <span className="toggle-knob" />
      </span>
      {label}
    </label>
  );
}

/** Poll-interval ladder: fine steps at the fast end, coarser as it grows.
 *  5s→1m, 30s→5m, 1m→15m, 5m→1h, 30m→6h, 1h→12h. */
const POLL_STEPS = (() => {
  const steps: number[] = [];
  const add = (from: number, to: number, step: number) => {
    for (let v = from; v <= to; v += step) steps.push(v);
  };
  add(5, 60, 5);
  add(90, 300, 30);
  add(360, 900, 60);
  add(1200, 3600, 300);
  add(5400, 21600, 1800);
  add(25200, 43200, 3600);
  return steps;
})();

function fmtInterval(sec: number): string {
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) {
    const m = Math.floor(sec / 60);
    const r = sec % 60;
    return r ? `${m}m ${r}s` : `${m}m`;
  }
  const h = Math.floor(sec / 3600);
  const r = Math.round((sec % 3600) / 60);
  return r ? `${h}h ${r}m` : `${h}h`;
}

function nearestStepIdx(sec: number): number {
  let best = 0;
  POLL_STEPS.forEach((v, i) => {
    if (Math.abs(v - sec) < Math.abs(POLL_STEPS[best] - sec)) best = i;
  });
  return best;
}

/** PR-window ladder: 1d, 2d, 3d, 5d, 1w, 2w, 3w, 1m, 2m, 3m, 6m, 1y. */
const AGE_STEPS = [1, 2, 3, 5, 7, 14, 21, 30, 61, 91, 183, 365];

function fmtAge(days: number): string {
  if (days === 1) return "1 day";
  if (days < 7) return `${days} days`;
  if (days < 30) {
    const w = Math.round(days / 7);
    return w === 1 ? "1 week" : `${w} weeks`;
  }
  if (days < 365) {
    const m = Math.round(days / 30.4);
    return m === 1 ? "1 month" : `${m} months`;
  }
  return "1 year";
}

function nearestAgeIdx(days: number): number {
  let best = 0;
  AGE_STEPS.forEach((v, i) => {
    if (Math.abs(v - days) < Math.abs(AGE_STEPS[best] - days)) best = i;
  });
  return best;
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  const query = useContext(SearchCtx).trim().toLowerCase();
  const paneName = useContext(PaneNameCtx);
  if (query) {
    const haystack =
      `${label} ${nodeText(hint)} ${nodeText(children)} ${paneName}`.toLowerCase();
    if (!haystack.includes(query)) return null;
  }
  return (
    <div className="field">
      {query && paneName && <span className="eyebrow field-pane">{paneName}</span>}
      <label>{label}</label>
      {children}
      {hint && <div className="field-hint">{hint}</div>}
    </div>
  );
}

export function SettingsView({
  onClose,
  initialPane,
}: {
  onClose: () => void;
  initialPane?: SettingsPane;
}) {
  const [pane, setPane] = useState<Pane>(initialPane ?? "general");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const prs = usePrStore((s) => s.prs);

  useEffect(() => {
    void ipc.getSettings().then(setSettings);
  }, []);

  if (!settings) return null;

  const save = async (patch: Partial<Settings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    try {
      await ipc.setSettings(next);
      setError(null);
      setSaved(true);
      setTimeout(() => setSaved(false), 1800);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="settings-shell">
      <nav className="settings-nav">
        <span className="eyebrow settings-title">Settings</span>
        <input
          className="settings-search"
          placeholder="Search settings…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {PANES.filter((p) => !p.dev || settings.developerMode).map((p) => (
          <button
            key={p.key}
            className={`settings-nav-item${pane === p.key && !query.trim() ? " active" : ""}`}
            onClick={() => {
              setQuery("");
              setPane(p.key);
            }}
          >
            <span className="glyph">{p.glyph}</span>
            {p.label}
          </button>
        ))}
        <div className="settings-nav-footer">
          {saved && <span className="save-note">saved</span>}
          {error && <span className="settings-error">{error}</span>}
          <button className="action-btn" onClick={onClose}>
            Done
          </button>
        </div>
      </nav>

      {query.trim() ? (
        // Search: every Field across the panes, filtered down to matches
        // (section headers hidden via .settings-search-results).
        <div className="settings-pane settings-search-results">
          <SearchCtx.Provider value={query}>
            <PaneNameCtx.Provider value="General">
              <GeneralPane settings={settings} save={save} />
            </PaneNameCtx.Provider>
            <PaneNameCtx.Provider value="Appearance">
              <AppearancePane />
            </PaneNameCtx.Provider>
            <PaneNameCtx.Provider value="GitHub">
              <GitHubPane settings={settings} save={save} />
            </PaneNameCtx.Provider>
            <PaneNameCtx.Provider value="AWS">
              <AwsPane settings={settings} save={save} />
            </PaneNameCtx.Provider>
          </SearchCtx.Provider>
        </div>
      ) : (
        <div className="settings-pane">
          {pane === "general" && <GeneralPane settings={settings} save={save} />}
          {pane === "appearance" && <AppearancePane />}
          {pane === "github" && <GitHubPane settings={settings} save={save} />}
          {pane === "repos" && (
            <ReposPane settings={settings} save={save} activeRepos={prs.map((p) => p.repo)} />
          )}
          {pane === "aws" && <AwsPane settings={settings} save={save} />}
          {pane === "developer" && settings.developerMode && (
            <DeveloperPane settings={settings} save={save} />
          )}
        </div>
      )}
    </div>
  );
}

type PaneProps = { settings: Settings; save: (p: Partial<Settings>) => Promise<void> };

// ---------------------------------------------------------------- general

function GeneralPane({ settings, save }: PaneProps) {
  const [legendReset, setLegendReset] = useState(false);
  return (
    <section className="pane-section">
      <h2>General</h2>
      <p className="pane-intro">How CORA behaves day to day.</p>

      <Field
        label="Callout window"
        hint="The small always-on-top PR panel. You can always toggle it from the tray or with ▣."
      >
        <Toggle
          checked={settings.showCalloutOnStartup}
          onChange={(v) => void save({ showCalloutOnStartup: v })}
          label="Open the callout at launch"
        />
      </Field>

      <Field label="Symbol legend" hint="Show the lamps/markers explainer card again in the PR list.">
        <button
          className="action-btn"
          disabled={legendReset}
          onClick={() => {
            localStorage.removeItem("cora.legendDismissed");
            setLegendReset(true);
          }}
        >
          {legendReset ? "Will show on next visit" : "Show legend again"}
        </button>
      </Field>

      <Field
        label="Pre-warm analysis"
        hint="Start the architecture analysis in the background as soon as a PR asks for your review, so results are ready when you open it. Capped per day to keep Bedrock costs bounded."
      >
        <Toggle
          checked={settings.autoAnalyzeReviewRequests}
          onChange={(v) => void save({ autoAnalyzeReviewRequests: v })}
          label="Auto-analyze new review requests"
        />
        {settings.autoAnalyzeReviewRequests && (
          <label className="check-row">
            <input
              type="number"
              min={1}
              className="input-narrow"
              value={settings.autoAnalyzeDailyCap}
              onChange={(e) =>
                void save({
                  autoAnalyzeDailyCap: Math.max(
                    1,
                    Number(e.target.value) || settings.autoAnalyzeDailyCap,
                  ),
                })
              }
            />
            analyses per day, max
          </label>
        )}
      </Field>

      <Field
        label="Review noise filters"
        hint="Glob patterns (one per line) for files auto-skipped in diff review — lockfiles, generated code, snapshots. * matches within a folder, ** across folders."
      >
        <textarea
          className="globs-editor"
          spellCheck={false}
          defaultValue={settings.reviewIgnoreGlobs.join("\n")}
          onBlur={(e) =>
            void save({
              reviewIgnoreGlobs: e.target.value
                .split("\n")
                .map((l) => l.trim())
                .filter(Boolean),
            })
          }
        />
      </Field>

      <Field
        label="Team review conventions"
        hint={
          <>
            Knowledge no diff reveals — fed to the analysis, the code pass, and the assistant.
            e.g. <span className="mono">UI primitives come from @team-and-tech/mona-lisa-design-system —
            flag hand-rolled equivalents.</span>
          </>
        }
      >
        <textarea
          className="globs-editor"
          spellCheck={false}
          placeholder="Design-system packages, shared libraries, review standards…"
          defaultValue={settings.reviewConventions}
          onBlur={(e) => void save({ reviewConventions: e.target.value })}
        />
      </Field>

      <Field
        label="Code findings pass"
        hint="After the architecture analysis, a second pass over the critical/important files hunts consequence-bearing defects and hand-rolled duplicates of existing code. Runs on the drill-down model."
      >
        <Toggle
          checked={settings.codeFindingsPass}
          onChange={(v) => void save({ codeFindingsPass: v })}
          label="Run the code-level pass after each analysis"
        />
      </Field>

      <Field
        label="Notifications"
        hint="Note: in dev builds macOS attributes notifications to the terminal that launched CORA; packaged builds notify as CORA."
      >
        <button
          className="action-btn"
          onClick={() => void invoke("open_notification_settings")}
        >
          Open macOS notification settings
        </button>
      </Field>

      <Field
        label="Developer mode"
        hint="Adds a Developer pane: live internal logs, the Bedrock system prompt editor, and app internals."
      >
        <Toggle
          checked={settings.developerMode}
          onChange={(v) => void save({ developerMode: v })}
          label="Enable developer mode"
        />
      </Field>
    </section>
  );
}

// ---------------------------------------------------------------- github

function GitHubPane({ settings, save }: PaneProps) {
  const [patPresent, setPatPresent] = useState(false);
  const [patDraft, setPatDraft] = useState("");
  const [patError, setPatError] = useState<string | null>(null);

  useEffect(() => {
    void ipc.githubPatPresent().then(setPatPresent);
  }, []);

  const savePat = async () => {
    try {
      await ipc.setGithubPat(patDraft);
      setPatDraft("");
      setPatPresent(true);
      setPatError(null);
    } catch (e) {
      setPatError(String(e));
    }
  };

  return (
    <section className="pane-section">
      <h2>GitHub</h2>
      <p className="pane-intro">
        How CORA talks to GitHub. The token lives in the macOS Keychain and never leaves the
        Rust core.
      </p>

      <Field
        label="Personal access token"
        hint={patError ?? "Needs repo read scope (repo or fine-grained contents/pull-requests read)."}
      >
        <div className="row">
          <input
            type="password"
            placeholder={patPresent ? "••••••••  (stored in Keychain)" : "ghp_… or github_pat_…"}
            value={patDraft}
            onChange={(e) => setPatDraft(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && patDraft && void savePat()}
          />
          <button className="action-btn" disabled={!patDraft} onClick={() => void savePat()}>
            Save
          </button>
          {patPresent && <span className="pat-ok">✓ connected</span>}
        </div>
      </Field>

      {patPresent && (
        <Field label="Disconnect" hint="Removes the token from the Keychain. Polling stops until a new token is saved.">
          <button
            className="action-btn danger"
            onClick={() =>
              void ipc.clearGithubPat().then(() => setPatPresent(false))
            }
          >
            Remove token
          </button>
        </Field>
      )}

      <Field
        label="GraphQL endpoint"
        hint={
          <>
            For GitHub Enterprise use <span className="mono">https://&lt;host&gt;/api/graphql</span>.
            The REST base for analysis tools is derived from this.
          </>
        }
      >
        <input
          value={settings.githubGraphqlUrl}
          onChange={(e) => void save({ githubGraphqlUrl: e.target.value })}
        />
      </Field>

      <Field
        label={`Poll interval — every ${fmtInterval(settings.pollIntervalSecs)}`}
        hint="How often CORA checks GitHub for changes. Fine steps at the fast end (5s), coarser toward the 12-hour maximum."
      >
        <input
          type="range"
          className="interval-slider"
          min={0}
          max={POLL_STEPS.length - 1}
          value={nearestStepIdx(settings.pollIntervalSecs)}
          onChange={(e) => void save({ pollIntervalSecs: POLL_STEPS[Number(e.target.value)] })}
        />
        <div className="interval-scale mono">
          <span>5s</span>
          <span>1m</span>
          <span>15m</span>
          <span>1h</span>
          <span>12h</span>
        </div>
      </Field>

      <Field
        label={`PR window — updated in the last ${fmtAge(settings.prMaxAgeDays || 365)}`}
        hint="Hide pull requests that haven't been updated within this window. They come back the moment something happens on them."
      >
        <input
          type="range"
          className="interval-slider"
          min={0}
          max={AGE_STEPS.length - 1}
          value={nearestAgeIdx(settings.prMaxAgeDays || 365)}
          onChange={(e) => void save({ prMaxAgeDays: AGE_STEPS[Number(e.target.value)] })}
        />
        <div className="interval-scale mono">
          <span>1d</span>
          <span>1w</span>
          <span>1m</span>
          <span>3m</span>
          <span>1y</span>
        </div>
      </Field>
    </section>
  );
}

// ---------------------------------------------------------------- appearance

const SANS_SUGGESTIONS = [
  '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
  "Inter, sans-serif",
  '"SF Pro Text", sans-serif',
  '"Helvetica Neue", Helvetica, Arial, sans-serif',
  '"Avenir Next", sans-serif',
  "Roboto, sans-serif",
];

const MONO_SUGGESTIONS = [
  'ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace',
  '"JetBrains Mono", monospace',
  '"Fira Code", monospace',
  '"Cascadia Code", monospace',
  '"IBM Plex Mono", monospace',
  '"Source Code Pro", monospace',
];

function AppearancePane() {
  // Theme state lives in localStorage (shared with the callout window);
  // this counter just re-renders after each mutation.
  const [, setBump] = useState(0);
  const bump = () => setBump((n) => n + 1);

  const active = activeThemeId();
  const themes = allThemes();
  // Only custom themes are editable — the editor targets the active one.
  const editing = customThemes().find((c) => c.id === active) ?? null;

  const update = (patch: Partial<Theme>) => {
    if (!editing) return;
    saveCustomTheme({ ...editing, ...patch });
    bump();
  };

  return (
    <section className="pane-section">
      <h2>Appearance</h2>
      <p className="pane-intro">
        Pick a theme, or clone one to make it yours — colors and fonts apply live as you edit,
        in every window.
      </p>

      <Field label="Theme">
        <div className="theme-list">
          {themes.map((th) => (
            <div key={th.id} className={`theme-row${th.id === active ? " active" : ""}`}>
              <button
                className="theme-pick"
                onClick={() => {
                  setActiveTheme(th.id);
                  bump();
                }}
              >
                <span className="theme-swatches">
                  {(["ink0", "ink1", "text", "ok", "warn", "bad", "chat"] as const).map((k) => (
                    <span key={k} style={{ background: th.colors[k] }} />
                  ))}
                </span>
                <span className="theme-name">{th.name}</span>
                {!th.builtin && <span className="theme-tag mono">custom</span>}
              </button>
              <button className="action-btn" title="Copy into an editable theme" onClick={() => {
                const clone = cloneTheme(th);
                setActiveTheme(clone.id);
                bump();
              }}>
                Clone
              </button>
              {!th.builtin && (
                <button
                  className="icon-btn"
                  title="Delete this theme"
                  onClick={() => {
                    deleteCustomTheme(th.id);
                    bump();
                  }}
                >
                  ✕
                </button>
              )}
            </div>
          ))}
        </div>
      </Field>

      {editing ? (
        <>
          <Field label="Theme name">
            <input
              value={editing.name}
              onChange={(e) => update({ name: e.target.value })}
            />
          </Field>

          <Field label="Colors" hint="Every change applies immediately across the app.">
            <div className="color-grid">
              {COLOR_KEYS.map((k) => (
                <label key={k} className="color-cell">
                  <input
                    type="color"
                    value={editing.colors[k]}
                    onChange={(e) =>
                      update({ colors: { ...editing.colors, [k]: e.target.value } })
                    }
                  />
                  <span className="color-label">{COLOR_LABELS[k]}</span>
                  <span className="mono color-hex">{editing.colors[k]}</span>
                </label>
              ))}
            </div>
          </Field>

          <Field
            label="Application font"
            hint="UI chrome — lists, buttons, prose. Any CSS font stack; suggestions in the dropdown."
          >
            <input
              list="sans-fonts"
              value={editing.fonts.sans}
              onChange={(e) => update({ fonts: { ...editing.fonts, sans: e.target.value } })}
            />
            <datalist id="sans-fonts">
              {SANS_SUGGESTIONS.map((f) => (
                <option key={f} value={f} />
              ))}
            </datalist>
          </Field>

          <Field label="Code font" hint="Diffs, file paths, identifiers, data.">
            <input
              list="mono-fonts"
              value={editing.fonts.mono}
              onChange={(e) => update({ fonts: { ...editing.fonts, mono: e.target.value } })}
            />
            <datalist id="mono-fonts">
              {MONO_SUGGESTIONS.map((f) => (
                <option key={f} value={f} />
              ))}
            </datalist>
          </Field>
        </>
      ) : (
        <p className="pane-intro">
          Built-in themes are read-only — <strong>Clone</strong> one to edit its colors and
          fonts with live preview.
        </p>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- repositories

function ReposPane({
  settings,
  save,
  activeRepos,
}: PaneProps & { activeRepos: string[] }) {
  const [draft, setDraft] = useState("");
  const [draftError, setDraftError] = useState<string | null>(null);

  const activeCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const repo of activeRepos) counts.set(repo, (counts.get(repo) ?? 0) + 1);
    return counts;
  }, [activeRepos]);

  const rows = useMemo(() => {
    const all = new Set<string>([
      ...settings.watchedRepos,
      ...Object.keys(settings.repoPriorities),
      ...activeCounts.keys(),
    ]);
    return [...all].sort().map((repo) => ({
      repo,
      watched: settings.watchedRepos.includes(repo),
      priority: settings.repoPriorities[repo] ?? ("normal" as RepoPriority),
      activePrs: activeCounts.get(repo) ?? 0,
    }));
  }, [settings, activeCounts]);

  const addRepo = () => {
    const repo = draft.trim();
    if (!REPO_RE.test(repo)) {
      setDraftError("Use the owner/name form, e.g. team-and-tech/core-services");
      return;
    }
    if (settings.watchedRepos.includes(repo)) {
      setDraftError("Already watched.");
      return;
    }
    setDraftError(null);
    setDraft("");
    void save({ watchedRepos: [...settings.watchedRepos, repo].sort() });
  };

  const toggleWatched = (repo: string, watched: boolean) => {
    void save({
      watchedRepos: watched
        ? [...settings.watchedRepos, repo].sort()
        : settings.watchedRepos.filter((r) => r !== repo),
    });
  };

  const setPriority = (repo: string, priority: RepoPriority) => {
    const next = { ...settings.repoPriorities };
    if (priority === "normal") delete next[repo];
    else next[repo] = priority;
    void save({ repoPriorities: next });
  };

  const removeRepo = (repo: string) => {
    const priorities = { ...settings.repoPriorities };
    delete priorities[repo];
    void save({
      watchedRepos: settings.watchedRepos.filter((r) => r !== repo),
      repoPriorities: priorities,
    });
  };

  return (
    <section className="pane-section pane-wide">
      <h2>Repositories</h2>
      <p className="pane-intro">
        <strong>Watched</strong> repos have every open PR tracked, not just the ones involving
        you. <strong>Priority</strong> weights a repo anywhere it appears — high floats to the
        top, low sinks, ignored is never tracked at all.
      </p>

      <div className="repo-add">
        <input
          placeholder="owner/name — press Enter to watch"
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value);
            setDraftError(null);
          }}
          onKeyDown={(e) => e.key === "Enter" && draft && addRepo()}
        />
        <button className="action-btn" disabled={!draft} onClick={addRepo}>
          Watch
        </button>
      </div>
      {draftError && <div className="settings-error">{draftError}</div>}

      {rows.length === 0 ? (
        <p className="pane-intro">
          Nothing yet — repos appear here once you watch one or once PRs involving you are
          tracked.
        </p>
      ) : (
        <table className="repo-table">
          <thead>
            <tr>
              <th>Repository</th>
              <th className="col-center">Open PRs</th>
              <th className="col-center">Watch all PRs</th>
              <th>Priority</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.repo} className={row.priority === "ignored" ? "row-ignored" : ""}>
                <td className="mono repo-cell">{row.repo}</td>
                <td className="col-center mono">{row.activePrs > 0 ? row.activePrs : "—"}</td>
                <td className="col-center">
                  <Toggle checked={row.watched} onChange={(v) => toggleWatched(row.repo, v)} />
                </td>
                <td>
                  <select
                    value={row.priority}
                    onChange={(e) => setPriority(row.repo, e.target.value as RepoPriority)}
                  >
                    <option value="high">high</option>
                    <option value="normal">normal</option>
                    <option value="low">low</option>
                    <option value="ignored">ignored</option>
                  </select>
                </td>
                <td className="col-center">
                  {(row.watched || row.priority !== "normal") && (
                    <button
                      className="icon-btn"
                      title="Unwatch and clear priority"
                      onClick={() => removeRepo(row.repo)}
                    >
                      ✕
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- aws

function AwsPane({ settings, save }: PaneProps) {
  const [check, setCheck] = useState<
    { state: "idle" } | { state: "checking" } | { state: "ok" } | { state: "err"; detail: string }
  >({ state: "idle" });
  const [signingIn, setSigningIn] = useState(false);

  const test = async () => {
    setCheck({ state: "checking" });
    try {
      await ipc.checkAws(settings.awsProfile, settings.awsRegion);
      setCheck({ state: "ok" });
    } catch (e) {
      setCheck({ state: "err", detail: String(e) });
    }
  };

  const signIn = async () => {
    setSigningIn(true);
    try {
      await ipc.awsSsoLogin(settings.awsProfile);
      await test();
    } catch (e) {
      setCheck({ state: "err", detail: String(e) });
    } finally {
      setSigningIn(false);
    }
  };

  return (
    <section className="pane-section">
      <h2>AWS</h2>
      <p className="pane-intro">
        Bedrock powers the architecture analysis. Credentials come from your local AWS config —
        CORA never stores them.
      </p>

      <Field label="Connection">
        <div className="row">
          <button className="action-btn" disabled={check.state === "checking"} onClick={() => void test()}>
            {check.state === "checking" ? "Checking…" : "Test connection"}
          </button>
          {check.state === "ok" && <span className="pat-ok">✓ credentials valid</span>}
          {check.state === "err" && (
            <button className="action-btn auth-primary" disabled={signingIn} onClick={() => void signIn()}>
              {signingIn ? "Waiting for browser…" : "Sign in with AWS SSO"}
            </button>
          )}
        </div>
        {check.state === "err" && <div className="settings-error">{check.detail}</div>}
      </Field>

      <Field
        label="Profile"
        hint={
          <>
            From ~/.aws/config. If analysis fails with an auth error, refresh with{" "}
            <span className="mono">aws sso login --profile {settings.awsProfile || "…"}</span>
          </>
        }
      >
        <input
          className="input-narrow"
          value={settings.awsProfile}
          onChange={(e) => void save({ awsProfile: e.target.value })}
        />
      </Field>

      <Field label="Region">
        <input
          className="input-narrow"
          placeholder="us-east-2"
          value={settings.awsRegion}
          onChange={(e) => void save({ awsRegion: e.target.value })}
        />
      </Field>

      <Field
        label="Custom endpoint URL"
        hint={
          settings.awsEndpointUrl && !settings.awsEndpointUrl.startsWith("http") ? (
            <span className="settings-error">
              Must be an https:// URL — inference-profile ARNs go in the model field below.
            </span>
          ) : (
            "Optional — only if your org routes Bedrock through a private endpoint (a URL, not an ARN)."
          )
        }
      >
        <input
          placeholder="https://bedrock-runtime.…"
          value={settings.awsEndpointUrl}
          onChange={(e) => void save({ awsEndpointUrl: e.target.value })}
        />
      </Field>

      <Field
        label="Model id or inference-profile ARN"
        hint="Used for the Context/Container analysis — the full-architecture pass."
      >
        <input
          value={settings.bedrockModelId}
          onChange={(e) => void save({ bedrockModelId: e.target.value })}
        />
      </Field>

      <Field
        label="Drill-down model"
        hint="Faster model for Component/Code drill-downs (code-level, not system-wide). Empty = use the main model."
      >
        <input
          value={settings.bedrockDrillModelId}
          onChange={(e) => void save({ bedrockDrillModelId: e.target.value })}
        />
      </Field>
    </section>
  );
}
