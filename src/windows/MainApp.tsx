import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PrSource } from "../bindings/PrSource";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { TrackedPr } from "../bindings/TrackedPr";
import { AssessmentView } from "../components/analysis/AssessmentView";
import { AwsAuthCard } from "../components/analysis/AwsAuthCard";
import { C4Canvas } from "../components/analysis/C4Canvas";
import { StatusStrip, UnreadMarker } from "../components/StatusStrip";
import { ipc, onFocusPr } from "../lib/ipc";
import { analysisKey, useAnalysisStore } from "../state/analysisStore";
import { ciTone, mergeTone, parseTitle, reviewTone, timeAgo, usePrStore } from "../state/prStore";
import { SettingsView } from "./SettingsView";

/** Reason grouping: a PR appears once, under its most specific reason. */
const REASONS: { key: PrSource; label: string }[] = [
  { key: "review-requested", label: "Needs your review" },
  { key: "authored", label: "Yours" },
  { key: "chat", label: "From chat" },
  { key: "manual", label: "Pinned" },
  { key: "involved", label: "Involved" },
  { key: "watched-repo", label: "Watched repos" },
];

type GroupMode = "org" | "repo" | "reason" | "type";
type SortMode = "activity" | "attention" | "repo";

const PRIORITY_WEIGHT: Record<RepoPriority, number> = { high: 0, normal: 1, low: 2, ignored: 3 };
const PRIORITY_CYCLE: RepoPriority[] = ["normal", "high", "low", "ignored"];

/** "Ready for my review" requirements, one per lamp. */
type ReadyFilters = { ciPass: boolean; reviewNeeded: boolean; noConflicts: boolean };
const NO_READY_FILTERS: ReadyFilters = { ciPass: false, reviewNeeded: false, noConflicts: false };

function passesReady(pr: TrackedPr, f: ReadyFilters): boolean {
  // ciPass admits "no checks configured" (idle) — nothing is blocking.
  if (f.ciPass && !["ok", "idle"].includes(ciTone(pr))) return false;
  // reviewNeeded keeps only PRs still awaiting a decision.
  if (f.reviewNeeded && reviewTone(pr) !== "warn") return false;
  // noConflicts drops only known-conflicting; UNKNOWN is GitHub still computing.
  if (f.noConflicts && mergeTone(pr) === "bad") return false;
  return true;
}

const orgOf = (repo: string) => repo.split("/")[0] ?? repo;
const shortRepo = (repo: string) => repo.split("/")[1] ?? repo;

function reasonOf(pr: TrackedPr): PrSource {
  return REASONS.find((g) => pr.sources.includes(g.key))?.key ?? "watched-repo";
}

/** Higher = needs the engineer sooner. */
function attentionScore(pr: TrackedPr): number {
  let score = pr.unread.length * 10;
  if (ciTone(pr) === "bad") score += 5;
  if (mergeTone(pr) === "bad") score += 3;
  if (reviewTone(pr) === "bad") score += 2;
  if (pr.sources.includes("review-requested")) score += 1;
  return score;
}

const SORTERS: Record<SortMode, (a: TrackedPr, b: TrackedPr) => number> = {
  activity: (a, b) => b.lastChangeAt.localeCompare(a.lastChangeAt),
  attention: (a, b) => attentionScore(b) - attentionScore(a) || b.lastChangeAt.localeCompare(a.lastChangeAt),
  repo: (a, b) => a.repo.localeCompare(b.repo) || a.number - b.number,
};

function usePersisted<T extends string>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => (localStorage.getItem(key) as T) ?? initial);
  const set = useCallback(
    (v: T) => {
      setValue(v);
      localStorage.setItem(key, v);
    },
    [key],
  );
  return [value, set] as const;
}

function Legend() {
  const [dismissed, setDismissed] = useState(
    () => localStorage.getItem("cora.legendDismissed") === "1",
  );
  if (dismissed) return null;
  return (
    <div className="legend">
      <button
        className="legend-close"
        title="Dismiss"
        onClick={() => {
          localStorage.setItem("cora.legendDismissed", "1");
          setDismissed(true);
        }}
      >
        ✕
      </button>
      The three lamps on each row, top to bottom: <strong>CI checks</strong>,{" "}
      <strong>review state</strong>, <strong>merge conflicts</strong>.
      <br />
      <span className="lamp ok" /> good · <span className="lamp warn" /> waiting ·{" "}
      <span className="lamp bad" /> blocked · <span className="lamp" /> n/a
      <br />
      On the left edge: <span className="marker new">◆</span> you haven't opened it yet ·{" "}
      <span className="marker count">2</span> updates since you last looked. Hover for details.
    </div>
  );
}

