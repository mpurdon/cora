import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { tip } from "../components/Tooltip";
import type { CodeFinding } from "../bindings/CodeFinding";
import type { PrSource } from "../bindings/PrSource";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { TrackedPr } from "../bindings/TrackedPr";
import type { AnalysisLevel } from "../bindings/AnalysisLevel";
import type { C4Node } from "../bindings/C4Node";
import type { C4NodeKind } from "../bindings/C4NodeKind";
import { AssistantPanel } from "../components/analysis/AssistantPanel";
import { formatTokens } from "../components/analysis/TraceSteps";
import { AssessmentView } from "../components/analysis/AssessmentView";
import { PrDescription } from "../components/analysis/PrDescription";
import { AwsAuthCard } from "../components/analysis/AwsAuthCard";
import { C4Canvas } from "../components/analysis/C4Canvas";
import { resolveFindingFile } from "../components/analysis/DiffPeek";
import { DiffView, fileDigest, parseDiffCached, type DiffFile } from "../components/analysis/DiffView";
import { CopyButton } from "../components/CopyButton";
import { codeSkeleton, componentSkeleton, filterGraph } from "../components/analysis/skeleton";
import type { C4Graph } from "../bindings/C4Graph";
import { HistoryView } from "../components/analysis/HistoryView";
import type { ActivityItem } from "../bindings/ActivityItem";
import type { MarkViewedEvent } from "../bindings/MarkViewedEvent";
import { StatusStrip, UnreadMarker } from "../components/StatusStrip";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ACTION_META, inBucket, type ActionKind } from "../lib/actions";
import type { PrReviews } from "../bindings/PrReviews";
import type { PrConversation } from "../bindings/PrConversation";
import { CommentsView } from "../components/analysis/CommentsView";
import { ContextMenu } from "../components/ContextMenu";
import { HistoryDrawer } from "../components/HistoryDrawer";
import { PrioritySelector } from "../components/PrioritySelector";
import { RepoSettingsDrawer } from "../components/RepoSettingsDrawer";
import type { Settings } from "../bindings/Settings";
import {
  IconChat,
  IconClipboard,
  IconEllipsis,
  IconExternal,
  IconLink,
  IconRefresh,
} from "../components/icons";
import { PrFilesRail } from "../components/PrFilesRail";
import { events, ipc, onFocusPr } from "../lib/ipc";
import { flagRank, FLAG_LABEL } from "../lib/flags";
import { usePersisted, usePersistedFlag } from "../lib/persisted";
import {
  findingSeed,
  isFindingCommented,
  requestChangesSeed,
  resolveApproveMessage,
  viewerComments,
} from "../lib/comments";
import { setThemeOrg } from "../lib/theme";
import { useChatStore } from "../state/chatStore";
import { analysisKey, useAnalysisStore } from "../state/analysisStore";
import { useDiffStore } from "../state/diffStore";
import { ciTone, isFinished, mergeTone, parseTitle, reviewTone, timeAgo, usePrStore } from "../state/prStore";
import {
  initReviewStore,
  lockedReview,
  useReviewStore,
  withPending,
} from "../state/reviewStore";
import { SettingsView, type SettingsPane } from "./SettingsView";
import {
  PR_PRIORITY_DISPLAY_ORDER,
  PR_PRIORITY_ICON,
  PR_PRIORITY_LABEL,
  PR_PRIORITY_WEIGHT,
  REPO_PRIORITY_DISPLAY_ORDER,
  REPO_PRIORITY_ICON,
  REPO_PRIORITY_LABEL,
  REPO_PRIORITY_TOOLTIP,
  REPO_PRIORITY_WEIGHT,
} from "../lib/priority";

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

/** Opening a persistently-critical PR is itself the acknowledgment. */
function openPr(pr: TrackedPr) {
  if (pr.needsAttention) void ipc.acknowledgeCriticalPr(pr.id);
  void openUrl(pr.url);
}
/** "Ready for my review" requirements, one per lamp. */
type ReadyFilters = { ciPass: boolean; reviewNeeded: boolean; noConflicts: boolean };
const NO_READY_FILTERS: ReadyFilters = { ciPass: false, reviewNeeded: false, noConflicts: false };

function passesReady(pr: TrackedPr, f: ReadyFilters): boolean {
  // A finished PR is not a review candidate, so the readiness lamps say nothing
  // about it — asking it to look review-ready is how a closed PR with reviewers
  // still pending slips through "needs review". Once revealed, it stays.
  if (isFinished(pr)) return true;
  // ciPass admits "no checks configured" (idle) — nothing is blocking.
  if (f.ciPass && !["ok", "idle"].includes(ciTone(pr))) return false;
  // reviewNeeded keeps only PRs that still need YOUR decision. reviewTone is
  // the aggregate (REVIEW_REQUIRED while any requested reviewer hasn't weighed
  // in), so on its own it keeps a PR you've already approved when a co-reviewer
  // still owes one — hence the second clause: a standing decision of yours,
  // not re-requested, no longer needs you.
  if (f.reviewNeeded) {
    if (reviewTone(pr) !== "warn") return false;
    const iDecided =
      (pr.myReviewState === "APPROVED" || pr.myReviewState === "CHANGES_REQUESTED") &&
      !pr.myReviewRerequested;
    if (iDecided) return false;
  }
  // noConflicts drops only known-conflicting; UNKNOWN is GitHub still computing.
  if (f.noConflicts && mergeTone(pr) === "bad") return false;
  return true;
}

const orgOf = (repo: string) => repo.split("/")[0] ?? repo;
const shortRepo = (repo: string) => repo.split("/")[1] ?? repo;

function reasonOf(pr: TrackedPr): PrSource {
  return REASONS.find((g) => pr.sources.includes(g.key))?.key ?? "watched-repo";
}

/** You already reviewed (approved / requested changes) and nothing has
 *  happened since — the ball is in someone else's court, hide by default. */
function reviewedAndIdle(pr: TrackedPr): boolean {
  if (isFinished(pr)) return false;
  if (pr.myReviewRerequested) return false;
  if (!pr.myReviewState || !pr.myReviewedAt) return false;
  if (pr.myReviewState !== "APPROVED" && pr.myReviewState !== "CHANGES_REQUESTED") return false;
  // Gate on CORA's change detection (commits, comments, CI, review state),
  // not GitHub's updatedAt — your own housekeeping (resolving threads,
  // labels) bumps updatedAt but shouldn't resurface a reviewed PR. The
  // review itself registers as a change — allow slack for that echo.
  return Date.parse(pr.lastChangeAt) - Date.parse(pr.myReviewedAt) < 2 * 60_000;
}

/** Routine dependency/action/toolchain bumps — low-risk housekeeping that
 *  shouldn't outrank real changes in the attention sort. */
function isRoutineBump(pr: TrackedPr): boolean {
  if (pr.author.endsWith("[bot]")) {
    return /\b(bump|update|upgrade)\b/i.test(pr.title);
  }
  return /\b(bump|update|upgrade)s?\b.*\b(to|from)\s+v?\d+(\.\d+)*/i.test(pr.title);
}

