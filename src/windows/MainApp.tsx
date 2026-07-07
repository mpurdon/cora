import { useEffect, useMemo, useState } from "react";
import type { PrSource } from "../bindings/PrSource";
import type { TrackedPr } from "../bindings/TrackedPr";
import { StatusStrip } from "../components/StatusStrip";
import { ipc, onFocusPr } from "../lib/ipc";
import { timeAgo, usePrStore } from "../state/prStore";
import { SettingsView } from "./SettingsView";

/** A PR appears once, under its most specific reason for being tracked. */
const GROUPS: { key: PrSource; label: string }[] = [
  { key: "review-requested", label: "Needs your review" },
  { key: "authored", label: "Yours" },
  { key: "chat", label: "From chat" },
  { key: "manual", label: "Pinned" },
  { key: "involved", label: "Involved" },
  { key: "watched-repo", label: "Watched repos" },
];

function groupOf(pr: TrackedPr): PrSource {
  return GROUPS.find((g) => pr.sources.includes(g.key))?.key ?? "watched-repo";
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
        <button
          className="action-btn"
          onClick={() => void ipc.setPrMuted(pr.id, !pr.muted)}
        >
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

export function MainApp() {
  const { prs, pollStatus, init } = usePrStore();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showTrackInput, setShowTrackInput] = useState(false);

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

  const grouped = useMemo(() => {
    const byGroup = new Map<PrSource, TrackedPr[]>();
    for (const pr of prs) {
      const g = groupOf(pr);
      byGroup.set(g, [...(byGroup.get(g) ?? []), pr]);
    }
    return GROUPS.filter((g) => byGroup.has(g.key)).map((g) => ({
      ...g,
      prs: byGroup.get(g.key)!,
    }));
  }, [prs]);

  const selected = prs.find((p) => p.id === selectedId) ?? null;

  const select = (id: string) => {
    setShowSettings(false);
    setSelectedId(id);
    void ipc.markPrRead(id);
  };

  return (
    <div className="main-shell">
      <nav className="rail">
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
        <div className="rail-list">
          {grouped.length === 0 && (
            <div className="rail-group">
              <span className="eyebrow">Nothing tracked yet</span>
            </div>
          )}
          {grouped.map((group) => (
            <div key={group.key} className="rail-group">
              <span className="eyebrow">{group.label}</span>
              {group.prs.map((pr) => (
                <button
                  key={pr.id}
                  className={`rail-row${pr.id === selectedId ? " selected" : ""}`}
                  onClick={() => select(pr.id)}
                >
                  <StatusStrip pr={pr} />
                  <span className="body">
                    <span className="repo">
                      {pr.repo}#{pr.number}
                    </span>
                    <span className="pr-title">{pr.title}</span>
                  </span>
                  {pr.unread.length > 0 && (
                    <span className="unread-count">{pr.unread.length}</span>
                  )}
                </button>
              ))}
            </div>
          ))}
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
          <button className="icon-btn" title="Toggle callout" onClick={() => void ipc.toggleCallout()}>
            ▣
          </button>
          <button
            className="icon-btn"
            title="Settings"
            onClick={() => setShowSettings((s) => !s)}
          >
            ⚙
          </button>
        </div>
      </nav>

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
