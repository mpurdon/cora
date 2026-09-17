import type { PrConversation } from "../bindings/PrConversation";
import type { PrReviews } from "../bindings/PrReviews";
import type { TrackedPr } from "../bindings/TrackedPr";
import { approveSeed, isNonBlockingComment } from "./comments";

/** The Teams message the button opens with: what you just did on the PR, as
 *  you'd type it in a chat, ending with the link so Teams unfurls it. Built
 *  from the same conversation the approve composer reads, so "approved, my
 *  two comments are addressed" says the same thing in both places. The
 *  assistant writes its own; this is the no-LLM path. Plain punctuation
 *  only: no em dashes in anything drafted as the user's words. */
export function teamsSeed(
  pr: TrackedPr,
  reviews: PrReviews | null,
  conversation: PrConversation | null,
  viewer: string,
): string {
  const ref = `${pr.repo.split("/")[1] ?? pr.repo}#${pr.number}`;
  const mine = reviews?.reviews.find((r) => r.author === viewer);
  const state = mine?.state;
  const comments = myCommentsCount(conversation, viewer);

  let line: string;
  if (state === "APPROVED") {
    // "Approving. My 2 comments on a.py are addressed." becomes
    // "Approved widgets#42. My 2 comments on a.py are addressed."
    line = approveSeed(conversation, viewer).replace(/^Approving\b/, `Approved ${ref}`);
  } else if (state === "CHANGES_REQUESTED") {
    line =
      comments.blocking > 0
        ? `Requested changes on ${ref}, ${count(comments.blocking, "comment")} to look at when you get a chance.`
        : `Requested changes on ${ref}, details in the review.`;
  } else if (comments.total > 0) {
    line = `Left ${count(comments.total, "comment")} on ${ref}${
      comments.blocking === 0 ? ", nothing blocking" : ""
    }, back to you.`;
  } else {
    line = `Had a look at ${ref}.`;
  }
  // The link rides on its own line so Teams unfurls it; the webhook route
  // also gets an HTML copy with the ref itself linked (teams.rs).
  return `${line}\n${pr.url}`;
}

function count(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

/** Threads you opened, split by whether they hold up approval. */
function myCommentsCount(
  conversation: PrConversation | null,
  viewer: string,
): { total: number; blocking: number } {
  let total = 0;
  let blocking = 0;
  for (const t of conversation?.threads ?? []) {
    const root = t.comments[0];
    if (!viewer || root?.author !== viewer) continue;
    total += 1;
    if (!t.resolved && !t.outdated && !isNonBlockingComment(root.body)) blocking += 1;
  }
  return { total, blocking };
}
