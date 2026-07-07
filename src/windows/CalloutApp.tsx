import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { TrackedPr } from "../bindings/TrackedPr";
import { StatusStrip, unreadTitle } from "../components/StatusStrip";
import { ipc } from "../lib/ipc";
import { timeAgo, usePrStore } from "../state/prStore";

function Row({ pr, pulsing }: { pr: TrackedPr; pulsing: boolean }) {
  return (
    <button
      className={`callout-row${pr.muted ? " muted-pr" : ""}`}
      onClick={() => void ipc.showMainWindow(pr.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        void ipc.setPrMuted(pr.id, !pr.muted);
      }}
      title={`${pr.repo}#${pr.number} — click to open, right-click to ${pr.muted ? "unmute" : "mute"}`}
    >
      <StatusStrip pr={pr} pulsing={pulsing} />
      <span className="body">
        <span className="meta">
          <span className="repo">
            {pr.repo}#{pr.number}
          </span>
          {pr.sources.includes("chat") && <span className="badge-chat">chat</span>}
        </span>
        <span className="pr-title">
          {pr.isDraft ? "· draft · " : ""}
          {pr.title}
        </span>
      </span>
      <span className="right">
        <span className="ago">{timeAgo(pr.lastChangeAt)}</span>
        {pr.unread.length > 0 && (
          <span className="unread-count" title={unreadTitle(pr)}>
            {pr.unread.length}
          </span>
        )}
      </span>
    </button>
  );
}

export function CalloutApp() {
  const { prs, pollStatus, recentlyChanged, init } = usePrStore();

  useEffect(() => {
    document.body.classList.add("callout");
    void init();
  }, [init]);

  const open = prs.filter((p) => p.state === "OPEN" || p.unread.length > 0);
  const noToken = pollStatus?.ok === false && pollStatus.message?.includes("no GitHub token");
  const syncClass = pollStatus == null ? "" : pollStatus.ok ? "live" : "err";

  return (
    <div className="callout-shell">
      <header className="callout-header" data-tauri-drag-region>
        <span className={`sync-dot ${syncClass}`} title={pollStatus?.message ?? "syncing"} />
        <span className="title" data-tauri-drag-region>
          CORA
        </span>
        <span className="eyebrow" data-tauri-drag-region>
          {open.length} open
        </span>
        <span className="spacer" data-tauri-drag-region />
        <button className="icon-btn" title="Refresh now" onClick={() => void ipc.pollNow()}>
          ⟳
        </button>
        <button className="icon-btn" title="Open CORA" onClick={() => void ipc.showMainWindow()}>
          ⌂
        </button>
        <button
          className="icon-btn"
          title="Hide callout"
          onClick={() => void getCurrentWindow().hide()}
        >
          ✕
        </button>
      </header>

      {noToken ? (
        <div className="callout-empty">
          <span>Connect GitHub to start tracking pull requests.</span>
          <button className="action" onClick={() => void ipc.showMainWindow()}>
            Open settings
          </button>
        </div>
      ) : open.length === 0 ? (
        <div className="callout-empty">
          <span>All clear — nothing needs you.</span>
        </div>
      ) : (
        <div className="callout-list">
          {open.map((pr) => (
            <Row key={pr.id} pr={pr} pulsing={recentlyChanged.has(pr.id)} />
          ))}
        </div>
      )}
    </div>
  );
}