function TrackPrInput({ onDone }: { onDone: () => void }) {
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const track = async () => {
    try {
      await ipc.trackPrUrl(url);
      onDone();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <div className="track-inline">
      <input
        autoFocus
        placeholder="Paste a PR URL, press Enter"
        value={url}
        onChange={(e) => {
          setUrl(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && url) void track();
          if (e.key === "Escape") onDone();
        }}
      />
      {error && <span className="track-error">{error}</span>}
    </div>
  );
}

type Tab = "assessment" | "c4" | "diff";

/** Assessment + C4 tabs share one L1 analysis run per PR head. */
function AnalysisPanel({
  pr,
  tab,
  highlight,
  onFocusNodes,
}: {
  pr: TrackedPr;
  tab: "assessment" | "c4";
  highlight: string[];
  onFocusNodes: (ids: string[]) => void;
}) {
  const { runs, init, ensure, start } = useAnalysisStore();
  const run = runs[analysisKey(pr.id, "context")];

  useEffect(() => {
    void init().then(() => ensure(pr.id, "context"));
  }, [pr.id, pr.headSha, init, ensure]);

  if (!run || run.status === "idle") {
    return (
      <div className="placeholder">
        <p>
          CORA reads the diff and explores the repository to place this change in the
          architecture — external-boundary effects first.
        </p>
        <button className="action-btn analyze-btn" onClick={() => void start(pr.id, "context")}>
          Analyze architecture
        </button>
      </div>
    );
  }

  if (run.status === "running") {
    return (
      <div className="placeholder analysis-running">
        <span className="sync-dot live" />
        <div className="progress-ticker">
          {run.progress.slice(-8).map((message, i) => (
            <div key={i} className={i === run.progress.slice(-8).length - 1 ? "current" : ""}>
              {message}
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (run.status === "error") {
    if (run.errorKind === "aws-auth") {
      return (
        <AwsAuthCard
          detail={run.error ?? ""}
          onSignedIn={() => void start(pr.id, "context")}
        />
      );
    }
    if (run.errorKind === "github-auth") {
      return (
        <div className="auth-card">
          <div className="auth-title">
            <span className="lamp bad" />
            GitHub token needed
          </div>
          <p className="auth-body">
            The analysis engine reads the repository through GitHub. Add a personal access
            token in Settings → GitHub, then retry.
          </p>
          <div className="auth-actions">
            <button className="action-btn" onClick={() => void start(pr.id, "context")}>
              Retry
            </button>
          </div>
        </div>
      );
    }
    return (
      <div className="auth-card">
        <div className="auth-title">
          <span className="lamp bad" />
          Analysis failed
        </div>
        <pre className="auth-detail">{run.error}</pre>
        <div className="auth-actions">
          <button className="action-btn" onClick={() => void start(pr.id, "context")}>
            Retry
          </button>
        </div>
      </div>
    );
  }

  const result = run.result!;
  if (result.headSha !== pr.headSha) {
    // Stale — new commits landed since this analysis.
    return (
      <div className="placeholder">
        <p>New commits landed since the last analysis.</p>
        <button className="action-btn" onClick={() => void start(pr.id, "context")}>
          Re-analyze
        </button>
      </div>
    );
  }

  return tab === "assessment" ? (
    <AssessmentView assessment={result.assessment} onFocusNodes={onFocusNodes} />
  ) : (
    <C4Canvas graph={result.graph} highlightIds={highlight} />
  );
}

function Detail({
  pr,
  repoPriority,
  onRepoPriority,
}: {
  pr: TrackedPr;
  repoPriority: RepoPriority;
  onRepoPriority: (p: RepoPriority) => Promise<void>;
}) {
  const [tab, setTab] = useState<Tab>("assessment");
  const [highlight, setHighlight] = useState<string[]>([]);
  const { type, clean } = parseTitle(pr.title);
  const focusNodes = (ids: string[]) => {
    setHighlight(ids);
    if (ids.length > 0) setTab("c4");
  };
  return (
    <div className="detail">
      <div className="crumbs">
        <span className="eyebrow">
          {pr.repo} · #{pr.number} · by {pr.author} · updated {timeAgo(pr.updatedAt)} ago
        </span>
      </div>
      <h1>
        {pr.isDraft ? "Draft: " : ""}
        {clean}
      </h1>
      <div className="facts">
        <span className="fact">
          <StatusStrip pr={pr} />
        </span>
        {type !== "unknown" && <span className="fact mono">{type}</span>}
        <span className="fact mono">{pr.state.toLowerCase()}</span>
        <span className="fact mono diffstat">
          <span className="add">+{pr.additions}</span> <span className="del">−{pr.deletions}</span>{" "}
          · {pr.changedFiles} files
        </span>
        {pr.labels.map((l) => (
          <span key={l.name} className="label-chip">
            {l.name}
          </span>
        ))}
      </div>
      <div className="actions">
        <a className="action-btn" href={pr.url} target="_blank" rel="noreferrer">
          Open on GitHub ↗
        </a>
        <button className="action-btn" onClick={() => void ipc.setPrMuted(pr.id, !pr.muted)}>
          {pr.muted ? "Unmute" : "Mute"}
        </button>
        <button className="action-btn" onClick={() => void ipc.untrackPr(pr.id)}>
          Untrack
        </button>
        <label className="prio-select">
          repo priority
          <select
            value={repoPriority}
            onChange={(e) => void onRepoPriority(e.target.value as RepoPriority)}
          >
            <option value="high">high</option>
            <option value="normal">normal</option>
            <option value="low">low</option>
            <option value="ignored">ignored</option>
          </select>
        </label>
      </div>

      <div className="tabs" role="tablist">
        {(
          [
            ["assessment", "Assessment"],
            ["c4", "Architecture"],
            ["diff", "Diff"],
          ] as [Tab, string][]
        ).map(([key, label]) => (
          <button
            key={key}
            role="tab"
            aria-selected={tab === key}
            className={`tab${tab === key ? " active" : ""}`}
            onClick={() => setTab(key)}
          >
            {label}
          </button>
        ))}
      </div>

      {(tab === "assessment" || tab === "c4") && (
        <AnalysisPanel pr={pr} tab={tab} highlight={highlight} onFocusNodes={focusNodes} />
      )}
      {tab === "diff" && (
        <div className="placeholder">
          File-level diff view is coming. For now, use “Open on GitHub”.
        </div>
      )}
    </div>
  );
}

export function MainApp() {
  const { prs, pollStatus, init } = usePrStore();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showTrackInput, setShowTrackInput] = useState(false);

  const [groupMode, setGroupMode] = usePersisted<GroupMode>("cora.groupMode", "org");
  const [sortMode, setSortMode] = usePersisted<SortMode>("cora.sortMode", "attention");
  const [filter, setFilter] = useState("");
  const [ready, setReady] = useState<ReadyFilters>(() => {
    try {
      return { ...NO_READY_FILTERS, ...JSON.parse(localStorage.getItem("cora.readyFilters") ?? "{}") };
    } catch {
      return NO_READY_FILTERS;
    }
  });
  const toggleReady = (key: keyof ReadyFilters) => {
    setReady((prev) => {
      const next = { ...prev, [key]: !prev[key] };
      localStorage.setItem("cora.readyFilters", JSON.stringify(next));
      return next;
    });
  };

  const [priorities, setPriorities] = useState<Record<string, RepoPriority>>({});
  useEffect(() => {
    void ipc.getSettings().then((s) => setPriorities(s.repoPriorities));
  }, [showSettings]); // re-read after the settings page closes
  const prioOf = useCallback(
    (repo: string): RepoPriority => priorities[repo] ?? "normal",
    [priorities],
  );
  const setRepoPriority = async (repo: string, p: RepoPriority) => {
    const s = await ipc.getSettings();
    const next = { ...s.repoPriorities };
    if (p === "normal") delete next[repo];
    else next[repo] = p;
    await ipc.setSettings({ ...s, repoPriorities: next });
    setPriorities(next);
  };
  const [collapsed, setCollapsed] = useState<Set<string>>(
    () => new Set(JSON.parse(localStorage.getItem("cora.collapsed") ?? "[]") as string[]),
  );

  const [railWidth, setRailWidth] = useState(() =>
    Math.min(520, Math.max(220, Number(localStorage.getItem("cora.railWidth")) || 300)),
  );
  const dragging = useRef(false);

  useEffect(() => {
    void init();
    const unlisten = onFocusPr((id) => {
      setShowSettings(false);
      setSelectedId(id);
      void ipc.markPrRead(id);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [init]);

  const toggleGroup = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      localStorage.setItem("cora.collapsed", JSON.stringify([...next]));
      return next;
    });
  };

  const startResize = (e: React.PointerEvent) => {
    dragging.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };
  const onResize = (e: React.PointerEvent) => {
    if (!dragging.current) return;
    const w = Math.min(520, Math.max(220, e.clientX));
    setRailWidth(w);
  };
  const endResize = () => {
    if (!dragging.current) return;
    dragging.current = false;
    localStorage.setItem("cora.railWidth", String(railWidth));
  };

  const { grouped, hiddenByReady } = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const textMatched = prs.filter(
      (pr) =>
        !q ||
        pr.repo.toLowerCase().includes(q) ||
        pr.title.toLowerCase().includes(q) ||
        pr.author.toLowerCase().includes(q) ||
        String(pr.number).includes(q),
    );
    const unignored = textMatched.filter((pr) => prioOf(pr.repo) !== "ignored");
    const visible = unignored.filter((pr) => passesReady(pr, ready));
    const hiddenByReady = unignored.length - visible.length;
    const sorted = [...visible].sort(
      (a, b) =>
        PRIORITY_WEIGHT[prioOf(a.repo)] - PRIORITY_WEIGHT[prioOf(b.repo)] ||
        SORTERS[sortMode](a, b),
    );

    const byKey = new Map<string, { label: string; prs: TrackedPr[] }>();
    for (const pr of sorted) {
      const [key, label] =
        groupMode === "org"
          ? [orgOf(pr.repo), orgOf(pr.repo)]
          : groupMode === "repo"
            ? [pr.repo, shortRepo(pr.repo)]
            : groupMode === "type"
              ? [parseTitle(pr.title).type, parseTitle(pr.title).type]
              : [reasonOf(pr), REASONS.find((r) => r.key === reasonOf(pr))!.label];
      const bucket = byKey.get(key) ?? { label, prs: [] };
      bucket.prs.push(pr);
      byKey.set(key, bucket);
    }
    const entries = [...byKey.entries()].map(([key, v]) => ({ key, ...v }));
    // High-priority repos surface their groups first in every mode.
    const groupWeight = (g: { prs: TrackedPr[] }) =>
      Math.min(...g.prs.map((p) => PRIORITY_WEIGHT[prioOf(p.repo)]));
    if (groupMode === "reason") {
      entries.sort(
        (a, b) =>
          REASONS.findIndex((r) => r.key === a.key) - REASONS.findIndex((r) => r.key === b.key),
      );
    } else if (groupMode === "type") {
      // known types alphabetical, "unknown" last
      entries.sort(
        (a, b) =>
          groupWeight(a) - groupWeight(b) ||
          (a.key === "unknown" ? 1 : b.key === "unknown" ? -1 : a.label.localeCompare(b.label)),
      );
    } else {
      entries.sort((a, b) => groupWeight(a) - groupWeight(b) || a.label.localeCompare(b.label));
    }
    return { grouped: entries, hiddenByReady };
  }, [prs, filter, sortMode, groupMode, ready, prioOf]);

  const selected = prs.find((p) => p.id === selectedId) ?? null;

  const select = (id: string) => {
    setShowSettings(false);
    setSelectedId(id);
    void ipc.markPrRead(id);
  };

  return (
    <div className="main-shell" onPointerMove={onResize} onPointerUp={endResize}>
      <nav className="rail" style={{ width: railWidth }}>
        <div className="rail-header">
          <span className="name">CORA</span>
          <span className="eyebrow">{prs.length} tracked</span>
          <span className="spacer" />
          <button
            className="icon-btn"
            title="Track a PR by URL"
            onClick={() => setShowTrackInput((s) => !s)}
          >
            +
          </button>
        </div>
        {showTrackInput && <TrackPrInput onDone={() => setShowTrackInput(false)} />}

        <div className="rail-controls">
          <input
            className="filter-input"
            placeholder="Filter repo, title, author…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <select
            title="Group by"
            value={groupMode}
            onChange={(e) => setGroupMode(e.target.value as GroupMode)}
          >
            <option value="org">by org</option>
            <option value="repo">by repo</option>
            <option value="type">by type</option>
            <option value="reason">by reason</option>
          </select>
          <select
            title="Sort by"
            value={sortMode}
            onChange={(e) => setSortMode(e.target.value as SortMode)}
          >
            <option value="attention">attention</option>
            <option value="activity">activity</option>
            <option value="repo">repo</option>
          </select>
        </div>

        <div className="filter-chips">
          <button
            className={`chip${ready.ciPass ? " on" : ""}`}
            title="Show only PRs whose CI checks pass (or that have no checks)"
            onClick={() => toggleReady("ciPass")}
          >
            <span className="lamp ok" /> ci passing
          </button>
          <button
            className={`chip${ready.reviewNeeded ? " on" : ""}`}
            title="Show only PRs still awaiting a review decision"
            onClick={() => toggleReady("reviewNeeded")}
          >
            <span className="lamp warn" /> needs review
          </button>
          <button
            className={`chip${ready.noConflicts ? " on" : ""}`}
            title="Hide PRs with merge conflicts"
            onClick={() => toggleReady("noConflicts")}
          >
            <span className="lamp bad" /> no conflicts
          </button>
          {hiddenByReady > 0 && <span className="hidden-note">−{hiddenByReady}</span>}
        </div>

        <Legend />

        <div className="rail-list">
          {grouped.length === 0 && (
            <div className="rail-group">
              <span className="eyebrow">{filter ? "No matches" : "Nothing tracked yet"}</span>
            </div>
          )}
          {grouped.map((group) => {
            const isCollapsed = collapsed.has(`${groupMode}:${group.key}`);
            const unreadSum = group.prs.reduce((n, p) => n + p.unread.length, 0);
            const repoPrio = groupMode === "repo" ? prioOf(group.key) : null;
            return (
              <div key={group.key} className="rail-group">
                <button
                  className="group-header"
                  onClick={() => toggleGroup(`${groupMode}:${group.key}`)}
                  aria-expanded={!isCollapsed}
                >
                  <span className="chevron">{isCollapsed ? "▸" : "▾"}</span>
                  <span className="eyebrow">{group.label}</span>
                  <span className="group-count">{group.prs.length}</span>
                  {repoPrio && repoPrio !== "normal" && (
                    <span className={`prio-tag ${repoPrio}`}>{repoPrio}</span>
                  )}
                  <span className="spacer" />
                  {groupMode === "repo" && (
                    <span
                      className="flag-btn"
                      role="button"
                      title={`Priority: ${repoPrio}. Click to cycle high → low → ignored.`}
                      onClick={(e) => {
                        e.stopPropagation();
                        const next =
                          PRIORITY_CYCLE[
                            (PRIORITY_CYCLE.indexOf(repoPrio ?? "normal") + 1) %
                              PRIORITY_CYCLE.length
                          ];
                        void setRepoPriority(group.key, next);
                      }}
                    >
                      ⚑
                    </span>
                  )}
                  {unreadSum > 0 && (
                    <span
                      className="unread-count"
                      title={`${unreadSum} unacknowledged update${unreadSum > 1 ? "s" : ""} in this group`}
                    >
                      {unreadSum}
                    </span>
                  )}
                </button>
                {!isCollapsed &&
                  group.prs.map((pr) => (
                    <button
                      key={pr.id}
                      className={`rail-row${pr.id === selectedId ? " selected" : ""}`}
                      onClick={() => select(pr.id)}
                      title={`${pr.repo}#${pr.number} — ${pr.title}`}
                    >
                      <UnreadMarker pr={pr} />
                      <StatusStrip pr={pr} />
                      <span className="body">
                        <span className="repo">
                          {groupMode === "repo" ? `#${pr.number}` : `${shortRepo(pr.repo)}#${pr.number}`}
                          {groupMode === "reason" && (
                            <span className="repo-org"> · {orgOf(pr.repo)}</span>
                          )}
                        </span>
                        <span className="pr-title">{parseTitle(pr.title).clean}</span>
                      </span>
                    </button>
                  ))}
              </div>
            );
          })}
        </div>
        <div className="rail-footer">
          <span
            className={`sync-dot ${pollStatus == null ? "" : pollStatus.ok ? "live" : "err"}`}
          />
          <span className="status-text">
            {pollStatus == null
              ? "starting…"
              : pollStatus.ok
                ? `synced ${timeAgo(pollStatus.at)} ago`
                : (pollStatus.message ?? "sync failed")}
          </span>
          <button
            className="icon-btn"
            title="Toggle callout"
            onClick={() => void ipc.toggleCallout()}
          >
            ▣
          </button>
          <button className="icon-btn" title="Settings" onClick={() => setShowSettings((s) => !s)}>
            ⚙
          </button>
        </div>
      </nav>
      <div
        className="rail-resizer"
        role="separator"
        aria-orientation="vertical"
        onPointerDown={startResize}
      />

      <main className="content">
        {showSettings ? (
          <SettingsView onClose={() => setShowSettings(false)} />
        ) : selected ? (
          <Detail
            pr={selected}
            repoPriority={prioOf(selected.repo)}
            onRepoPriority={(p) => setRepoPriority(selected.repo, p)}
          />
        ) : (
          <div className="empty-detail">Select a pull request — or ⚙ to connect GitHub.</div>
        )}
      </main>
    </div>
  );
}
