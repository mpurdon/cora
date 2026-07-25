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

export type SettingsPane =
  | "general"
  | "appearance"
  | "github"
  | "orgs"
  | "repos"
  | "users"
  | "aws"
  | "developer";
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
  { key: "orgs", label: "Organizations", glyph: "◫" },
  { key: "repos", label: "Repositories", glyph: "▤" },
  { key: "users", label: "Users", glyph: "◔" },
  { key: "aws", label: "AWS", glyph: "▲" },
  { key: "developer", label: "Developer", glyph: "⌬", dev: true },
];

const REPO_RE = /^[\w.-]+\/[\w.-]+$/;

/** Switch-style on/off control — settings never use bare checkboxes. */
function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="toggle-row">
      <input
        type="checkbox"
        disabled={disabled}
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


/** Filled-track percentage for the custom slider (see .interval-slider). */
function sliderFill(idx: number, maxIdx: number): React.CSSProperties {
  return { "--fill": `${(idx / maxIdx) * 100}%` } as React.CSSProperties;
}

// Output-token ceiling steps, chosen to land on real Claude-on-Bedrock hard
// output caps: 4k/8k (older Claude), 16k, and 24k–64k (newer models). The
// model behind an inference-profile ARN is opaque, so we can't know which cap
// applies — picking a real value keeps the choice meaningful, and the
// over-cap Bedrock error (engine.rs) is the backstop if a step is too high.
const TOKEN_STEPS = [4096, 8192, 16384, 24576, 32768, 49152, 65536];

function fmtTokens(n: number): string {
  const k = n / 1024;
  return Number.isInteger(k) ? `${k}k` : `${k.toFixed(1)}k`;
}

// How many rows the callout shows — a query cap only (get_activity). Older
// rows stay in SQLite up to a high runaway guard, so shrinking this shortens
// the feed without deleting history.
const FEED_STEPS = [50, 100, 150, 200, 300, 500];

function nearestIdx(steps: number[], value: number): number {
  let best = 0;
  steps.forEach((v, i) => {
    if (Math.abs(v - value) < Math.abs(steps[best] - value)) best = i;
  });
  return best;
}

/** A slider over a small set of discrete `steps`, with a tick label under each
 *  value. WKWebView (Tauri's macOS engine) paints the native range thumb at a
 *  position that doesn't track the standard geometry, so external labels can't
 *  be aligned to it. We hide the native thumb and draw our own thumb and labels
 *  from one shared formula — thumb left edge at frac·(track − thumb), label
 *  centre at frac·(track − thumb) + thumb/2 — so they align by construction at
 *  any CSS zoom. The thumb width lives once in CSS as --thumb-w. */
