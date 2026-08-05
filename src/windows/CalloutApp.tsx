import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ActivityItem } from "../bindings/ActivityItem";
import type { TrackedPr } from "../bindings/TrackedPr";
import { ACTION_META, ACTION_ORDER, inBucket, type ActionKind } from "../lib/actions";
import { FLAG_LABEL } from "../lib/flags";
import { useRichTip } from "../components/Tooltip";
import {
  IconAlertTriangle,
  IconChat,
  IconCheckCircle,
  IconEllipsis,
  IconEye,
  IconGitCommit,
  IconGitMerge,
  IconGitPr,
  IconRefresh,
  IconSparkle,
  IconXCircle,
} from "../components/icons";
import { ContextMenu } from "../components/ContextMenu";
import { ipc } from "../lib/ipc";
import { setThemeOrg } from "../lib/theme";
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
      data-tip={`${ACTION_META[kind].label} — open CORA filtered to these`}
    >
      <span className="tile-count">{count}</span>
      <span className="tile-label">{ACTION_META[kind].short}</span>
    </button>
  );
}

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
  return new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** The verb as a glyph. Review verdicts split by outcome; everything else
 *  maps straight from the activity kind. */
function verbIcon(item: ActivityItem): { el: React.ReactElement; cls: string } {
  switch (item.kind) {
    case "comment":
      return { el: <IconChat />, cls: "neutral" };
    case "commits":
      return { el: <IconGitCommit />, cls: "neutral" };
    case "ci":
      return { el: <IconAlertTriangle />, cls: "bad" };
    case "merged":
      return { el: <IconGitMerge />, cls: "merged" };
    case "closed":
      return { el: <IconXCircle />, cls: "bad" };
    case "reopened":
      return { el: <IconRefresh />, cls: "ok" };
    case "new":
      return { el: <IconEye />, cls: "chat" };
    case "ready":
      return { el: <IconGitPr />, cls: "ok" };
    case "review":
      if (item.summary === "review approved") return { el: <IconCheckCircle />, cls: "ok" };
      if (item.summary === "changes requested") return { el: <IconXCircle />, cls: "bad" };
      return { el: <IconEye />, cls: "neutral" };
    case "analysis":
      return { el: <IconSparkle />, cls: "chat" };
    default:
      return { el: <IconEllipsis />, cls: "neutral" };
  }
}

/** Icon carries the verb; only words with information of their own remain
 *  (comment snippets, analysis-ready notices). */
function lineText(item: ActivityItem): string {
  if (item.kind === "comment") return item.summary.replace(/^commented:\s*/, "");
  if (item.kind === "analysis") return item.summary;
  return "";
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
  const icon = verbIcon(item);
  const tip = useRichTip<HTMLButtonElement>(
    <>
      <div className="tooltip-head">
        {item.repo}#{item.number}
      </div>
      <div>
        {item.actor ? `@${item.actor} ` : ""}
        {item.summary}
      </div>
      <div style={{ color: "var(--muted)" }}>{item.prTitle}</div>
      <div className="tooltip-hint" style={{ marginTop: 4 }}>
        click to open · right-click to flag
      </div>
    </>,
  );
  return (
    <button
      {...tip}
      className={`feed-item${item.read ? "" : " unread"}${item.important ? " important" : ""}`}
      onClick={() => onOpen(item)}
      onContextMenu={(e) => onMenu(e, item)}
    >
      <span className="feed-dot" />
      <span className="feed-time mono">{itemTime(item.at)}</span>
      <span className={`feed-icon ${icon.cls}`}>{icon.el}</span>
      <span className="feed-main">
        {item.flag && <span className={`feed-flag ${item.flag}`}>⚑</span>}
        <span className="feed-repo mono">
          {item.repo.split("/")[1] ?? item.repo}#{item.number}
        </span>
        {lineText(item) && <span className="feed-text">{lineText(item)}</span>}
      </span>
      <span className="feed-actor">{item.actor ? `@${item.actor}` : ""}</span>
    </button>
  );
}

/** Canvas fireworks over the callout — the "your analysis is ready" moment
 *  deserves more than a feed row. Staggered bursts in the theme accents,
 *  gravity and fade, gone in ~3 seconds. */
function Fireworks({ onDone }: { onDone: () => void }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const done = useRef(onDone);
  done.current = onDone;

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    ctx.scale(dpr, dpr);

    const css = getComputedStyle(document.documentElement);
    const palette = ["--ok", "--warn", "--chat", "--merge"].map(
      (v) => css.getPropertyValue(v).trim() || "#e8c268",
    );

    interface Spark {
      x: number;
      y: number;
      vx: number;
      vy: number;
      life: number;
      max: number;
      color: string;
      size: number;
    }
    let sparks: Spark[] = [];
    const burst = (x: number, y: number, color: string) => {
      const n = 34;
      for (let i = 0; i < n; i++) {
        const a = (Math.PI * 2 * i) / n + Math.random() * 0.25;
        const speed = 1.1 + Math.random() * 2.7;
        sparks.push({
          x,
          y,
          vx: Math.cos(a) * speed,
          vy: Math.sin(a) * speed - 0.4,
          life: 0,
          max: 55 + Math.random() * 30,
          color,
          size: 1 + Math.random() * 1.6,
        });
      }
    };
    const timers = [0, 300, 620, 950].map((t, i) =>
      setTimeout(
        () =>
          burst(
            w * (0.15 + Math.random() * 0.7),
            h * (0.15 + Math.random() * 0.45),
            palette[i % palette.length],
          ),
        t,
      ),
    );

    let raf = 0;
    let frames = 0;
    const tick = () => {
      frames++;
      ctx.clearRect(0, 0, w, h);
      sparks = sparks.filter((s) => s.life < s.max);
      for (const s of sparks) {
        s.life++;
        s.x += s.vx;
        s.y += s.vy;
        s.vy += 0.05;
        s.vx *= 0.985;
        s.vy *= 0.985;
        const t = 1 - s.life / s.max;
        ctx.globalAlpha = Math.max(0, t);
        ctx.fillStyle = s.color;
        ctx.beginPath();
        ctx.arc(s.x, s.y, s.size * (0.5 + t), 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.globalAlpha = 1;
      if (frames < 300 && (sparks.length > 0 || frames < 70)) {
        raf = requestAnimationFrame(tick);
      } else {
        done.current();
      }
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      timers.forEach(clearTimeout);
    };
  }, []);

  return <canvas ref={ref} className="fireworks-canvas" />;
}