/** Higher = needs the engineer sooner. */
function attentionScore(pr: TrackedPr): number {
  let score = pr.unread.length * 10;
  if (isRoutineBump(pr)) score -= 4;
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

/** Pointer-driven panel width: clamped while dragging, persisted on release. */
function useResizableWidth(
  key: string,
  min: number,
  max: number,
  fallback: number,
  dir: 1 | -1, // +1: dragging right widens (left panel); -1: dragging left widens (right panel)
) {
  const clamp = (w: number) => Math.min(max, Math.max(min, w));
  const [width, setWidth] = useState(() => clamp(Number(localStorage.getItem(key)) || fallback));
  // Window-level listeners for the drag, and pure deltas in the pointer's own
  // coordinate space. Under CSS zoom, clientX is in visual px while widths are
  // layout px — measuring the resizer's own rect-vs-offset ratio gives the
  // conversion no matter which convention the webview uses.
  const start = (e: React.PointerEvent) => {
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    const scale = el.offsetWidth > 0 ? el.getBoundingClientRect().width / el.offsetWidth : 1;
    const startX = e.clientX;
    const startW = width;
    let latest: number | null = null;
    const onMove = (ev: PointerEvent) => {
      latest = clamp(startW + (dir * (ev.clientX - startX)) / scale);
      setWidth(latest);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      document.body.classList.remove("col-resizing");
      if (latest != null) localStorage.setItem(key, String(latest));
    };
    document.body.classList.add("col-resizing");
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };
  return { width, start };
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
        data-tip="Dismiss"
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

/** Requested reviewers + latest review states — visible before analyzing.
 *  Your own verdict is the actions row's job ("✓ you approved"), so it never
 *  repeats here under your login. The one exception is a review you can still
 *  move — then the strip carries it, labelled "you", and the actions row shows
 *  the buttons instead. */
function ReviewStrip({ reviews }: { reviews: PrReviews | null }) {
  if (!reviews) return null;
  const me = reviews.viewerLogin;
  const locked = lockedReview(reviews) != null;
  const iOweAReview = reviews.requested.includes(me);
  const others = reviews.reviews.filter((r) => r.author !== me);
  const mine = locked || iOweAReview ? undefined : reviews.reviews.find((r) => r.author === me);
  const requested = reviews.requested.filter((who) => who !== me || !locked);
  if (others.length === 0 && !mine && requested.length === 0) return null;

  const glyph = (state: string) =>
    state === "APPROVED" ? "✓" : state === "CHANGES_REQUESTED" ? "±" : "💬";

  return (
    <div className="review-strip">
      {(mine ? [mine, ...others] : others).map((r) => (
        <span key={r.author} className={`review-chip state-${r.state.toLowerCase()}`}>
          <span className="review-glyph">{glyph(r.state)}</span>
          {r.author === me ? "you" : r.author}
          <span className="review-state">{r.state.toLowerCase().replace(/_/g, " ")}</span>
        </span>
      ))}
      {requested.map((who) => (
        <span key={who} className="review-chip state-requested">
          <span className="review-glyph">…</span>
          {who === me ? "you" : who}
          <span className="review-state">requested</span>
        </span>
      ))}
    </div>
  );
}

/** Approve / request changes, with the review comment GitHub expects.
 *  Locks after you've reviewed, until the PR moves: new commits, your review
 *  re-requested, or (for changes-requested) all threads resolved. */
/** ReviewActions and PrControls each own a pending flow (approve composer,
 *  confirm-merge/close). Only one may be open at a time — Detail holds the
 *  active owner; each component stands down when the other takes it. */
type FlowOwner = "review" | "controls" | null;

function ReviewActions({
  pr,
  reviews,
  flow,
  setFlow,
}: {
  pr: TrackedPr;
  reviews: PrReviews | null;
  flow: FlowOwner;
  setFlow: (f: FlowOwner) => void;
}) {
  const [mode, setMode] = useState<"approve" | "request-changes" | null>(null);
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The live conversation drives the auto-seeded request-changes summary.
  const [conversation, setConversation] = useState<PrConversation | null>(null);

  // A half-written review for one PR must not follow you to the next.
  useEffect(() => {
    setMode(null);
    setBody("");
    setError(null);
  }, [pr.id]);

  useEffect(() => {
    const load = () =>
      void ipc.getPrComments(pr.id).then(setConversation).catch(() => setConversation(null));
    load();
    const un = listen("reviews:changed", load);
    return () => void un.then((fn) => fn());
  }, [pr.id]);

  useEffect(() => {
    if (flow !== "review") setMode(null);
  }, [flow]);
  const openMode = (m: "approve" | "request-changes") => {
    setFlow("review");
    setMode(m);
    // Both verdicts open with a one-sentence summary written from the live
    // conversation — a pointer to your comments when requesting changes, what
    // your review settled when approving. Only when the box is still empty:
    // never clobber text you've typed.
    if (body.trim()) return;
    const viewer = reviews?.viewerLogin ?? "";
    if (m === "request-changes") {
      const seed = requestChangesSeed(conversation, viewer);
      if (seed) setBody(seed);
      return;
    }
    // Approve's seed can be a repo/global override, so it's resolved against
    // freshly-fetched settings rather than a value threaded through props —
    // that way an edit made in the repo settings drawer takes effect on the
    // very next Approve click, no refetch wiring needed.
    void ipc.getSettings().then((settings) => {
      const seed = resolveApproveMessage(pr.repo, settings, conversation, viewer);
      // Re-check at resolve time: the user may have started typing while
      // this was in flight.
      if (seed) setBody((current) => (current.trim() ? current : seed));
    });
  };

  if (isFinished(pr)) return null;

  const mine = lockedReview(reviews);
  if (mine) {
    const verb = mine.state === "APPROVED" ? "approved" : "requested changes";
    return (
      <span
        className={`review-chip state-${mine.state.toLowerCase()}`}
        data-tip="Re-enabled on new commits, resolved threads, or a re-requested review"
      >
        <span className="review-glyph">{mine.state === "APPROVED" ? "✓" : "±"}</span>
        you {verb}
      </span>
    );
  }

  const submit = async () => {
    if (!mode) return;
    setBusy(true);
    setError(null);
    try {
      // Hold the review GitHub just created: its own read-back lags the
      // mutation, and the header must show your verdict the moment it lands.
      // No refetch from here — submit_review's own refresh emits
      // reviews:changed, which Detail already listens for.
      const verdict = await ipc.submitReview(pr.id, mode, body);
      useReviewStore.getState().record(pr.id, verdict);
      setMode(null);
      setBody("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (mode) {
    return (
      <div className="review-composer">
        <textarea
          autoFocus
          placeholder={
            mode === "approve"
              ? "Optional approval comment…"
              : "What needs to change? (required)"
          }
          value={body}
          disabled={busy}
          onChange={(e) => setBody(e.target.value)}
        />
        {error && <div className="settings-error">{error}</div>}
        <div className="row">
          <button
            className={`action-btn ${mode === "approve" ? "btn-ok" : "confirm-danger"}`}
            disabled={busy || (mode === "request-changes" && !body.trim())}
            onClick={() => void submit()}
          >
            {busy ? "Submitting…" : mode === "approve" ? "Submit approval" : "Submit request"}
          </button>
          <button className="action-btn btn-danger" disabled={busy} onClick={() => setMode(null)}>
            Cancel
          </button>
        </div>
      </div>
    );
  }

  // Approving over your own open questions is almost always a mistake —
  // resolve (or escalate to request-changes) first.
  const myOpen = reviews?.myOpenThreads ?? 0;
  return (
    <>
      <button
        className="action-btn btn-ok"
        disabled={myOpen > 0}
        data-tip={
          myOpen > 0
            ? `You have ${myOpen} unresolved review thread${myOpen === 1 ? "" : "s"} — resolve ${myOpen === 1 ? "it" : "them"} before approving. (Comments starting with praise:, note:, or fyi:, or marked (non-blocking), don't count.)`
            : undefined
        }
        onClick={() => openMode("approve")}
      >
        ✓ Approve
      </button>
      <button className="action-btn quiet-danger" onClick={() => openMode("request-changes")}>
        ± Request changes
      </button>
    </>
  );
}

/** Merge / close / reopen with a two-step confirm — no accidental merges.
 *  Close lives in the ⋯ overflow menu; a bump of `closeRequested` starts its
 *  confirm step here so the busy/error handling stays in one place. */
/** GitHub's button labels, keyed by our merge-method values. */
const MERGE_LABEL = {
  squash: "Squash and merge",
  merge: "Merge pull request",
  rebase: "Rebase and merge",
} as const;

function PrControls({
  pr,
  closeRequested,
  flow,
  setFlow,
}: {
  pr: TrackedPr;
  closeRequested: number;
  flow: FlowOwner;
  setFlow: (f: FlowOwner) => void;
}) {
  const [method, setMethod] = useState<"squash" | "merge" | "rebase">(
    () => (localStorage.getItem("cora.mergeMethod") as "squash" | "merge" | "rebase") ?? "squash",
  );
  const [confirming, setConfirming] = useState<"merge" | "close" | null>(null);
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Confirm flows are per-PR conversations — never carry one across a switch.
  useEffect(() => {
    setConfirming(null);
    setMenuAt(null);
    setError(null);
  }, [pr.id]);

  useEffect(() => {
    if (flow !== "controls") {
      setConfirming(null);
      setMenuAt(null);
    }
  }, [flow]);
  const openConfirm = (what: "merge" | "close") => {
    setFlow("controls");
    setConfirming(what);
  };

  useEffect(() => {
    if (closeRequested > 0 && !isFinished(pr)) {
      setFlow("controls");
      setConfirming("close");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [closeRequested, pr.state]);

  const act = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      setConfirming(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (pr.state === "MERGED") return null;

  return (
    <>
      {pr.state === "CLOSED" ? (
        <button
          className="action-btn"
          disabled={busy}
          onClick={() => void act(() => ipc.reopenPr(pr.id))}
        >
          Reopen
        </button>
      ) : confirming === "merge" ? (
        <>
          <button
            className="action-btn confirm-danger"
            disabled={busy}
            onClick={() => void act(() => ipc.mergePr(pr.id, method))}
          >
            {busy ? "Merging…" : `Confirm ${method} merge`}
          </button>
          <button
            className="action-btn btn-danger"
            disabled={busy}
            onClick={() => setConfirming(null)}
          >
            Cancel
          </button>
        </>
      ) : confirming === "close" ? (
        <>
          <button
            className="action-btn confirm-danger"
            disabled={busy}
            onClick={() => void act(() => ipc.closePr(pr.id))}
          >
            {busy ? "Closing…" : "Confirm close"}
          </button>
          <button
            className="action-btn btn-danger"
            disabled={busy}
            onClick={() => setConfirming(null)}
          >
            Cancel
          </button>
        </>
      ) : (
        <span className="merge-split">
          <button className="action-btn merge-btn" onClick={() => openConfirm("merge")}>
            {MERGE_LABEL[method]}
          </button>
          <button
            className="action-btn merge-caret"
            aria-haspopup="menu"
            aria-expanded={menuAt != null}
            data-tip="Choose merge method"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              setMenuAt({ x: r.left, y: r.bottom + 4 });
            }}
          >
            ▾
          </button>
          {menuAt && (
            <ContextMenu
              x={menuAt.x}
              y={menuAt.y}
              onClose={() => setMenuAt(null)}
              sections={[
                {
                  items: (Object.keys(MERGE_LABEL) as (keyof typeof MERGE_LABEL)[]).map((m) => ({
                    label: MERGE_LABEL[m],
                    checked: m === method,
                    onClick: () => {
                      setMethod(m);
                      localStorage.setItem("cora.mergeMethod", m);
                    },
                  })),
                },
              ]}
            />
          )}
        </span>
      )}
      {error && <span className="settings-error">{error}</span>}
    </>
  );
}

/** Manual single-PR data refresh (status, checks, comments count). */
function RefreshPrButton({ prId }: { prId: string }) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      className="icon-btn"
      data-tip="Refresh this PR from GitHub"
      aria-label="Refresh this PR from GitHub"
      disabled={busy}
      onClick={() => {
        setBusy(true);
        void ipc.refreshPr(prId).finally(() => setBusy(false));
      }}
    >
      <span className={busy ? "glyph-spin" : undefined}>
        <IconRefresh />
      </span>
    </button>
  );
}

function TrackPrInput({
  onDone,
  onTracked,
}: {
  onDone: () => void;
  onTracked: (id: string) => void;
}) {
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const track = async () => {
    try {
      const pr = await ipc.trackPrUrl(url);
      onDone();
      // A hand-entered PR is the one you want to look at — open it.
      onTracked(pr.id);
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

type Tab = "description" | "assessment" | "c4" | "diff" | "comments" | "history";
/** A row of the shortcut sheet. `run` present = the app handles the key;
 *  absent = documentation for something the mouse or the OS owns. */
interface Shortcut {
  keys: string;
  label: string;
}

/** A shortcut the app actually handles. Both halves are required, so the
 *  keydown loop never has to assert that a row it matched can also run. */
interface KeyShortcut extends Shortcut {
  match: (e: KeyboardEvent) => boolean;
  run: (e: KeyboardEvent) => void;
}

/** Things the keyboard doesn't own — the mouse and the OS do — listed
 *  because the sheet is where people look for them regardless. */
const DOC_SHORTCUTS: Shortcut[] = [
  { keys: "esc", label: "close menus, drawers, this help" },
  { keys: "⌘ + / − / 0", label: "zoom in / out / reset" },
  { keys: "right-click a PR", label: "analyze, PR/author/repo priority, mute, untrack" },
  { keys: "click a node", label: "drill into the architecture · diff at code level" },
];

/** Bumping this re-announces the sheet once, for everyone, on next start. */
const HOTKEYS_SEEN = "cora.hotkeysSeen.v2";

const TAB_ORDER: [Tab, string][] = [
  ["description", "Description"],
  ["assessment", "Assessment"],
  ["c4", "Architecture"],
  ["diff", "Diff"],
  ["comments", "Comments"],
  ["history", "History"],
];

/** C4 drill state: which UI level we're viewing and the path down to it.
 *  The four C4 rungs map to two kinds of frame: "container" is free
 *  navigation (it re-scopes the context result client-side — drilling a
 *  system never costs a model run), while component and code are scoped
 *  agentic runs that enrich an instant diff-derived sketch. */
type UiLevel = "context" | "container" | "component" | "code";

interface DrillFrame {
  level: UiLevel;
  focus?: string;
  label: string;
  /** The clicked node — scopes the sketch graph while the run enriches. */
  node?: C4Node;
}

const ROOT_FRAME: DrillFrame = { level: "context", label: "System" };
const UI_DEPTH: Record<UiLevel, number> = { context: 0, container: 1, component: 2, code: 3 };
/** The analysis level backing each UI level. */
const ANALYSIS_OF: Record<UiLevel, AnalysisLevel> = {
  context: "context",
  container: "context",
  component: "component",
  code: "code",
};

function nextDrillLevel(kind: C4NodeKind, current: UiLevel): UiLevel | null {
  const next: UiLevel | null =
    kind === "system"
      ? "container"
      : kind === "container"
        ? "component"
        : kind === "component"
          ? "code"
          : null;
  if (!next || UI_DEPTH[next] <= UI_DEPTH[current]) return null;
  return next;
}

/** The full analysis trace while a run is live — fills the panel and stays
 *  pinned to the newest step, so a long run reads as progress, not a stall. */
function ProgressLog({ steps }: { steps: { message: string }[] }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [steps.length]);
  return (
    <div className="progress-log" ref={ref}>
      {steps.map((p, i) => (
        <div key={i} className={i === steps.length - 1 ? "current" : ""}>
          {p.message}
        </div>
      ))}
    </div>
  );
}

type AnalysisPanelProps = {
  pr: TrackedPr;
  tab: "assessment" | "c4";
  highlight: string[];
  onFocusNodes: (ids: string[]) => void;
};

/** Assessment + C4 tabs share one analysis run per (PR head, drill frame). */
function AnalysisPanel({ pr, tab, highlight, onFocusNodes }: AnalysisPanelProps) {
  const { runs, init, ensure, start } = useAnalysisStore();
  const [stack, setStack] = useState<DrillFrame[]>([ROOT_FRAME]);

  useEffect(() => setStack([ROOT_FRAME]), [pr.id]);

  // Which findings you've already commented on, derived from the live PR
  // conversation so a finding settles to "commented" once you leave a review
  // comment at its location — durable across reloads, reflects GitHub-side
  // comments too. Refetches on any comment/review change.
  const [conversation, setConversation] = useState<PrConversation | null>(null);
  const [viewerLogin, setViewerLogin] = useState("");
  useEffect(() => {
    const load = () =>
      void ipc.getPrComments(pr.id).then(setConversation).catch(() => setConversation(null));
    load();
    void ipc
      .getPrReviews(pr.id)
      .then((r) => setViewerLogin(r.viewerLogin))
      .catch(() => {});
    const un = listen("reviews:changed", load);
    return () => void un.then((fn) => fn());
  }, [pr.id]);
  const mine = useMemo(
    () => viewerComments(conversation, viewerLogin),
    [conversation, viewerLogin],
  );

  const frame = stack[stack.length - 1];
  const aLevel = ANALYSIS_OF[frame.level];
  // The container view reuses the root context run — no focus of its own.
  const aFocus = frame.level === "container" ? undefined : frame.focus;
  const run = runs[analysisKey(pr.id, aLevel, aFocus)];
  const contextRun = runs[analysisKey(pr.id, "context")];
  const contextResult = contextRun?.status === "done" ? contextRun.result : undefined;

  useEffect(() => {
    // Drilled levels auto-run on arrival; the root level waits for the user.
    void init().then(() => ensure(pr.id, aLevel, aFocus, stack.length > 1));
  }, [pr.id, pr.headSha, aLevel, aFocus, stack.length, init, ensure]);

  // Entering a system pre-warms its changed containers' component runs, so
  // the next drill usually lands on a finished graph.
  useEffect(() => {
    if (frame.level !== "container" || !frame.focus || !contextResult) return;
    for (const n of contextResult.graph.nodes) {
      if (n.kind === "container" && n.boundary === frame.focus && n.change !== "unchanged") {
        void ensure(pr.id, "component", n.id, true);
      }
    }
  }, [frame, contextResult, pr.id, ensure]);

  // Diff file list feeds the instant sketch graphs at component/code depth.
  const [diffFiles, setDiffFiles] = useState<DiffFile[]>([]);
  useEffect(() => {
    if (UI_DEPTH[frame.level] < 2) return;
    let live = true;
    void useDiffStore
      .getState()
      .ensure(pr.id, pr.headSha)
      .then(() => {
        if (!live) return;
        const entry = useDiffStore.getState().entries[pr.id];
        setDiffFiles(entry?.raw ? parseDiffCached(entry.raw) : []);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [pr.id, pr.headSha, frame.level]);

  // Instant stand-in while the scoped run works (or after it fails): the
  // drilled node's diff files grouped into modules. No edges — those are
  // what the analysis adds.
  const sketch = useMemo(() => {
    if (!contextResult || !frame.node) return null;
    if (frame.level === "component") return componentSkeleton(frame.node, contextResult, diffFiles);
    if (frame.level === "code") return codeSkeleton(frame.node, contextResult, diffFiles);
    return null;
  }, [contextResult, frame, diffFiles]);

  const [pendingDrill, setPendingDrill] = useState<C4Node | null>(null);

  const pushDrill = (node: C4Node, next: UiLevel) => {
    setPendingDrill(null);
    setStack((s) => [...s, { level: next, focus: node.id, label: node.name, node }]);
  };

  const drillInto = (node: C4Node) => {
    const next = nextDrillLevel(node.kind, frame.level);
    if (!next) return;
    // The container view is free navigation; deeper levels cost an agentic
    // run — drilling an untouched node for context only should be deliberate.
    if (next !== "container" && node.change === "unchanged") {
      setPendingDrill(node);
      return;
    }
    pushDrill(node, next);
  };

  const canvasFor = (graph: C4Graph) => (
    <C4Canvas
      graph={graph}
      highlightIds={highlight}
      isDrillable={(node) => nextDrillLevel(node.kind, frame.level) !== null}
      onDrill={drillInto}
      onPeek={(node) => {
        // Diffs only surface at the code level — higher levels are for
        // navigating the architecture, not reading patches. People and
        // external systems have nothing to peek at anywhere. The peek lives
        // in the assistant panel, so it goes through the store.
        if (frame.level !== "code") return;
        if (node.kind === "person" || node.kind === "external-system") return;
        useDiffStore.getState().openPeek(pr.id, node);
      }}
    />
  );

  const LEVEL_NAME: Record<UiLevel, string> = {
    context: "context",
    container: "container",
    component: "component",
    code: "code",
  };
  const crumbs = (
    <nav className="drill-crumbs" aria-label="C4 level">
      {stack.map((f, i) => {
        const current = i === stack.length - 1;
        return (
          <span key={i} className="crumb-wrap">
            {i > 0 && <span className="crumb-sep">›</span>}
            <button
              className={`crumb${current ? " current" : ""}`}
              disabled={current}
              data-tip={current ? undefined : `Back up to ${f.label}`}
              onClick={() => setStack(stack.slice(0, i + 1))}
            >
              <span className={`crumb-level level-${f.level}`}>{LEVEL_NAME[f.level]}</span>
              <span className="crumb-name">{f.label}</span>
            </button>
          </span>
        );
      })}
    </nav>
  );

  const retry = () => void start(pr.id, aLevel, aFocus);

  // "± comment" on a finding: anchor a pre-filled composer in the diff.
  // A PR-level conversation comment is the last resort, only when nothing
  // in the finding resolves to a changed file.
  const commentFinding = async (seed: string, nodeIds: string[]) => {
    const store = useDiffStore.getState();
    // The Assessment tab can be open before the diff was ever fetched —
    // resolve against the real file list, not an empty one.
    await store.ensure(pr.id, pr.headSha).catch(() => {});
    const entry = useDiffStore.getState().entries[pr.id];
    const files = entry?.raw ? parseDiffCached(entry.raw) : [];
    const nodes = (run?.result?.graph.nodes ?? []).filter((n) => nodeIds.includes(n.id));
    const path = resolveFindingFile(seed, nodes, files);
    const { requestCompose } = useDiffStore.getState();
    if (path) {
      requestCompose({ target: "diff", path, seed });
      window.dispatchEvent(new CustomEvent("cora:set-tab", { detail: "diff" }));
    } else {
      requestCompose({ target: "conversation", path: null, seed });
      window.dispatchEvent(new CustomEvent("cora:set-tab", { detail: "comments" }));
    }
  };

  // Code findings carry their own path + line — no resolution needed.
  const commentCode = (f: CodeFinding) => {
    useDiffStore
      .getState()
      .requestCompose({ target: "diff", path: f.path, line: f.line, seed: findingSeed(f) });
    window.dispatchEvent(new CustomEvent("cora:set-tab", { detail: "diff" }));
  };

  // Hand a finding to the assistant chat. The store request opens the panel
  // (MainApp) and sends the seeded prompt (AssistantPanel) — no tab switch, so
  // the reviewer keeps the findings in view alongside the explanation.
  const explainCode = (f: CodeFinding) => {
    useDiffStore.getState().requestExplain(pr.id, f);
  };

  if (!run || run.status === "idle") {
    // A drilled frame lands here for the beat between the click and
    // ensure() flipping the run to running — show the sketch (or at least
    // a live pulse) instead of flashing the analyze placeholder.
    if (stack.length > 1) {
      return (
        <>
          <div className="panel-meta">
            {crumbs}
            <span className="spacer" />
          </div>
          <div className="sketch-strip">
            <span className="sync-dot live" />
            <span className="sketch-label">starting analysis…</span>
          </div>
          {tab === "c4" && sketch ? (
            canvasFor(sketch)
          ) : (
            <div className="canvas-loading">preparing…</div>
          )}
        </>
      );
    }
    return (
      <div className="placeholder">
        <p>
          CORA reads the diff and explores the repository to place this change in the
          architecture — external-boundary effects first.
        </p>
        <button className="action-btn analyze-btn" onClick={retry}>
          Analyze architecture
        </button>
      </div>
    );
  }

  if (run.status === "running") {
    // Drilled C4 frames show the instant diff-derived sketch — the run
    // enriches it in the background and swaps in when it lands.
    if (tab === "c4" && sketch) {
      const last = run.progress[run.progress.length - 1];
      return (
        <>
          <div className="panel-meta">
            {crumbs}
            <span className="spacer" />
          </div>
          <div className="sketch-strip">
            <span className="sync-dot live" />
            <span className="sketch-label">
              sketch — modules from the diff; relationships arrive with the analysis
            </span>
            <span className="mono sketch-progress">{last?.message}</span>
          </div>
          {canvasFor(sketch)}
        </>
      );
    }
    return (
      <>
        <div className="panel-meta">
          {crumbs}
          <span className="spacer" />
        </div>
        <ProgressLog steps={run.progress} />
      </>
    );
  }

  if (run.status === "error") {
    if (run.errorKind === "aws-auth") {
      return <AwsAuthCard detail={run.error ?? ""} onSignedIn={retry} />;
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
            <button className="action-btn" onClick={retry}>
              Retry
            </button>
          </div>
        </div>
      );
    }
    // A failed drill degrades to the sketch, not a dead end.
    if (tab === "c4" && sketch) {
      return (
        <>
          <div className="panel-meta">
            {crumbs}
            <span className="spacer" />
          </div>
          <div className="sketch-strip error" data-tip={run.error}>
            <span className="lamp bad" />
            <span className="sketch-label">
              analysis failed — showing the diff-derived sketch
            </span>
            <button className="action-btn" onClick={retry}>
              Retry
            </button>
          </div>
          {canvasFor(sketch)}
        </>
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
          <button className="action-btn" onClick={retry}>
            Retry
          </button>
          {stack.length > 1 && (
            <button className="action-btn" onClick={() => setStack(stack.slice(0, -1))}>
              Back up
            </button>
          )}
        </div>
      </div>
    );
  }

  const result = run.result!;

  // Seeing a current-head analysis counts as having examined the new commits.
  // (Hook order is safe: this render path always reaches here when done.)
  if (result.headSha === pr.headSha && pr.unread.includes("new-commits")) {
    void ipc.markPrReadKinds(pr.id, ["new-commits"]);
  }

  if (result.headSha !== pr.headSha) {
    // Stale — new commits landed since this analysis.
    return (
      <div className="placeholder">
        <p>New commits landed since the last analysis.</p>
        <button className="action-btn" onClick={retry}>
          Re-analyze
        </button>
      </div>
    );
  }

  return (
    <>
      <div className="panel-meta">
        {crumbs}
        <span className="mono panel-usage">
          analyzed {timeAgo(result.createdAt)} ago
          {result.usage.turns > 0 && (
            <>
              {" "}
              · {result.usage.turns} turns · {formatTokens(result.usage.inputTokens)} in /{" "}
              {formatTokens(result.usage.outputTokens)} out
            </>
          )}
        </span>
        <span className="spacer" />
        <button className="action-btn" data-tip="Run the analysis again from scratch" onClick={retry}>
          Re-analyze
        </button>
      </div>
      {tab === "assessment" ? (
        <AssessmentView
          assessment={result.assessment}
          codeFindings={result.codeFindings}
          codePass={result.codePass}
          onFocusNodes={onFocusNodes}
          onCommentFinding={commentFinding}
          onCommentCode={commentCode}
          onExplainCode={explainCode}
          isCommented={(f) => isFindingCommented(f, mine)}
        />
      ) : (
        <>
          <div className="drill-hint eyebrow">
            {frame.level === "code"
              ? "click a node to see its diff"
              : "click a node to drill in"}
          </div>
          {canvasFor(
            frame.level === "container" && frame.focus
              ? filterGraph(result.graph, frame.focus)
              : result.graph,
          )}
          {pendingDrill && (
            <div className="drill-confirm">
              <p>
                <span className="mono">{pendingDrill.name}</span> isn't modified by this PR —
                analyses focus on the diff. Drill in anyway for context? (full agentic run)
              </p>
              <div className="row">
                <button
                  className="action-btn"
                  onClick={() => {
                    const next = nextDrillLevel(pendingDrill.kind, frame.level);
                    if (next) pushDrill(pendingDrill, next);
                  }}
                >
                  Analyze anyway
                </button>
                <a
                  className="action-btn"
                  href={`${pr.url.split("/pull/")[0]}/tree/${pr.headSha}`}
                  target="_blank"
                  rel="noreferrer"
                >
                  Browse source ↗
                </a>
                <button className="action-btn" onClick={() => setPendingDrill(null)}>
                  Cancel
                </button>
              </div>
            </div>
          )}
        </>
      )}
    </>
  );
}

function Detail({
  pr,
  tab,
  onTab: setTab,
  pendingComment,
  onPendingCommentHandled,
  assistantOpen,
  onToggleAssistant,
}: {
  pr: TrackedPr;
  tab: Tab;
  onTab: (t: Tab) => void;
  pendingComment: string | null;
  onPendingCommentHandled: () => void;
  assistantOpen: boolean;
  onToggleAssistant: () => void;
}) {
  const [highlight, setHighlight] = useState<string[]>([]);
  const [reviewBump, setReviewBump] = useState(0);
  const [reviews, setReviews] = useState<PrReviews | null>(null);
  // ⋯ overflow menu (housekeeping actions) + its Close-PR confirm trigger.
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const [closeRequested, setCloseRequested] = useState(0);
  // Which pending flow (approve composer / confirm-merge-close) is open —
  // holding it here keeps the two mutually exclusive.
  const [flow, setFlow] = useState<FlowOwner>(null);
  const { type, clean } = parseTitle(pr.title);

  useEffect(() => setFlow(null), [pr.id]);

  useEffect(() => setReviews(null), [pr.id]);
  useEffect(() => {
    // Refetches keep the last answer on screen until the new one lands —
    // blanking it would flash the Approve buttons back over your own verdict.
    let live = true;
    void ipc
      .getPrReviews(pr.id)
      .then((r) => live && setReviews(r))
      .catch(() => live && setReviews(null));
    return () => {
      live = false;
    };
  }, [pr.id, pr.headSha, reviewBump]);
  // Anything that moves thread/review state (resolving a thread, posting a
  // diff comment, a refresh) re-checks the approve gate.
  useEffect(() => {
    const un = listen("reviews:changed", () => setReviewBump((n) => n + 1));
    return () => void un.then((fn) => fn());
  }, []);
  // Your just-submitted verdict, shown until GitHub's read-back catches up.
  const pendingVerdict = useReviewStore((s) => s.pending[pr.id]);
  const shownReviews = useMemo(
    () => withPending(reviews, pendingVerdict),
    [reviews, pendingVerdict],
  );
  const focusNodes = (ids: string[]) => {
    setHighlight(ids);
    if (ids.length > 0) setTab("c4");
  };

  // A newly opened PR always starts at the Assessment.
  useEffect(() => {
    setTab("assessment");
    setHighlight([]);
  }, [pr.id]);

  // Reply-notification deep link: jump to the Comments tab, then the
  // CommentsView scrolls to the anchored comment.
  useEffect(() => {
    if (pendingComment) setTab("comments");
  }, [pendingComment]);

  // Engagement-based acknowledgment: reading comments clears new-comments,
  // examining the diff clears new-commits.
  useEffect(() => {
    if (tab === "comments" && pr.unread.includes("new-comments")) {
      void ipc.markPrReadKinds(pr.id, ["new-comments"]);
    }
    if (tab === "diff" && pr.unread.includes("new-commits")) {
      void ipc.markPrReadKinds(pr.id, ["new-commits"]);
    }
  }, [tab, pr.id, pr.unread]);

  // Number-key tab switching, dispatched from the window-level hotkeys.
  useEffect(() => {
    const onTabHotkey = (e: Event) => setTab((e as CustomEvent<Tab>).detail);
    window.addEventListener("cora:set-tab", onTabHotkey);
    return () => window.removeEventListener("cora:set-tab", onTabHotkey);
  }, []);

  return (
    <div className="detail">
      <div className="crumbs">
        <span className="eyebrow">
          {pr.repo} · #{pr.number} · by {pr.author} · updated {timeAgo(pr.updatedAt)} ago
        </span>
      </div>
      <h1>
        <StatusStrip pr={pr} variant="title" />
        <span className="title-text">
          {pr.isDraft ? "Draft: " : ""}
          {clean}
        </span>
      </h1>
      <div className="facts">
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
        <ReviewActions pr={pr} reviews={shownReviews} flow={flow} setFlow={setFlow} />
        <PrControls pr={pr} closeRequested={closeRequested} flow={flow} setFlow={setFlow} />
        <span className="spacer" />
        <RefreshPrButton prId={pr.id} />
        <CopyButton text={pr.url} what="the PR link" icon={<IconLink />} />
        <button className="icon-btn" data-tip="Open on GitHub" aria-label="Open on GitHub" onClick={() => openPr(pr)}>
          <IconExternal />
        </button>
        <button
          className="icon-btn"
          data-tip="More actions"
          aria-label="More actions"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setMenuAt({ x: r.right, y: r.bottom + 4 });
          }}
        >
          <IconEllipsis />
        </button>
        <button
          className={`icon-btn${assistantOpen ? " active" : ""}`}
          data-tip="Assistant — chat about this PR (a)"
          aria-label="Assistant — chat about this PR (a)"
          onClick={onToggleAssistant}
        >
          <IconChat />
        </button>
      </div>
      {menuAt && (
        <ContextMenu
          x={menuAt.x}
          y={menuAt.y}
          align="right"
          onClose={() => setMenuAt(null)}
          sections={[
            {
              items: [
                {
                  label: pr.muted ? "Unmute" : "Mute",
                  onClick: () => void ipc.setPrMuted(pr.id, !pr.muted),
                },
                { label: "Mark read", onClick: () => void ipc.markPrRead(pr.id) },
                {
                  label: "Untrack",
                  danger: true,
                  onClick: () => void ipc.untrackPr(pr.id),
                },
                ...(!isFinished(pr)
                  ? [
                      {
                        label: "Close pull request…",
                        danger: true,
                        onClick: () => setCloseRequested((n) => n + 1),
                      },
                    ]
                  : []),
              ],
            },
          ]}
        />
      )}
      <ReviewStrip reviews={shownReviews} />

      <div className="tabs" role="tablist">
        {TAB_ORDER.map(([key, label], i) => (
          <button
            key={key}
            role="tab"
            aria-selected={tab === key}
            className={`tab${tab === key ? " active" : ""}`}
            data-tip={`Hotkey: ${i + 1}`}
            onClick={() => setTab(key)}
          >
            {label}
          </button>
        ))}
      </div>

      {tab === "description" && <PrDescription body={pr.body} />}
      {(tab === "assessment" || tab === "c4") && (
        <AnalysisPanel pr={pr} tab={tab} highlight={highlight} onFocusNodes={focusNodes} />
      )}
      {tab === "diff" && <DiffView prId={pr.id} headSha={pr.headSha} />}
      {tab === "history" && <HistoryView prId={pr.id} headSha={pr.headSha} />}
      {tab === "comments" && (
        <CommentsView
          prId={pr.id}
          focusCommentId={pendingComment}
          onFocusHandled={onPendingCommentHandled}
        />
      )}
    </div>
  );
}

export function MainApp() {
  const { prs, pollStatus, init } = usePrStore();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // IDE-style master/detail rail: the PR list is the "project list"; picking
  // a PR focuses the rail on that PR's changed files until you go back.
  const [railView, setRailView] = useState<"list" | "files">("list");
  const [assistantOpen, , setAssistant] = usePersistedFlag("cora.assistantOpen");
  // "Explain" on a finding raises a store request; open the panel so
  // AssistantPanel can pick it up, send the prompt, and show the chat.
  const explainRequest = useDiffStore((s) => s.explainRequest);
  useEffect(() => {
    if (explainRequest) setAssistant(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [explainRequest]);
  // A code peek (a node clicked on the Architecture tab) shows in the panel,
  // so it opens the panel. Remember whether the panel was closed at the time:
  // dropping the peek then closes it again — the panel is left as it was found.
  const peek = useDiffStore((s) => s.peek);
  const closePeek = useDiffStore((s) => s.closePeek);
  const panelBeforePeek = useRef<boolean | null>(null);
  useEffect(() => {
    if (peek) {
      if (panelBeforePeek.current === null) panelBeforePeek.current = assistantOpen;
      setAssistant(true);
    } else {
      if (panelBeforePeek.current === false) setAssistant(false);
      panelBeforePeek.current = null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [peek]);
  // Closing the panel by any route (✕, the `a` key) drops the peek with it,
  // and a peek belongs to the PR it was opened on.
  useEffect(() => {
    if (!assistantOpen) closePeek();
  }, [assistantOpen, closePeek]);
  useEffect(() => closePeek(), [selectedId, closePeek]);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsPane, setSettingsPane] = useState<SettingsPane>("general");
  const [showTrackInput, setShowTrackInput] = useState(false);
  // Lifted from Detail so the assistant panel knows when the Diff tab is
  // active (its File-insights view only makes sense against the diff).
  const [tab, setTab] = useState<Tab>("assessment");

  const openSettings = (pane: SettingsPane = "general") => {
    setSettingsPane(pane);
    setShowSettings(true);
  };

  // Drives the empty-state copy: "select a PR" vs "connect GitHub".
  const [patPresent, setPatPresent] = useState<boolean | null>(null);
  useEffect(() => {
    void ipc
      .githubPatPresent()
      .then(setPatPresent)
      .catch(() => setPatPresent(null));
  }, [showSettings]);

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

  const [showMuted, toggleMuted] = usePersistedFlag("cora.showMuted");
  const [showReviewed, toggleReviewed] = usePersistedFlag("cora.showReviewed");

  const [showFinished, toggleFinished] = usePersistedFlag("cora.showFinished");

  const [priorities, setPriorities] = useState<Record<string, RepoPriority>>({});
  const [authorPriorities, setAuthorPriorities] = useState<Record<string, RepoPriority>>({});
  const [watchedRepos, setWatchedRepos] = useState<string[]>([]);
  useEffect(() => {
    void ipc.getSettings().then((s) => {
      setPriorities(s.repoPriorities);
      setAuthorPriorities(s.authorPriorities);
      setWatchedRepos(s.watchedRepos);
    });
  }, [showSettings]); // re-read after the settings page closes

  const toggleWatchRepo = async (repo: string) => {
    const s = await ipc.getSettings();
    const watched = s.watchedRepos.includes(repo)
      ? s.watchedRepos.filter((r) => r !== repo)
      : [...s.watchedRepos, repo].sort();
    await ipc.setSettings({ ...s, watchedRepos: watched });
    setWatchedRepos(watched);
  };
  const prioOf = useCallback(
    (repo: string): RepoPriority => priorities[repo] ?? "standard",
    [priorities],
  );
  const setRepoPriority = async (repo: string, p: RepoPriority) => {
    await ipc.setRepoPriority(repo, p); // audited server-side
    const s = await ipc.getSettings();
    setPriorities(s.repoPriorities);
  };
  const authorPrioOf = useCallback(
    (author: string): RepoPriority => authorPriorities[author] ?? "standard",
    [authorPriorities],
  );
  const setAuthorPriority = async (author: string, p: RepoPriority) => {
    await ipc.setAuthorPriority(author, p); // audited server-side
    const s = await ipc.getSettings();
    setAuthorPriorities(s.authorPriorities);
  };
  const [collapsed, setCollapsed] = useState<Set<string>>(
    () => new Set(JSON.parse(localStorage.getItem("cora.collapsed") ?? "[]") as string[]),
  );

  const rail = useResizableWidth("cora.railWidth", 220, 520, 300, 1);
  const assistant = useResizableWidth("cora.assistantWidth", 300, 900, 400, -1);
  const filterRef = useRef<HTMLInputElement>(null);
  const [menu, setMenu] = useState<
    | { x: number; y: number; kind: "pr"; pr: TrackedPr; analyzed?: boolean }
    | { x: number; y: number; kind: "repo"; repo: string }
    | null
  >(null);
  const [showHotkeys, setShowHotkeys] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [repoSettingsRepo, setRepoSettingsRepo] = useState<string | null>(null);
  const [repoSettings, setRepoSettings] = useState<Settings | null>(null);
  useEffect(() => {
    if (!repoSettingsRepo) return;
    setRepoSettings(null);
    void ipc.getSettings().then(setRepoSettings);
  }, [repoSettingsRepo]);
  const saveRepoSettings = async (patch: Partial<Settings>) => {
    if (!repoSettings) return;
    const next = { ...repoSettings, ...patch };
    setRepoSettings(next);
    await ipc.setSettings(next);
  };
  const [pendingComment, setPendingComment] = useState<string | null>(null);
  const [bucketFilter, setBucketFilter] = useState<ActionKind | null>(null);
  const [orgState, setOrgState] = useState<import("../bindings/OrgState").OrgState | null>(null);
  const [availableOrgs, setAvailableOrgs] = useState<
    import("../bindings/GithubOrg").GithubOrg[]
  >([]);
  const [orgMenuAt, setOrgMenuAt] = useState<{ x: number; y: number } | null>(null);

  const refreshOrgState = useCallback(
    () =>
      void ipc
        .getOrgState()
        .then((s) => {
          setOrgState(s);
          setThemeOrg(s.active); // each org keeps its own theme
        })
        .catch(() => {}),
    [],
  );

  useEffect(() => {
    // Every org the PAT can see — the corner selector offers not-yet-enabled
    // ones under "enable…" so switching never requires a Settings trip.
    void ipc.listGithubOrgs().then(setAvailableOrgs).catch(() => {});
  }, []);

  const chooseOrg = useCallback(
    async (login: string) => {
      const state = await ipc.getOrgState().catch(() => null);
      if (!state) return;
      try {
        if (!state.enabled.includes(login)) {
          await ipc.setEnabledOrgs([...state.enabled, login]);
        }
        if (state.active !== login) {
          await ipc.setActiveOrg(login);
        }
      } catch (e) {
        console.error("org switch failed", e);
      }
      refreshOrgState();
    },
    [refreshOrgState],
  );

  useEffect(() => {
    void init();
    initReviewStore();
    refreshOrgState();
    // Org switch (from any window): hard-reset every PR-keyed cache so no
    // data from the previous org lingers, then refetch as the new org.
    const unlistenOrg = listen<string>(events.orgChanged, () => {
      setSelectedId(null);
      usePrStore.getState().reset();
      useAnalysisStore.getState().reset();
      useChatStore.getState().reset();
      useDiffStore.getState().reset();
      void init();
      refreshOrgState();
    });
    const openKinds: import("../bindings/ChangeKind").ChangeKind[] = [
      "new",
      "title-changed",
      "draft-changed",
      "reopened",
      "merged",
      "closed",
      "ci-changed",
      "review-changed",
    ];
    const unlisten = onFocusPr((id) => {
      setShowSettings(false);
      setSelectedId(id);
      setRailView("files");
      void ipc.markPrReadKinds(id, openKinds);
    });
    // Future reply notifications deep-link straight to a comment.
    const unlistenComment = listen<{ prId: string; commentId: string }>(
      "focus:comment",
      (e) => {
        setShowSettings(false);
        setSelectedId(e.payload.prId);
        setRailView("files");
        setPendingComment(e.payload.commentId);
        void ipc.markPrReadKinds(e.payload.prId, openKinds);
      },
    );
    // Callout tile double-click: land here filtered to that bucket.
    const unlistenBucket = listen<ActionKind>("focus:bucket", (e) => {
      setShowSettings(false);
      setBucketFilter(e.payload);
    });
    // Assistant marks files viewed: the frontend owns diff parsing and
    // digests, so apply through the same path as the manual checkbox.
    const unlistenViewed = listen<MarkViewedEvent>("viewed:mark", async (e) => {
      const { prId, paths, all, viewed } = e.payload;
      const pr = usePrStore.getState().prs.find((p) => p.id === prId);
      if (!pr) return;
      const diff = useDiffStore.getState();
      await diff.ensure(prId, pr.headSha).catch(() => {});
      const entry = useDiffStore.getState().entries[prId];
      if (!entry?.raw) return;
      const files = parseDiffCached(entry.raw);
      const wanted = new Set(paths);
      for (const f of files) {
        if (all || wanted.has(f.path)) {
          useDiffStore.getState().setViewed(prId, f.path, fileDigest(f), viewed);
        }
      }
    });
    // Notification deep-link: macOS gives no click callback, so the first
    // focus after a notification claims the pending target (2 min window).
    const claimPending = () =>
      void ipc.takePendingFocus().then((target) => {
        if (!target) return;
        setShowSettings(false);
        setSelectedId(target.prId);
        setRailView("files");
        if (target.commentId) setPendingComment(target.commentId);
        void ipc.markPrReadKinds(target.prId, openKinds);
      });
    const unlistenFocus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) claimPending();
    });
    return () => {
      void unlisten.then((fn) => fn());
      void unlistenComment.then((fn) => fn());
      void unlistenBucket.then((fn) => fn());
      void unlistenViewed.then((fn) => fn());
      void unlistenFocus.then((fn) => fn());
      void unlistenOrg.then((fn) => fn());
    };
  }, [init, refreshOrgState]);

  const toggleGroup = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      localStorage.setItem("cora.collapsed", JSON.stringify([...next]));
      return next;
    });
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
    const unignored = textMatched.filter(
      (pr) =>
        prioOf(pr.repo) !== "ignored" &&
        authorPrioOf(pr.author) !== "ignored" &&
        (showMuted || !pr.muted) &&
        (showReviewed || !reviewedAndIdle(pr)) &&
        (showFinished || !isFinished(pr)),
    );
    const bucketMatched = bucketFilter
      ? unignored.filter((pr) => inBucket(pr, bucketFilter))
      : unignored;
    const visible = bucketMatched.filter((pr) => passesReady(pr, ready));
    const hiddenByReady = unignored.length - visible.length;
    // A PR carries both pulls, the repo's and the author's, summed rather than
    // the repo deciding with the author as a tiebreak. Ranking an author up is
    // worth exactly what ranking a repo down costs, so a name you want to see
    // draws level with work from a repo you'd otherwise reach for first. Then
    // per-PR priority, then the chosen sort. Both axes are offset against
    // Ordering: repo priority, then the author's priority within the group,
    // then per-PR priority, then the chosen sort.
    const sorted = [...visible].sort(
      (a, b) =>
        REPO_PRIORITY_WEIGHT[prioOf(b.repo)] - REPO_PRIORITY_WEIGHT[prioOf(a.repo)] ||
        REPO_PRIORITY_WEIGHT[authorPrioOf(b.author)] - REPO_PRIORITY_WEIGHT[authorPrioOf(a.author)] ||
        PR_PRIORITY_WEIGHT[b.priority] - PR_PRIORITY_WEIGHT[a.priority] ||
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
    const prWeight = (p: TrackedPr) =>
      REPO_PRIORITY_WEIGHT[prioOf(p.repo)] + REPO_PRIORITY_WEIGHT[authorPrioOf(p.author)];
    const groupWeight = (g: { prs: TrackedPr[] }) => Math.max(...g.prs.map(prWeight));
    if (groupMode === "reason") {
      entries.sort(
        (a, b) =>
          REASONS.findIndex((r) => r.key === a.key) - REASONS.findIndex((r) => r.key === b.key),
      );
    } else if (groupMode === "type") {
      // known types alphabetical, "unknown" last
      entries.sort(
        (a, b) =>
          groupWeight(b) - groupWeight(a) ||
          (a.key === "unknown" ? 1 : b.key === "unknown" ? -1 : a.label.localeCompare(b.label)),
      );
    } else {
      entries.sort((a, b) => groupWeight(b) - groupWeight(a) || a.label.localeCompare(b.label));
    }
    return { grouped: entries, hiddenByReady };
  }, [prs, filter, sortMode, groupMode, ready, prioOf, authorPrioOf, bucketFilter, showMuted, showReviewed, showFinished]);

  const selected = prs.find((p) => p.id === selectedId) ?? null;

  // Opening a PR only acknowledges what the header makes visible at a glance.
  // new-comments clears when you read the Comments tab; new-commits when you
  // examine the diff or a current analysis — engagement, not selection.
  const select = (id: string) => {
    setShowSettings(false);
    setSelectedId(id);
    setRailView("files");
    void ipc.markPrReadKinds(id, [
      "new",
      "title-changed",
      "draft-changed",
      "reopened",
      "merged",
      "closed",
      "ci-changed",
      "review-changed",
    ]);
  };

  // Flags live on activity rows, but they are a worklist rather than a page of
  // the feed — `getFlagged` returns every flagged row however old, so one can't
  // fall off the end of the callout's display limit. They also ignore every
  // rail filter: a PR you marked "must review" must not vanish because it's
  // finished, muted, or outside the ready chips.
  const [flaggedRows, setFlaggedRows] = useState<ActivityItem[]>([]);
  useEffect(() => {
    const load = () => void ipc.getFlagged().then(setFlaggedRows).catch(() => {});
    load();
    const un = listen(events.activityChanged, load);
    return () => void un.then((f) => f());
  }, []);

  const flagged = useMemo(() => {
    const byId = new Map(prs.map((p) => [p.id, p]));
    const byPr = new Map<string, { pr: TrackedPr; flags: Set<string>; rank: number }>();
    for (const item of flaggedRows) {
      const pr = byId.get(item.prId);
      if (!pr) continue; // untracked and aged out — nothing left to link to
      const entry = byPr.get(item.prId) ?? { pr, flags: new Set<string>(), rank: Infinity };
      entry.flags.add(item.flag);
      entry.rank = Math.min(entry.rank, flagRank(item.flag));
      byPr.set(item.prId, entry);
    }
    return [...byPr.values()].sort(
      (a, b) =>
        a.rank - b.rank ||
        a.pr.repo.localeCompare(b.pr.repo) ||
        a.pr.number - b.pr.number,
    );
  }, [flaggedRows, prs]);

  // A file click in the focused rail lands on that file in the Diff tab.
  const openFile = (path: string) => {
    const store = useDiffStore.getState();
    store.requestFocusFile(path);
    store.setVisiblePath(path); // highlight immediately; scroll spy takes over
    window.dispatchEvent(new CustomEvent("cora:set-tab", { detail: "diff" }));
  };


  // Visible PRs in display order, for j/k navigation.
  const flatVisible = useMemo(
    () =>
      grouped.flatMap((g) =>
        collapsed.has(`${groupMode}:${g.key}`) ? [] : g.prs,
      ),
    [grouped, collapsed, groupMode],
  );

  // One table drives both the handler and the help sheet, so a shortcut can't
  // exist without being documented (or be documented after it's gone). Rows
  // with no `run` are things the keyboard doesn't own — right-click, the
  // window zoom the OS handles — listed because the help is where people look.
  const shortcuts: KeyShortcut[] = useMemo(
    () => [
      {
        keys: "j / k",
        label: "next / previous pull request",
        match: (e) => e.key === "j" || e.key === "k",
        run: (e) => {
          if (flatVisible.length === 0) return;
          const idx = flatVisible.findIndex((p) => p.id === selectedId);
          const next =
            e.key === "j"
              ? Math.min(flatVisible.length - 1, idx + 1)
              : Math.max(0, idx <= 0 ? 0 : idx - 1);
          select(flatVisible[idx === -1 ? 0 : next].id);
        },
      },
      {
        keys: `1 – ${TAB_ORDER.length}`,
        label: TAB_ORDER.map(([, label]) => label).join(" · "),
        match: (e) => {
          const i = Number(e.key) - 1;
          return !!selected && Number.isInteger(i) && i >= 0 && i < TAB_ORDER.length;
        },
        run: (e) => {
          const tab = TAB_ORDER[Number(e.key) - 1][0];
          window.dispatchEvent(new CustomEvent("cora:set-tab", { detail: tab }));
        },
      },
      {
        keys: "a",
        label: "toggle the assistant panel",
        match: (e) => e.key === "a" && !!selected,
        run: () => setAssistant(!assistantOpen),
      },
      {
        keys: "A",
        label: "analyze this PR (shift, so it isn't a slip)",
        match: (e) => e.key === "A" && !!selected,
        run: () => selected && void ipc.runAnalysis(selected.id, "context"),
      },
      {
        keys: "r",
        label: "refresh this PR from GitHub",
        match: (e) => e.key === "r" && !!selected,
        run: () => selected && void ipc.refreshPr(selected.id).catch(() => {}),
      },
      {
        keys: "o",
        label: "open this PR on GitHub",
        match: (e) => e.key === "o" && !!selected,
        run: () => selected && openPr(selected),
      },
      {
        keys: "y",
        label: "copy this PR's link",
        match: (e) => e.key === "y" && !!selected,
        run: () => selected && void navigator.clipboard.writeText(selected.url),
      },
      {
        keys: "/",
        label: "focus the filter",
        match: (e) => e.key === "/",
        run: () => filterRef.current?.focus(),
      },
      {
        keys: "h",
        label: "history — your actions, undoable",
        match: (e) => e.key === "h",
        run: () => setShowHistory(true),
      },
      {
        keys: ",",
        label: "settings",
        match: (e) => e.key === ",",
        run: () => openSettings(),
      },
      {
        keys: "?",
        label: "show or hide this help",
        match: (e) => e.key === "?",
        run: () => setShowHotkeys((s) => !s),
      },
    ],
    [flatVisible, selectedId, selected, assistantOpen, openSettings],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      const typing =
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.tagName === "SELECT" ||
        target.isContentEditable;
      if (e.key === "Escape") {
        setShowHotkeys(false);
        setMenu(null);
        if (typing) (target as HTMLInputElement).blur();
        return;
      }
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return;

      const hit = shortcuts.find((s) => s.match(e));
      if (hit) {
        hit.run(e);
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shortcuts]);

  // Announce the shortcuts once, the first time this build runs — a help
  // sheet nobody knows about may as well not exist. Bump the key when the
  // list changes enough to be worth re-announcing.
  useEffect(() => {
    if (localStorage.getItem(HOTKEYS_SEEN) === "1") return;
    localStorage.setItem(HOTKEYS_SEEN, "1");
    setShowHotkeys(true);
  }, []);

  const orgBar = orgState && (
    <>
      <button
        className="org-select org-bar"
        data-tip="Active organization — data, settings, and feeds are per-org. Picking a new org enables it with its own isolated database."
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
                label: (orgState.unread[o] ?? 0) > 0 ? `${o} · ${orgState.unread[o]}` : o,
                checked: o === orgState.active,
                onClick: () => void chooseOrg(o),
              })),
            },
            ...(availableOrgs.some((o) => !orgState.enabled.includes(o.login))
              ? [
                  {
                    title: "enable…",
                    items: availableOrgs
                      .filter((o) => !orgState.enabled.includes(o.login))
                      .map((o) => ({
                        label: o.personal ? `${o.login} (personal)` : o.login,
                        onClick: () => void chooseOrg(o.login),
                      })),
                  },
                ]
              : []),
          ]}
        />
      )}
    </>
  );

  return (
    <div className="main-shell">
      <nav className="rail" style={{ width: rail.width }}>
        {selected && railView === "files" ? (
          <PrFilesRail pr={selected} onBack={() => setRailView("list")} onOpenFile={openFile} />
        ) : (
          <>
            <div className="rail-header">
              <span className="name">CORA</span>
              <span className="eyebrow">{prs.length} tracked</span>
              <span className="spacer" />
              <button
                className="icon-btn"
                data-tip="Track a PR by URL"
                aria-label="Track a PR by URL"
                onClick={() => setShowTrackInput((s) => !s)}
              >
                +
              </button>
            </div>
            {showTrackInput && (
              <TrackPrInput onDone={() => setShowTrackInput(false)} onTracked={select} />
            )}
            {orgBar}

            <div className="rail-controls">
              <input
                ref={filterRef}
                className="filter-input"
                placeholder="Filter…  ( / )"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
              />
              <select
                data-tip="Group by"
                value={groupMode}
                onChange={(e) => setGroupMode(e.target.value as GroupMode)}
              >
                <option value="org">by org</option>
                <option value="repo">by repo</option>
                <option value="type">by type</option>
                <option value="reason">by reason</option>
              </select>
              <select
                data-tip="Sort by"
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
                data-tip="Show only PRs whose CI checks pass (or that have no checks)"
                onClick={() => toggleReady("ciPass")}
              >
                <span className="lamp ok" /> ci passing
              </button>
              <button
                className={`chip${ready.reviewNeeded ? " on" : ""}`}
                data-tip="Show only PRs still awaiting a review decision"
                onClick={() => toggleReady("reviewNeeded")}
              >
                <span className="lamp warn" /> needs review
              </button>
              <button
                className={`chip${ready.noConflicts ? " on" : ""}`}
                data-tip="Hide PRs with merge conflicts"
                onClick={() => toggleReady("noConflicts")}
              >
                <span className="lamp bad" /> no conflicts
              </button>
              <button
                className={`chip${showMuted ? " on" : ""}`}
                data-tip="Muted PRs are hidden by default — toggle to see them (dimmed)"
                onClick={toggleMuted}
              >
                <span className="lamp" /> muted
              </button>
              <button
                className={`chip${showReviewed ? " on" : ""}`}
                data-tip="PRs you've approved or requested changes on, with nothing new since, are hidden — toggle to see them (dimmed). They return on new commits, comments, or a re-requested review."
                onClick={toggleReviewed}
              >
                <span className="lamp ok" /> reviewed
              </button>
              <button
                className={`chip${showFinished ? " on" : ""}`}
                data-tip="Closed and merged PRs are hidden by default — toggle to see them (dimmed). Reopen lives in the detail panel."
                onClick={toggleFinished}
              >
                <span className="lamp" /> finished
              </button>
              {hiddenByReady > 0 && <span className="hidden-note">−{hiddenByReady}</span>}
            </div>

            {bucketFilter && (
              <div className="bucket-filter-chip">
                <span className="eyebrow">{ACTION_META[bucketFilter].label}</span>
                <button className="icon-btn" data-tip="Clear filter" aria-label="Clear filter" onClick={() => setBucketFilter(null)}>
                  ✕
                </button>
              </div>
            )}

            <Legend />

            <div className="rail-list">
              {flagged.length > 0 && (
                <div className="flagged-rail">
                  <div className="eyebrow flagged-rail-head">
                    ⚑ flagged
                    <span className="group-count">{flagged.length}</span>
                  </div>
                  {flagged.map(({ pr, flags }) => (
                    <button
                      key={pr.id}
                      className={`flagged-row${pr.id === selectedId ? " selected" : ""}`}
                      onClick={() => select(pr.id)}
                      data-tip={`${pr.repo}#${pr.number} — ${pr.title}`}
                    >
                      <span className="flagged-tags">
                        {[...flags].sort((a, b) => flagRank(a) - flagRank(b)).map((f) => (
                          <span key={f} className={`flag-tag ${f}`}>
                            {FLAG_LABEL[f] ?? f}
                          </span>
                        ))}
                      </span>
                      <span className="flagged-title">
                        <span className="flagged-num">#{pr.number}</span> {pr.title}
                      </span>
                      <span
                        className="flagged-clear"
                        role="button"
                        data-tip="Clear the flag — removes it from every feed row for this PR"
                        onClick={(e) => {
                          e.stopPropagation();
                          // The section is a worklist, so "done" means done with
                          // the PR, not with the one row you clicked.
                          void ipc.clearActivityFlags(pr.id);
                        }}
                      >
                        ✕
                      </span>
                    </button>
                  ))}
                </div>
              )}
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
                      onContextMenu={(e) => {
                        // Repo groups get the repo-priority menu; a group's PRs all
                        // share one repo in by-repo mode.
                        if (groupMode !== "repo") return;
                        e.preventDefault();
                        setMenu({ x: e.clientX, y: e.clientY, kind: "repo", repo: group.key });
                      }}
                      aria-expanded={!isCollapsed}
                    >
                      <span className="chevron">{isCollapsed ? "▸" : "▾"}</span>
                      <span className="eyebrow">{group.label}</span>
                      <span className="group-count">{group.prs.length}</span>
                      {repoPrio && repoPrio !== "standard" && (
                        <span className={`prio-tag ${repoPrio}`} data-tip={REPO_PRIORITY_TOOLTIP[repoPrio]}>
                          {REPO_PRIORITY_LABEL[repoPrio]}
                        </span>
                      )}
                      <span className="spacer" />
                      {unreadSum > 0 && (
                        <span
                          className="unread-count"
                          data-tip={`${unreadSum} unacknowledged update${unreadSum > 1 ? "s" : ""} in this group`}
                        >
                          {unreadSum}
                        </span>
                      )}
                    </button>
                    {!isCollapsed &&
                      group.prs.map((pr) => (
                        <button
                          key={pr.id}
                          className={`rail-row${pr.id === selectedId ? " selected" : ""}${pr.muted ? " muted-pr" : ""}${reviewedAndIdle(pr) ? " reviewed-idle" : ""}${isFinished(pr) ? " finished-pr" : ""}`}
                          onClick={() => select(pr.id)}
                          onContextMenu={(e) => {
                            e.preventDefault();
                            setMenu({ x: e.clientX, y: e.clientY, kind: "pr", pr });
                            // Cheap cache lookup decides whether the menu
                            // offers "Analyze" — only for un-analyzed heads.
                            void ipc.getAnalysis(pr.id, "context").then((cached) =>
                              setMenu((m) =>
                                m && m.kind === "pr" && m.pr.id === pr.id
                                  ? { ...m, analyzed: !!cached }
                                  : m,
                              ),
                            );
                          }}
                          data-tip={`${pr.repo}#${pr.number} — ${pr.title}`}
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
          </>
        )}
        <div className="rail-footer">
          <span
            className={`sync-dot ${pollStatus == null ? "" : pollStatus.ok ? "live" : "err"}`}
          />
          <span className="status-text">
            {pollStatus == null
              ? "starting…"
              : pollStatus.syncing
                ? "refreshing…"
                : pollStatus.ok
                  ? `synced ${timeAgo(pollStatus.at)} ago`
                  : (pollStatus.message ?? "sync failed")}
          </span>
          <button
            className="icon-btn"
            {...tip("Refresh from GitHub now")}
            onClick={() => void ipc.pollNow()}
          >
            <span className={pollStatus?.syncing ? "glyph-spin" : undefined}>
              <IconRefresh />
            </span>
          </button>
          <button
            className="icon-btn"
            {...tip("History — your actions, undoable")}
            onClick={() => setShowHistory(true)}
          >
            <IconClipboard />
          </button>
          <button
            className="icon-btn"
            {...tip("Toggle callout")}
            onClick={() => void ipc.toggleCallout()}
          >
            ▣
          </button>
          <button
            className="icon-btn"
            {...tip("Keyboard shortcuts  (?)")}
            onClick={() => setShowHotkeys((s) => !s)}
          >
            ⌨
          </button>
          <button
            className="icon-btn"
            {...tip("Settings  (,)")}
            onClick={() => (showSettings ? setShowSettings(false) : openSettings())}
          >
            ⚙
          </button>
        </div>
      </nav>
      <div
        className="rail-resizer"
        role="separator"
        aria-orientation="vertical"
        onPointerDown={rail.start}
      />

      <main className="content">
        {showSettings ? (
          <SettingsView onClose={() => setShowSettings(false)} initialPane={settingsPane} />
        ) : selected ? (
          <Detail
            pr={selected}
            tab={tab}
            onTab={setTab}
            pendingComment={pendingComment}
            onPendingCommentHandled={() => setPendingComment(null)}
            assistantOpen={assistantOpen}
            onToggleAssistant={() => setAssistant(!assistantOpen)}
          />
        ) : prs.length === 0 && pollStatus?.ok === false ? (
          <Onboarding onOpenSettings={() => openSettings("github")} />
        ) : patPresent === false ? (
          <div className="empty-detail">
            <p>Connect GitHub to start tracking pull requests.</p>
            <button className="action-btn auth-primary" onClick={() => openSettings("github")}>
              Open GitHub settings
            </button>
          </div>
        ) : (
          <div className="empty-detail">Select a pull request.</div>
        )}
      </main>

      {selected && !showSettings && assistantOpen && (
        <>
          <div
            className="assistant-resizer"
            role="separator"
            aria-orientation="vertical"
            onPointerDown={assistant.start}
          />
          <AssistantPanel
            pr={selected}
            width={assistant.width}
            insightsEnabled={tab === "diff"}
            onClose={() => setAssistant(false)}
          />
        </>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          sections={
            menu.kind === "pr"
              ? [
                  {
                    title: `PR #${menu.pr.number} priority`,
                    items: [
                      {
                        type: "custom",
                        key: "pr-priority",
                        render: () => (
                          <PrioritySelector
                            levels={PR_PRIORITY_DISPLAY_ORDER}
                            value={menu.pr.priority}
                            onChange={(p) => void ipc.setPrPriority(menu.pr.id, p)}
                            getLabel={(p) => PR_PRIORITY_LABEL[p]}
                            getIcon={(p) => PR_PRIORITY_ICON[p].icon}
                            groupLabel={`PR #${menu.pr.number} priority`}
                          />
                        ),
                      },
                    ],
                  },
                  {
                    items: [
                      ...(menu.analyzed === false
                        ? [
                            {
                              label: "Analyze architecture",
                              onClick: () => {
                                const s = useAnalysisStore.getState();
                                void s.init().then(() => s.ensure(menu.pr.id, "context", undefined, true));
                              },
                            },
                          ]
                        : []),
                      {
                        label: menu.pr.muted ? "Unmute" : "Mute",
                        onClick: () => void ipc.setPrMuted(menu.pr.id, !menu.pr.muted),
                      },
                      {
                        label: "Mark read",
                        onClick: () => void ipc.markPrRead(menu.pr.id),
                      },
                      {
                        label: "Open on GitHub",
                        onClick: () => openPr(menu.pr),
                      },
                      {
                        label: "Untrack",
                        danger: true,
                        onClick: () => void ipc.untrackPr(menu.pr.id),
                      },
                    ],
                  },
                  {
                    title: `@${menu.pr.author} priority (all their PRs)`,
                    items: [
                      {
                        type: "custom",
                        key: "author-priority",
                        render: () => (
                          <PrioritySelector
                            levels={REPO_PRIORITY_DISPLAY_ORDER}
                            value={authorPrioOf(menu.pr.author)}
                            onChange={(p) => void setAuthorPriority(menu.pr.author, p)}
                            getLabel={(p) => REPO_PRIORITY_LABEL[p]}
                            getIcon={(p) => REPO_PRIORITY_ICON[p].icon}
                            groupLabel={`@${menu.pr.author} priority`}
                          />
                        ),
                      },
                    ],
                  },
                  {
                    title: `${menu.pr.repo} priority`,
                    items: [
                      {
                        type: "custom",
                        key: "repo-priority",
                        render: () => (
                          <PrioritySelector
                            levels={REPO_PRIORITY_DISPLAY_ORDER}
                            value={prioOf(menu.pr.repo)}
                            onChange={(p) => void setRepoPriority(menu.pr.repo, p)}
                            getLabel={(p) => REPO_PRIORITY_LABEL[p]}
                            getIcon={(p) => REPO_PRIORITY_ICON[p].icon}
                            groupLabel={`${menu.pr.repo} priority`}
                          />
                        ),
                      },
                    ],
                  },
                  {
                    items: [
                      {
                        label: "Watch all PRs in this repo",
                        checked: watchedRepos.includes(menu.pr.repo),
                        onClick: () => void toggleWatchRepo(menu.pr.repo),
                      },
                    ],
                  },
                ]
              : [
                  {
                    title: `${menu.repo} priority`,
                    items: [
                      {
                        type: "custom",
                        key: "repo-priority",
                        render: () => (
                          <PrioritySelector
                            levels={REPO_PRIORITY_DISPLAY_ORDER}
                            value={prioOf(menu.repo)}
                            onChange={(p) => void setRepoPriority(menu.repo, p)}
                            getLabel={(p) => REPO_PRIORITY_LABEL[p]}
                            getIcon={(p) => REPO_PRIORITY_ICON[p].icon}
                            groupLabel={`${menu.repo} priority`}
                          />
                        ),
                      },
                    ],
                  },
                  {
                    items: [
                      {
                        label: "Watch all PRs in this repo",
                        checked: watchedRepos.includes(menu.repo),
                        onClick: () => void toggleWatchRepo(menu.repo),
                      },
                      {
                        label: "Open on GitHub",
                        onClick: () => void openUrl(`https://github.com/${menu.repo}`),
                      },
                      {
                        label: "Repo settings…",
                        onClick: () => setRepoSettingsRepo(menu.repo),
                      },
                    ],
                  },
                ]
          }
        />
      )}
      <RepoSettingsDrawer
        repo={repoSettingsRepo ?? ""}
        open={!!repoSettingsRepo}
        onClose={() => setRepoSettingsRepo(null)}
        settings={repoSettings}
        onSaveSettings={saveRepoSettings}
        priority={repoSettingsRepo ? prioOf(repoSettingsRepo) : "standard"}
        onSetPriority={(p) => repoSettingsRepo && void setRepoPriority(repoSettingsRepo, p)}
      />

      {showHotkeys && (
        <HotkeysHelp
          shortcuts={[...shortcuts, ...DOC_SHORTCUTS]}
          onClose={() => setShowHotkeys(false)}
        />
      )}
      <HistoryDrawer open={showHistory} onClose={() => setShowHistory(false)} />
    </div>
  );
}

/** First-run guidance when nothing is connected yet. */
function Onboarding({ onOpenSettings }: { onOpenSettings: () => void }) {
  const STEPS: [string, string][] = [
    ["Connect GitHub", "Settings → GitHub — paste a PAT with repo read scope. Your review queue, authored PRs, and mentions appear within a poll."],
    ["Point at Bedrock", "Settings → AWS — profile, region, and your inference-profile ARN. Test connection will tell you if SSO needs a refresh."],
    ["Watch the repos you own", "Right-click any repo in the list (or Settings → Repositories) to track every PR, not just yours."],
    ["Live in the callout", "The small always-on-top window shows only what needs you. ? shows the keyboard shortcuts."],
  ];
  return (
    <div className="onboarding">
      <h1>Welcome to CORA</h1>
      <p className="pane-intro">
        Principal-engineer PR review: architecture fit, boundary impacts, and Well-Architected
        findings — not lint nits.
      </p>
      <ol className="onboarding-steps">
        {STEPS.map(([title, body], i) => (
          <li key={i}>
            <span className="onboarding-step-title">{title}</span>
            <span className="onboarding-step-body">{body}</span>
          </li>
        ))}
      </ol>
      <button className="action-btn auth-primary" onClick={onOpenSettings}>
        Open settings
      </button>
    </div>
  );
}

function HotkeysHelp({
  shortcuts,
  onClose,
}: {
  shortcuts: Shortcut[];
  onClose: () => void;
}) {
  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <div className="hotkeys-help">
        <div className="drawer-title">
          Keyboard shortcuts
          <span className="spacer" />
          <button className="icon-btn" {...tip("Close")} onClick={onClose}>
            ✕
          </button>
        </div>
        <table>
          <tbody>
            {shortcuts.map((s) => (
              <tr key={s.keys}>
                <td className="mono hotkey-key">{s.keys}</td>
                <td>{s.label}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="hotkeys-foot">
          Press <span className="mono">?</span> any time, or use the ⌨ button beside the gear.
        </p>
      </div>
    </>
  );
}
