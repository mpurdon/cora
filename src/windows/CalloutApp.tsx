import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { TrackedPr } from "../bindings/TrackedPr";
import {
  ACTION_META,
  ACTION_ORDER,
  bucketReason,
  inBucket,
  type ActionKind,
} from "../lib/actions";
import { ipc } from "../lib/ipc";
import { parseTitle, timeAgo, usePrStore } from "../state/prStore";

const ROW_HEIGHT = 44;
const LIST_CHROME = 30; // bucket label row

function Tile({
  kind,
  count,
  selected,
  pulsing,
  onSelect,
}: {
  kind: ActionKind;
  count: number;
  selected: boolean;
  pulsing: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      className={[
        "stat-tile",
        `tile-${kind}`,
        selected ? "selected" : "",
        count === 0 ? "zero" : "",
        pulsing ? "tile-pulse" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={(e) => {
        // Single click filters the list; double click jumps to the main
        // window pre-filtered on this bucket.
        if (e.detail === 2) {
          void invoke("show_main_filtered", { bucket: kind });
        } else {
          onSelect();
        }
      }}
      title={`${ACTION_META[kind].label} — double-click to open in CORA`}
    >
      <span className="tile-count">{count}</span>
      <span className="tile-label">{ACTION_META[kind].short}</span>
    </button>
  );
}

function FocusRow({ pr, kind }: { pr: TrackedPr; kind: ActionKind }) {
  return (
    <button
      className="action-row"
      onClick={() => void ipc.showMainWindow(pr.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        void ipc.setPrMuted(pr.id, true);
      }}
      title={`${pr.repo}#${pr.number} — click to open, right-click to mute`}
    >
      <span className="body">
        <span className="pr-title">{parseTitle(pr.title).clean}</span>
        <span className="action-reason">
          <span className="repo">
            {pr.repo.split("/")[1] ?? pr.repo}#{pr.number}
          </span>
          {" · "}
          {bucketReason(pr, kind)}
        </span>
      </span>
      <span className="ago">{timeAgo(pr.lastChangeAt)}</span>
    </button>
  );
}

export function CalloutApp() {
  const { prs, pollStatus, recentlyChanged, init } = usePrStore();
  const [selected, setSelected] = useState<ActionKind | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [maxRows, setMaxRows] = useState(5);

  useEffect(() => {
    document.body.classList.add("callout");
    void init();
  }, [init]);

  // The focus list never scrolls — fit rows to the space we actually have.
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      setMaxRows(Math.max(1, Math.floor((el.clientHeight - LIST_CHROME) / ROW_HEIGHT) - 1));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const buckets = useMemo(() => {
    const counts = new Map<ActionKind, TrackedPr[]>();
    for (const kind of ACTION_ORDER) {
      counts.set(
        kind,
        prs
          .filter((pr) => inBucket(pr, kind))
          .sort((a, b) => b.lastChangeAt.localeCompare(a.lastChangeAt)),
      );
    }
    return counts;
  }, [prs]);

  // Default focus: the most urgent non-empty bucket.
  const active: ActionKind | null =
    selected && (buckets.get(selected)?.length ?? 0) > 0
      ? selected
      : (ACTION_ORDER.find((k) => (buckets.get(k)?.length ?? 0) > 0) ?? null);

  const activeItems = active ? (buckets.get(active) ?? []) : [];
  const shown = activeItems.slice(0, maxRows);
  const overflow = activeItems.length - shown.length;

  const attention = new Set(
    ACTION_ORDER.filter((k) => k !== "new" && k !== "comments").flatMap((k) =>
      (buckets.get(k) ?? []).map((p) => p.id),
    ),
  ).size;

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
          {attention === 0 ? "all clear" : `${attention} need${attention === 1 ? "s" : ""} you`}
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
      ) : (
        <>
          <div className="stat-grid">
            {ACTION_ORDER.map((kind) => (
              <Tile
                key={kind}
                kind={kind}
                count={buckets.get(kind)?.length ?? 0}
                selected={active === kind}
                pulsing={(buckets.get(kind) ?? []).some((p) => recentlyChanged.has(p.id))}
                onSelect={() => setSelected(kind)}
              />
            ))}
          </div>

          <div className="focus-list" ref={listRef}>
            {active == null ? (
              <div className="callout-empty">
                <span>Nothing needs you right now.</span>
              </div>
            ) : (
              <>
                <div className="bucket-label eyebrow">{ACTION_META[active].label}</div>
                {shown.map((pr) => (
                  <FocusRow key={pr.id} pr={pr} kind={active} />
                ))}
                {overflow > 0 && (
                  <button
                    className="more-link"
                    onClick={() => void invoke("show_main_filtered", { bucket: active })}
                  >
                    + {overflow} more in CORA ↗
                  </button>
                )}
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}