export function CalloutApp() {
  const { prs, pollStatus, recentlyChanged, init } = usePrStore();
  const [items, setItems] = useState<ActivityItem[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number; item: ActivityItem } | null>(null);
  const [celebrating, setCelebrating] = useState(false);
  const [orgState, setOrgState] = useState<import("../bindings/OrgState").OrgState | null>(null);
  const [orgMenuAt, setOrgMenuAt] = useState<{ x: number; y: number } | null>(null);
  // Which "analysis ready" rows we've already seen — a genuinely new one
  // launches the fireworks. Seeded silently on first load so reopening the
  // callout doesn't re-celebrate old news.
  const seenAnalysis = useRef<Set<number> | null>(null);

  const refresh = () => void ipc.getActivity().then(setItems).catch(() => {});

  useEffect(() => {
    const fresh = items.filter((i) => i.kind === "analysis" && !i.read).map((i) => i.id);
    if (seenAnalysis.current == null) {
      seenAnalysis.current = new Set(fresh);
      return;
    }
    const news = fresh.filter((id) => !seenAnalysis.current!.has(id));
    if (news.length === 0) return;
    for (const id of news) seenAnalysis.current.add(id);
    if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setCelebrating(true);
    }
  }, [items]);

  useEffect(() => {
    document.body.classList.add("callout");
    void init();
    refresh();
    const refreshOrg = () =>
      void ipc
        .getOrgState()
        .then((s) => {
          setOrgState(s);
          setThemeOrg(s.active); // each org keeps its own theme
        })
        .catch(() => {});
    refreshOrg();
    const un = listen("activity:changed", () => {
      refresh();
      refreshOrg(); // unread badges track the feed
    });
    // Org switch (from any window): the feed and tiles become the new org's.
    const unOrg = listen("org:changed", () => {
      seenAnalysis.current = null; // don't celebrate the new org's backlog
      refresh();
      refreshOrg();
    });
    return () => {
      void un.then((fn) => fn());
      void unOrg.then((fn) => fn());
    };
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
      {celebrating && <Fireworks onDone={() => setCelebrating(false)} />}
      <header className="callout-header" data-tauri-drag-region>
        <span className={`sync-dot ${syncClass}`} data-tip={pollStatus?.message ?? "syncing"} aria-label={pollStatus?.message ?? "syncing"} />
        <span className="title" data-tauri-drag-region>
          CORA
        </span>
        <span className="spacer" data-tauri-drag-region />
        {unreadCount > 0 && (
          <button
            className="icon-btn"
            data-tip={`Mark all ${unreadCount} read`}
            aria-label={`Mark all ${unreadCount} read`}
            onClick={() => void ipc.markActivityRead([], true)}
          >
            ✓
          </button>
        )}
        <button className="icon-btn" data-tip="Refresh now" aria-label="Refresh now" onClick={() => void ipc.pollNow()}>
          ⟳
        </button>
        <button className="icon-btn" data-tip="Open CORA" aria-label="Open CORA" onClick={() => void ipc.showMainWindow()}>
          ⌂
        </button>
        <button
          className="icon-btn"
          data-tip="Hide callout"
          aria-label="Hide callout"
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
          {orgState && orgState.enabled.length > 1 && (
            <>
              <button
                className="org-select org-bar"
                data-tip="Active organization — the feed and tiles below are this org's"
                aria-haspopup="menu"
                aria-expanded={orgMenuAt != null}
                onClick={(e) => {
                  const r = e.currentTarget.getBoundingClientRect();
                  setOrgMenuAt({ x: r.left, y: r.bottom + 4 });
                }}
              >
                {orgState.active}
                {(orgState.unread[orgState.active] ?? 0) > 0
                  ? ` · ${orgState.unread[orgState.active]}`
                  : ""}
              </button>
              {orgMenuAt && (
                <ContextMenu
                  x={orgMenuAt.x}
                  y={orgMenuAt.y}
                  onClose={() => setOrgMenuAt(null)}
                  sections={[
                    {
                      title: "organizations",
                      items: orgState.enabled.map((o) => ({
                        label:
                          (orgState.unread[o] ?? 0) > 0 ? `${o} · ${orgState.unread[o]}` : o,
                        checked: o === orgState.active,
                        onClick: () => void ipc.setActiveOrg(o),
                      })),
                    },
                  ]}
                />
              )}
            </>
          )}
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
