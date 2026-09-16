import type { ReviewDismissal } from "../bindings/ReviewDismissal";

/** What a review said, as the noun a dismissal takes away. */
export function verdictNoun(state: string): string {
  return state === "APPROVED"
    ? "approval"
    : state === "CHANGES_REQUESTED"
      ? "change request"
      : "review";
}

/** How a review came to be dismissed, in one line — a push under "dismiss
 *  stale approvals" names the commit; a person names themselves and their
 *  reason. Shared by the PR header and its History tab so the same event
 *  reads the same in both. */
export function describeDismissal(d: ReviewDismissal, me: string): string {
  const whose = d.reviewer === me ? "your" : `@${d.reviewer}'s`;
  const what = `${whose} ${verdictNoun(d.previousState)}`;
  if (d.commitSha) return `${what} was dismissed by @${d.actor}'s push ${d.commitSha}`;
  const why = d.message.trim() ? `: “${d.message.trim()}”` : "";
  return `${what} was dismissed by @${d.actor}${why}`;
}
