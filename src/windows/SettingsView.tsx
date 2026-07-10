import { useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";
import { ipc } from "../lib/ipc";
import { usePrStore } from "../state/prStore";
import { DeveloperPane } from "./DeveloperPane";

type Pane = "general" | "github" | "repos" | "aws" | "developer";

const PANES: { key: Pane; label: string; glyph: string; dev?: boolean }[] = [
  { key: "general", label: "General", glyph: "◐" },
  { key: "github", label: "GitHub", glyph: "⎇" },
  { key: "repos", label: "Repositories", glyph: "▤" },
  { key: "aws", label: "AWS", glyph: "▲" },
  { key: "developer", label: "Developer", glyph: "⌬", dev: true },
];

const REPO_RE = /^[\w.-]+\/[\w.-]+$/;

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
      {hint && <div className="field-hint">{hint}</div>}
    </div>
  );
}

export function SettingsView({ onClose }: { onClose: () => void }) {
  const [pane, setPane] = useState<Pane>("github");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
        {PANES.filter((p) => !p.dev || settings.developerMode).map((p) => (
          <button
            key={p.key}
            className={`settings-nav-item${pane === p.key ? " active" : ""}`}
            onClick={() => setPane(p.key)}
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

      <div className="settings-pane">
        {pane === "general" && <GeneralPane settings={settings} save={save} />}
        {pane === "github" && <GitHubPane settings={settings} save={save} />}
        {pane === "repos" && (
          <ReposPane settings={settings} save={save} activeRepos={prs.map((p) => p.repo)} />
        )}
        {pane === "aws" && <AwsPane settings={settings} save={save} />}
        {pane === "developer" && settings.developerMode && (
          <DeveloperPane settings={settings} save={save} />
        )}
      </div>
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

      <Field label="Poll interval (seconds)" hint="How often CORA checks GitHub for changes. Minimum 15.">
        <input
          type="number"
          min={15}
          className="input-narrow"
          value={settings.pollIntervalSecs}
          onChange={(e) => void save({ pollIntervalSecs: Math.max(15, Number(e.target.value) || 45) })}
        />
      </Field>

      <Field
        label="Callout window"
        hint="The small always-on-top PR panel. You can always toggle it from the tray or with ▣."
      >
        <label className="check-row">
          <input
            type="checkbox"
            checked={settings.showCalloutOnStartup}
            onChange={(e) => void save({ showCalloutOnStartup: e.target.checked })}
          />
          Open the callout at launch
        </label>
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
        <label className="check-row">
          <input
            type="checkbox"
            checked={settings.autoAnalyzeReviewRequests}
            onChange={(e) => void save({ autoAnalyzeReviewRequests: e.target.checked })}
          />
          Auto-analyze new review requests
        </label>
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
        <label className="check-row">
          <input
            type="checkbox"
            checked={settings.developerMode}
            onChange={(e) => void save({ developerMode: e.target.checked })}
          />
          Enable developer mode
        </label>
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
                  <input
                    type="checkbox"
                    checked={row.watched}
                    onChange={(e) => toggleWatched(row.repo, e.target.checked)}
                  />
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