function StepSlider({
  steps,
  value,
  onChange,
  fmt,
}: {
  steps: number[];
  value: number;
  onChange: (v: number) => void;
  fmt?: (v: number) => string;
}) {
  const fracOf = (i: number) => (steps.length > 1 ? i / (steps.length - 1) : 0);
  const idx = nearestIdx(steps, value);
  // --frac positions the custom thumb; --fill (same ratio, as a %) colours the
  // native track gradient. Both derive from one computation here.
  const style = {
    "--frac": String(fracOf(idx)),
    "--fill": `${fracOf(idx) * 100}%`,
  } as React.CSSProperties;
  return (
    <div className="step-slider" style={style}>
      <input
        type="range"
        className="interval-slider step-slider-input"
        min={0}
        max={steps.length - 1}
        value={idx}
        onChange={(e) => onChange(steps[Number(e.target.value)])}
      />
      <span className="step-slider-thumb" aria-hidden="true" />
      <div className="slider-scale mono">
        {steps.map((v, i) => (
          <span
            key={v}
            style={{ left: `calc(${fracOf(i)} * (100% - var(--thumb-w)) + var(--thumb-w) / 2)` }}
          >
            {fmt ? fmt(v) : v}
          </span>
        ))}
      </div>
    </div>
  );
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
  const [activeOrg, setActiveOrg] = useState<string | null>(null);
  const prs = usePrStore((s) => s.prs);

  useEffect(() => {
    void ipc.getSettings().then(setSettings);
    void ipc.getOrgState().then((s) => setActiveOrg(s.active)).catch(() => {});
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
        {activeOrg && (
          <span
            className="settings-org-banner mono"
            title="Settings are per-organization — you are editing this org's"
          >
            {activeOrg}
          </span>
        )}
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
          {pane === "orgs" && <OrgsPane />}
          {pane === "repos" && (
            <ReposPane settings={settings} save={save} activeRepos={prs.map((p) => p.repo)} />
          )}
          {pane === "users" && (
            <UsersPane settings={settings} save={save} activeAuthors={prs.map((p) => p.author)} />
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

  const feedLimit = settings.calloutFeedLimit || 150;

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
          value={nearestIdx(POLL_STEPS, settings.pollIntervalSecs)}
          style={sliderFill(nearestIdx(POLL_STEPS, settings.pollIntervalSecs), POLL_STEPS.length - 1)}
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
        label={`Background org poll — every ${fmtInterval(settings.backgroundPollSecs || 300)}`}
        hint="How often orgs that are NOT active in the org selector refresh. They keep polling for awareness (native notifications), just at this gentler cadence."
      >
        <input
          type="range"
          className="interval-slider"
          min={0}
          max={POLL_STEPS.length - 1}
          value={nearestIdx(POLL_STEPS, settings.backgroundPollSecs || 300)}
          style={sliderFill(nearestIdx(POLL_STEPS, settings.backgroundPollSecs || 300), POLL_STEPS.length - 1)}
          onChange={(e) => void save({ backgroundPollSecs: POLL_STEPS[Number(e.target.value)] })}
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
          value={nearestIdx(AGE_STEPS, settings.prMaxAgeDays || 365)}
          style={sliderFill(nearestIdx(AGE_STEPS, settings.prMaxAgeDays || 365), AGE_STEPS.length - 1)}
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

      <Field
        label={`Callout feed — show ${feedLimit} items`}
        hint="How many activity rows the callout shows. A shorter feed is easier to reach the bottom of. Older rows stay in the local database — lowering this just shortens the list, it doesn't delete history."
      >
        <StepSlider
          steps={FEED_STEPS}
          value={feedLimit}
          onChange={(v) => void save({ calloutFeedLimit: v })}
        />
      </Field>
    </section>
  );
}

// ---------------------------------------------------------------- orgs

/** Which GitHub orgs CORA works with. Each enabled org gets a fully
 *  isolated database + settings; the selector in the rail switches. */
function OrgsPane() {
  const [available, setAvailable] = useState<
    import("../bindings/GithubOrg").GithubOrg[] | null
  >(null);
  const [orgState, setOrgState] = useState<
    import("../bindings/OrgState").OrgState | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => {
    void ipc.getOrgState().then(setOrgState).catch(() => {});
    void ipc
      .listGithubOrgs()
      .then(setAvailable)
      .catch((e) => setError(String(e)));
  };
  useEffect(refresh, []);

  const toggle = async (login: string, enable: boolean) => {
    if (!orgState) return;
    const next = enable
      ? [...orgState.enabled, login]
      : orgState.enabled.filter((o) => o !== login);
    setBusy(true);
    setError(null);
    try {
      await ipc.setEnabledOrgs(next);
      refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="pane-section">
      <h2>Organizations</h2>
      <p className="pane-intro">
        Every enabled organization is fully isolated: its own database, settings, watched
        repos, activity feed, and AWS configuration. Data never crosses org boundaries.
        Non-active orgs keep polling in the background at the gentler cadence set in
        General.
      </p>
      {error && <div className="settings-error">{error}</div>}
      {available == null && !error && <p className="pane-intro">Loading orgs from GitHub…</p>}
      {available?.map((org) => {
        const enabled = orgState?.enabled.includes(org.login) ?? false;
        const active = orgState?.active === org.login;
        return (
          <div key={org.login} className="org-row">
            <Toggle
              checked={enabled}
              disabled={busy || (enabled && (orgState?.enabled.length ?? 0) <= 1)}
              onChange={(v) => void toggle(org.login, v)}
            />
            <span className="org-login mono">{org.login}</span>
            {org.name !== org.login && <span className="org-name">{org.name}</span>}
            {org.personal && <span className="org-tag">personal</span>}
            {active && <span className="org-tag active">active</span>}
            {enabled && !active && (
              <button
                className="action-btn org-activate"
                disabled={busy}
                onClick={() => void ipc.setActiveOrg(org.login).then(refresh)}
              >
                Make active
              </button>
            )}
          </div>
        );
      })}
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
  const [filter, setFilter] = useState("");

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
    const q = filter.trim().toLowerCase();
    return [...all]
      .filter((repo) => !q || repo.toLowerCase().includes(q))
      .sort()
      .map((repo) => ({
        repo,
        watched: settings.watchedRepos.includes(repo),
        priority: settings.repoPriorities[repo] ?? ("normal" as RepoPriority),
        activePrs: activeCounts.get(repo) ?? 0,
      }));
  }, [settings, activeCounts, filter]);

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

      <input
        className="table-filter mono"
        placeholder="search repositories…"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />

      {rows.length === 0 ? (
        <p className="pane-intro">
          {filter
            ? "No repositories match."
            : "Nothing yet — repos appear here once you watch one or once PRs involving you are tracked."}
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

// ---------------------------------------------------------------- users

const LOGIN_RE = /^[A-Za-z0-9-]+(\[bot\])?$/;

/** Author priorities, mirroring the Repositories pane — see and adjust any
 *  user's weight without needing one of their PRs on screen. */
function UsersPane({
  settings,
  save,
  activeAuthors,
}: PaneProps & { activeAuthors: string[] }) {
  const [draft, setDraft] = useState("");
  const [draftError, setDraftError] = useState<string | null>(null);
  // Manually added logins with no stored priority yet — kept locally so the
  // row exists to pick a priority on (normal is never persisted).
  const [added, setAdded] = useState<string[]>([]);

  const activeCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const author of activeAuthors) counts.set(author, (counts.get(author) ?? 0) + 1);
    return counts;
  }, [activeAuthors]);

  const [filter, setFilter] = useState("");

  const rows = useMemo(() => {
    const all = new Set<string>([
      ...Object.keys(settings.authorPriorities),
      ...activeCounts.keys(),
      ...added,
    ]);
    const q = filter.trim().toLowerCase();
    return [...all]
      .filter((author) => !q || author.toLowerCase().includes(q))
      .sort()
      .map((author) => ({
        author,
        priority: settings.authorPriorities[author] ?? ("normal" as RepoPriority),
        activePrs: activeCounts.get(author) ?? 0,
      }));
  }, [settings, activeCounts, added, filter]);

  const setPriority = (author: string, priority: RepoPriority) => {
    const next = { ...settings.authorPriorities };
    if (priority === "normal") delete next[author];
    else next[author] = priority;
    void save({ authorPriorities: next });
  };

  const addUser = () => {
    const login = draft.trim().replace(/^@/, "");
    if (!LOGIN_RE.test(login)) {
      setDraftError("GitHub login only — letters, digits, hyphens (e.g. besendorfer)");
      return;
    }
    setDraftError(null);
    setDraft("");
    setAdded((a) => (a.includes(login) ? a : [...a, login]));
  };

  return (
    <section className="pane-section pane-wide">
      <h2>Users</h2>
      <p className="pane-intro">
        <strong>Priority</strong> weights a PR author everywhere — high authors float to the
        top of every group and their activity is always featured; ignored authors (bots,
        dependabot) are never tracked at all. Right-clicking a PR sets this too.
      </p>

      <div className="repo-add">
        <input
          placeholder="github login — press Enter to add"
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value);
            setDraftError(null);
          }}
          onKeyDown={(e) => e.key === "Enter" && draft && addUser()}
        />
        <button className="action-btn" disabled={!draft} onClick={addUser}>
          Add
        </button>
      </div>
      {draftError && <div className="settings-error">{draftError}</div>}

      <input
        className="table-filter mono"
        placeholder="search users…"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />

      {rows.length === 0 ? (
        <p className="pane-intro">
          {filter
            ? "No users match."
            : "Nothing yet — authors appear here once their PRs are tracked, or add one above."}
        </p>
      ) : (
        <table className="repo-table">
          <thead>
            <tr>
              <th>User</th>
              <th className="col-center">Open PRs</th>
              <th>Priority</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.author} className={row.priority === "ignored" ? "row-ignored" : ""}>
                <td className="mono repo-cell">@{row.author}</td>
                <td className="col-center mono">{row.activePrs > 0 ? row.activePrs : "—"}</td>
                <td>
                  <select
                    value={row.priority}
                    onChange={(e) => setPriority(row.author, e.target.value as RepoPriority)}
                  >
                    <option value="high">high</option>
                    <option value="normal">normal</option>
                    <option value="low">low</option>
                    <option value="ignored">ignored</option>
                  </select>
                </td>
                <td className="col-center">
                  {row.priority !== "normal" && (
                    <button
                      className="icon-btn"
                      title="Clear priority"
                      onClick={() => setPriority(row.author, "normal")}
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

  const archLimit = settings.archMaxOutputTokens || 16384;
  const codeLimit = settings.codeMaxOutputTokens || 16384;

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

      <Field
        label={`Architecture output ceiling — ${fmtTokens(archLimit)} tokens`}
        hint="Max output for the architecture pass (graph + assessment + pillar findings — the large submission). A ceiling, not a reservation: no cost unless reached. Keep it at or below your model's hard output cap, or Bedrock rejects the request."
      >
        <StepSlider
          steps={TOKEN_STEPS}
          value={archLimit}
          onChange={(v) => void save({ archMaxOutputTokens: v })}
          fmt={fmtTokens}
        />
      </Field>

      <Field
        label={`Code-pass output ceiling — ${fmtTokens(codeLimit)} tokens`}
        hint="Max output for the code-level findings pass. Its submission is short — this mostly bounds the model's reasoning before it submits. Same hard-cap caveat as above."
      >
        <StepSlider
          steps={TOKEN_STEPS}
          value={codeLimit}
          onChange={(v) => void save({ codeMaxOutputTokens: v })}
          fmt={fmtTokens}
        />
      </Field>
    </section>
  );
}
