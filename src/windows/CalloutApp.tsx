import { useEffect, useMemo } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { TrackedPr } from "../bindings/TrackedPr";
import { UnreadMarker } from "../components/StatusStrip";
import { ipc } from "../lib/ipc";
import { ciTone, mergeTone, parseTitle, timeAgo, usePrStore } from "../state/prStore";

/** What the user should DO about a PR — the callout's organizing principle. */
type ActionKind = "fix" | "address" | "merge" | "review" | "updates";

interface ActionItem {
  pr: TrackedPr;
  kind: ActionKind;
  reason: string;
}

const BUCKETS: { kind: ActionKind; label: string; verb: string }[] = [
  { kind: "fix", label: "Fix — your PR is blocked", verb: "fix" },
  { kind: "address", label: "Address review feedback", verb: "address" },
  { kind: "merge", label: "Ready to merge", verb: "merge" },
  { kind: "review", label: "Waiting on your review", verb: "review" },
  { kind: "updates", label: "New activity", verb: "look" },
];

/** First matching rule wins — ordered by how urgently it needs the user. */
function classify(pr: TrackedPr): ActionItem | null {
  if (pr.muted || pr.state !== "OPEN") return null;
  const authored = pr.sources.includes("authored");
  const reviewRequested = pr.sources.includes("review-requested");

  if (authored) {
    if (ciTone(pr) === "bad") {
      return { pr, kind: "fix", reason: "CI failing" };
    }
    if (mergeTone(pr) === "bad") {
      return { pr, kind: "fix", reason: "merge conflicts" };
    }
    if (pr.reviewDecision === "CHANGES_REQUESTED") {
      return { pr, kind: "address", reason: "changes requested" };
    }
    if (pr.reviewDecision === "APPROVED" && ciTone(pr) !== "warn" && !pr.isDraft) {
      return { pr, kind: "merge", reason: "approved · checks green" };
    }
  }

  if (reviewRequested && !pr.isDraft && pr.reviewDecision !== "APPROVED") {
    const gates: string[] = [];
    if (ciTone(pr) === "warn") gates.push("CI running");
    if (ciTone(pr) === "bad") gates.push("CI red");
    if (mergeTone(pr) === "bad") gates.push("conflicts");
    return {
      pr,
      kind: "review",
      reason: gates.length > 0 ? gates.join(" · ") : "ready for review",
    };
  }

  if (pr.unread.length > 0) {
    const kinds = [...new Set(pr.unread)].map((k) => k.replace(/-/g, " "));
    return { pr, kind: "updates", reason: kinds.join(", ") };
  }

  return null;
}

function Row({ item, verb }: { item: ActionItem; verb: string }) {
  const { pr } = item;
  return (
    <button
      className={`action-row kind-${item.kind}`}
      onClick={() => void ipc.showMainWindow(pr.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        void ipc.setPrMuted(pr.id, true);
      }}
      title={`${pr.repo}#${pr.number} — click to open, right-click to mute`}
    >
      <span className={`verb-chip ${item.kind}`}>{verb}</span>
      <span className="body">
        <span className="pr-title">{parseTitle(pr.title).clean}</span>
        <span className="action-reason">
          <span className="repo">{pr.repo.split("/")[1] ?? pr.repo}#{pr.number}</span>
          {" · "}
          {item.reason}
          {pr.sources.includes("chat") && <span className="badge-chat">chat</span>}
        </span>
      </span>
      <span className="right">
        <span className="ago">{timeAgo(pr.lastChangeAt)}</span>
        <UnreadMarker pr={pr} />
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

  const buckets = useMemo(() => {
    const items = prs
      .map(classify)
      .filter((i): i is ActionItem => i !== null)
      .sort((a, b) => b.pr.lastChangeAt.localeCompare(a.pr.lastChangeAt));
    return BUCKETS.map((bucket) => ({
      ...bucket,
      items: items.filter((i) => i.kind === bucket.kind),
    })).filter((bucket) => bucket.items.length > 0);
  }, [prs]);

  const total = buckets.reduce((n, b) => n + b.items.length, 0);
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
          {total === 0 ? "all clear" : `${total} need${total === 1 ? "s" : ""} you`}
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
      ) : total === 0 ? (
        <div className="callout-empty">
          <span>Nothing needs you right now.</span>
        </div>
      ) : (
        <div className="callout-list">
          {buckets.map((bucket) => (
            <section key={bucket.kind} className="action-bucket">
              <div className="bucket-label eyebrow">
                {bucket.label}
                <span className="bucket-count">{bucket.items.length}</span>
              </div>
              {bucket.items.map((item) => (
                <div
                  key={item.pr.id}
                  className={recentlyChanged.has(item.pr.id) ? "row-pulse" : undefined}
                >
                  <Row item={item} verb={bucket.verb} />
                </div>
              ))}
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
