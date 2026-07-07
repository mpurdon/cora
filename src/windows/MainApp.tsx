import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PrSource } from "../bindings/PrSource";
import type { TrackedPr } from "../bindings/TrackedPr";
import { StatusStrip, unreadTitle } from "../components/StatusStrip";
import { ipc, onFocusPr } from "../lib/ipc";
import { ciTone, mergeTone, reviewTone, timeAgo, usePrStore } from "../state/prStore";
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

type GroupMode = "org" | "repo" | "reason";
type SortMode = "activity" | "attention" | "repo";

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
      <span className="unread-count">2</span> updates since you last opened that PR — hover any of
      them for details.
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

function Detail({ pr }: { pr: TrackedPr }) {
  const [tab, setTab] = useState<Tab>("assessment");
  return (
    <div className="detail">
      <div className="crumbs">
        <span className="eyebrow">
          {pr.repo} · #{pr.number} · by {pr.author} · updated {timeAgo(pr.updatedAt)} ago
        </span>
      </div>
      <h1>
        {pr.isDraft ? "Draft: " : ""}
        {pr.title}
      </h1>
      <div className="facts">
        <span className="fact">
          <StatusStrip pr={pr} />
        </span>
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

      {tab === "assessment" && (
        <div className="placeholder">
          Architecture-fit assessment lands here next: boundary impacts first, then
          Well-Architected findings by pillar.
        </div>
      )}
      {tab === "c4" && (
        <div className="placeholder">
          The interactive C4 canvas lands here next: affected containers and the boundaries this
          change crosses, drill-down on demand.
        </div>
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

  const grouped = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const visible = prs.filter(
      (pr) =>
        !q ||
        pr.repo.toLowerCase().includes(q) ||
        pr.title.toLowerCase().includes(q) ||
        pr.author.toLowerCase().includes(q) ||
        String(pr.number).includes(q),
    );
    const sorted = [...visible].sort(SORTERS[sortMode]);

    const byKey = new Map<string, { label: string; prs: TrackedPr[] }>();
    for (const pr of sorted) {
      const [key, label] =
        groupMode === "org"
          ? [orgOf(pr.repo), orgOf(pr.repo)]
          : groupMode === "repo"
            ? [pr.repo, shortRepo(pr.repo)]
            : [reasonOf(pr), REASONS.find((r) => r.key === reasonOf(pr))!.label];
      const bucket = byKey.get(key) ?? { label, prs: [] };
      bucket.prs.push(pr);
      byKey.set(key, bucket);
    }
    const entries = [...byKey.entries()].map(([key, v]) => ({ key, ...v }));
    if (groupMode === "reason") {
      entries.sort(
        (a, b) =>
          REASONS.findIndex((r) => r.key === a.key) - REASONS.findIndex((r) => r.key === b.key),
      );
    } else {
      entries.sort((a, b) => a.label.localeCompare(b.label));
    }
    return entries;
  }, [prs, filter, sortMode, groupMode]);

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
                  <span className="spacer" />
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
                      title={`${pr.repo}#${pr.number}`}
                    >
                      <StatusStrip pr={pr} />
                      <span className="body">
                        <span className="repo">
                          {groupMode === "repo" ? `#${pr.number}` : `${shortRepo(pr.repo)}#${pr.number}`}
                          {groupMode === "reason" && (
                            <span className="repo-org"> · {orgOf(pr.repo)}</span>
                          )}
                        </span>
                        <span className="pr-title">{pr.title}</span>
                      </span>
                      {pr.unread.length > 0 && (
                        <span className="unread-count" title={unreadTitle(pr)}>
                          {pr.unread.length}
                        </span>
                      )}
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
          <Detail pr={selected} />
        ) : (
          <div className="empty-detail">Select a pull request — or ⚙ to connect GitHub.</div>
        )}
      </main>
    </div>
  );
}
