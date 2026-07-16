import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ActivityItem } from "../bindings/ActivityItem";
import type { TrackedPr } from "../bindings/TrackedPr";
import { ACTION_META, ACTION_ORDER, inBucket, type ActionKind } from "../lib/actions";
import { ipc } from "../lib/ipc";
import { usePrStore } from "../state/prStore";

function Tile({
  kind,
  count,
  pulsing,
}: {
  kind: ActionKind;
  count: number;
  pulsing: boolean;
}) {
  return (
    <button
      className={[
        "stat-tile",
        `tile-${kind}`,
        count === 0 ? "zero" : "",
        pulsing ? "tile-pulse" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={() => void invoke("show_main_filtered", { bucket: kind })}
      title={`${ACTION_META[kind].label} — open CORA filtered to these`}
    >
      <span className="tile-count">{count}</span>
      <span className="tile-label">{ACTION_META[kind].short}</span>
    </button>
  );
}

const FLAG_LABEL: Record<string, string> = {
  "must-review": "must review",
  "follow-up": "follow up with author",
};

/** Teams-style day buckets: today, weekday names back through the week,
 *  then coarser blocks. */
function groupLabel(at: string): string {
  const d = new Date(at);
  const now = new Date();
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86_400_000);
  if (days <= 0) return "today";
  if (days < 7) return d.toLocaleDateString(undefined, { weekday: "long" }).toLowerCase();
  if (days < 14) return "last week";
  if (d.getMonth() === now.getMonth() && d.getFullYear() === now.getFullYear())
    return "this month";
  return "older";
}

function itemTime(at: string): string {
  const d = new Date(at);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return sameDay
    ? time
    : `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${time}`;
}

function Row({
  item,
  onOpen,
  onMenu,
}: {
  item: ActivityItem;
  onOpen: (item: ActivityItem) => void;
  onMenu: (e: React.MouseEvent, item: ActivityItem) => void;
}) {
  return (
    <button
      className={`feed-item${item.read ? "" : " unread"}${item.important ? " important" : ""}`}
      onClick={() => onOpen(item)}
      onContextMenu={(e) => onMenu(e, item)}
      title={`${item.repo}#${item.number} — ${item.prTitle}\nclick to open · right-click to flag`}
    >
      <span className="feed-dot" />
      <span className="feed-body">
        <span className="feed-line">
          {item.actor && <span className="feed-actor">@{item.actor}</span>}{" "}
          <span className="feed-summary">{item.summary}</span>
        </span>
        <span className="feed-meta mono">
          {item.repo.split("/")[1] ?? item.repo}#{item.number}
          {item.flag && <span className={`feed-flag ${item.flag}`}>{FLAG_LABEL[item.flag]}</span>}
        </span>
      </span>
      <span className="feed-time mono">{itemTime(item.at)}</span>
    </button>
  );
}

export function CalloutApp() {
  const { prs, pollStatus, recentlyChanged, init } = usePrStore();
  const [items, setItems] = useState<ActivityItem[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number; item: ActivityItem } | null>(null);

  const refresh = () => void ipc.getActivity().then(setItems).catch(() => {});

  useEffect(() => {
    document.body.classList.add("callout");
    void init();
    refresh();
    const un = listen("activity:changed", refresh);
    return () => void un.then((fn) => fn());
  }, [init]);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("blur", close);
    };
  }, [menu]);

  const buckets = useMemo(() => {
    const counts = new Map<ActionKind, TrackedPr[]>();
    for (const kind of ACTION_ORDER) {
      counts.set(kind, prs.filter((pr) => inBucket(pr, kind)));
    }
    return counts;
  }, [prs]);

  const attention = new Set(
    ACTION_ORDER.filter((k) => k !== "new" && k !== "comments").flatMap((k) =>
      (buckets.get(k) ?? []).map((p) => p.id),
    ),
  ).size;

  // Featured: anything you flagged, plus unread activity on important PRs
  // (your own PRs, high-priority repos/PRs).
  const featured = items.filter((i) => i.flag !== "" || (i.important && !i.read));
  const featuredIds = new Set(featured.map((i) => i.id));
  const rest = items.filter((i) => !featuredIds.has(i.id));
  const groups: [string, ActivityItem[]][] = [];
  for (const item of rest) {
    const label = groupLabel(item.at);
    const last = groups[groups.length - 1];
    if (last && last[0] === label) last[1].push(item);
    else groups.push([label, [item]]);
  }
  const unreadCount = items.filter((i) => !i.read).length;

  const openItem = (item: ActivityItem) => {
    void ipc.markActivityRead([item.id], true);
    void ipc.showMainWindow(item.prId);
    if (item.commentId) {
      void emit("focus:comment", { prId: item.prId, commentId: item.commentId });
    }
  };

  const flagItem = (item: ActivityItem, flag: string) => {
    void ipc.setActivityFlag(item.id, flag);
    setMenu(null);
  };

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
        {unreadCount > 0 && (
          <button
            className="icon-btn"
            title={`Mark all ${unreadCount} read`}
            onClick={() => void ipc.markActivityRead([], true)}
          >
            ✓
          </button>
        )}
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
                pulsing={(buckets.get(kind) ?? []).some((p) => recentlyChanged.has(p.id))}
              />
            ))}
          </div>

          <div className="activity-feed">
            {items.length === 0 && (
              <div className="callout-empty">
                <span>Activity on your PRs will land here.</span>
              </div>
            )}
            {featured.length > 0 && (
              <>
                <div className="feed-group eyebrow featured-label">★ featured</div>
                {featured.map((item) => (
                  <Row key={item.id} item={item} onOpen={openItem} onMenu={(e, it) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, item: it });
                  }} />
                ))}
              </>
            )}
            {groups.map(([label, groupItems]) => (
              <div key={label + groupItems[0]?.id}>
                <div className="feed-group eyebrow">{label}</div>
                {groupItems.map((item) => (
                  <Row key={item.id} item={item} onOpen={openItem} onMenu={(e, it) => {
                    e.preventDefault();
                    setMenu({ x: e.clientX, y: e.clientY, item: it });
                  }} />
                ))}
              </div>
            ))}
          </div>

          {menu && (
            <div className="feed-menu" style={{ left: menu.x, top: menu.y }}>
              {Object.entries(FLAG_LABEL).map(([flag, label]) => (
                <button
                  key={flag}
                  className={menu.item.flag === flag ? "on" : ""}
                  onClick={() => flagItem(menu.item, menu.item.flag === flag ? "" : flag)}
                >
                  ⚑ {label}
                </button>
              ))}
              {menu.item.flag && (
                <button onClick={() => flagItem(menu.item, "")}>clear flag</button>
              )}
              <button
                onClick={() => {
                  void ipc.markActivityRead([menu.item.id], !menu.item.read);
                  setMenu(null);
                }}
              >
                mark {menu.item.read ? "unread" : "read"}
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
