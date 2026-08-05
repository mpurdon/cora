import { create } from "zustand";
import type { PollStatus } from "../bindings/PollStatus";
import type { TrackedPr } from "../bindings/TrackedPr";
import { ipc, onPollStatus, onPrChanged, onPrsSnapshot } from "../lib/ipc";

/** Ids that changed in the last poll cycle — drives the pulse animation. */
interface PrState {
  prs: TrackedPr[];
  pollStatus: PollStatus | null;
  recentlyChanged: Set<string>;
  init: () => Promise<void>;
  /** Org switch: drop every PR — the snapshot for the new org follows. */
  reset: () => void;
}

let initialized = false;

export const usePrStore = create<PrState>((set, get) => ({
  prs: [],
  pollStatus: null,
  recentlyChanged: new Set(),

  reset: () => set({ prs: [], pollStatus: null, recentlyChanged: new Set() }),

  init: async () => {
    if (initialized) {
      set({ prs: await ipc.listPrs() });
      return;
    }
    initialized = true;
    await onPrsSnapshot((prs) => set({ prs }));
    await onPollStatus((pollStatus) => set({ pollStatus }));
    await onPrChanged(({ pr }) => {
      const next = new Set(get().recentlyChanged);
      next.add(pr.id);
      set({ recentlyChanged: next });
      setTimeout(() => {
        const after = new Set(get().recentlyChanged);
        after.delete(pr.id);
        set({ recentlyChanged: after });
      }, 4000);
    });
    set({ prs: await ipc.listPrs() });
  },
}));

/** Closed or merged — the work is over, whatever the review lamps say about it.
 *  Mirrors `models::terminal_kind` on the Rust side, including its refusal to
 *  treat an unrecognised state as finished: guessing there would hide a live PR
 *  from the rail while the backend kept sweeping it as live. */
export function isFinished(pr: TrackedPr): boolean {
  return pr.state === "CLOSED" || pr.state === "MERGED";
}

export function timeAgo(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const secs = Math.max(0, Math.floor((Date.now() - then) / 1000));
  // Callers append " ago" — "moments ago" reads right, "now ago" doesn't.
  if (secs < 60) return "moments";
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

const TITLE_TYPE_RE =
  /^(feat|fix|chore|docs|refactor|test|style|perf|build|ci|revert)(\([^)]*\))?!?:\s*/i;

/** Split a conventional-commit style title into its type and clean text. */
export function parseTitle(title: string): { type: string; clean: string } {
  const m = title.match(TITLE_TYPE_RE);
  if (!m) return { type: "unknown", clean: title };
  return { type: m[1].toLowerCase(), clean: title.slice(m[0].length) || title };
}

export type DotTone = "ok" | "bad" | "warn" | "idle";

export function ciStatusTone(status: string | null | undefined): DotTone {
  switch (status) {
    case "SUCCESS":
      return "ok";
    case "FAILURE":
    case "ERROR":
      return "bad";
    case "PENDING":
    case "EXPECTED":
      return "warn";
    default:
      return "idle";
  }
}

export function ciTone(pr: TrackedPr): DotTone {
  return ciStatusTone(pr.ciStatus);
}

export function reviewTone(pr: TrackedPr): DotTone {
  switch (pr.reviewDecision) {
    case "APPROVED":
      return "ok";
    case "CHANGES_REQUESTED":
      return "bad";
    case "REVIEW_REQUIRED":
      return "warn";
    default:
      return "idle";
  }
}

export function mergeTone(pr: TrackedPr): DotTone {
  switch (pr.mergeable) {
    case "MERGEABLE":
      return "ok";
    case "CONFLICTING":
      return "bad";
    default:
      return "idle";
  }
}
