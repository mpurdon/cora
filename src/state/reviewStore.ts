import { create } from "zustand";
import type { PrReviews } from "../bindings/PrReviews";
import type { ReviewSummary } from "../bindings/ReviewSummary";
import type { ReviewVerdict } from "../bindings/ReviewVerdict";
import { onReviewSubmitted } from "../lib/ipc";

interface ReviewState {
  /** The review you just submitted, keyed by PR id. GitHub's read-back lags
   *  its own mutation by a second or more: query right after approving and the
   *  strip still lists you as a pending reviewer, so the click looks like it
   *  did nothing. Holding the created review here lets the header and the
   *  conversation show it at once. Dropped as soon as a refetch agrees. */
  pending: Record<string, ReviewVerdict>;
  record: (prId: string, verdict: ReviewVerdict) => void;
  clear: (prId: string) => void;
}

export const useReviewStore = create<ReviewState>((set, get) => ({
  pending: {},
  record: (prId, verdict) => set({ pending: { ...get().pending, [prId]: verdict } }),
  clear: (prId) => {
    if (!(prId in get().pending)) return;
    const next = { ...get().pending };
    delete next[prId];
    set({ pending: next });
  },
}));

let subscribed = false;

/** Subscribe once, from a mounted component. Reviews submitted by the
 *  assistant land here the same way yours do — the backend announces every
 *  verdict it creates, whoever asked for it. Deliberately not a module-level
 *  side effect: a throw at import time takes down the whole entry graph, and
 *  main.tsx's error reporting with it. */
export function initReviewStore(): void {
  if (subscribed) return;
  subscribed = true;
  void onReviewSubmitted((e) => useReviewStore.getState().record(e.prId, e.verdict));
}

/** Has a refetch caught up with a pending verdict? Both timestamps come from
 *  GitHub (the mutation returns the review it created), so this compares
 *  server clock to server clock — no local skew to guess at. */
export function serverHasVerdict(pending: ReviewVerdict, reviews: ReviewSummary[]): boolean {
  return reviews.some(
    (r) =>
      r.author === pending.author &&
      r.submittedAt != null &&
      r.submittedAt >= pending.submittedAt,
  );
}

/** The review state to render: server truth, with a just-submitted review
 *  folded in until the server reports it. Also drops you from the requested
 *  list — submitting a review clears the request, GitHub just hasn't said so
 *  yet. A no-op once the refetch has caught up, so callers can merge freely. */
export function withPending(
  reviews: PrReviews | null,
  pending: ReviewVerdict | undefined,
): PrReviews | null {
  if (!reviews || !pending || serverHasVerdict(pending, reviews.reviews)) return reviews;
  const me = pending.author || reviews.viewerLogin;
  return {
    ...reviews,
    requested: reviews.requested.filter((who) => who !== me),
    reviews: [
      ...reviews.reviews.filter((r) => r.author !== me),
      { author: me, state: pending.state, submittedAt: pending.submittedAt },
    ],
  };
}

/** Your review, when it should lock the approve / request-changes buttons —
 *  null while you're free to act. Locked until the PR moves: new commits,
 *  your review re-requested, or (for changes-requested) threads all resolved. */
export function lockedReview(reviews: PrReviews | null): ReviewSummary | null {
  const mine = reviews?.reviews.find((r) => r.author === reviews.viewerLogin);
  if (!reviews || !mine) return null;
  if (mine.state !== "APPROVED" && mine.state !== "CHANGES_REQUESTED") return null;
  const reRequested = reviews.requested.includes(reviews.viewerLogin);
  const commitsAfterReview =
    mine.submittedAt != null &&
    reviews.lastCommitAt != null &&
    reviews.lastCommitAt > mine.submittedAt;
  const threadsCleared = mine.state === "CHANGES_REQUESTED" && reviews.openThreads === 0;
  return reRequested || commitsAfterReview || threadsCleared ? null : mine;
}
