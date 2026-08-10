import type { TrackedPr } from "../bindings/TrackedPr";
import { ciTone, isFinished, mergeTone } from "../state/prStore";

/**
 * The attention buckets both windows share: what the user should DO.
 * A PR can belong to several buckets — these are counters, not partitions.
 */
export type ActionKind = "fix" | "address" | "merge" | "review" | "new" | "comments";

export const ACTION_ORDER: ActionKind[] = ["fix", "address", "merge", "review", "new", "comments"];

export const ACTION_META: Record<ActionKind, { label: string; short: string }> = {
  fix: { label: "Your PR is blocked", short: "fix" },
  address: { label: "Changes requested on yours", short: "address" },
  merge: { label: "Approved — ready to merge", short: "merge" },
  review: { label: "Waiting on your review", short: "review" },
  new: { label: "Never opened", short: "new" },
  comments: { label: "New comments", short: "comments" },
};

export function inBucket(pr: TrackedPr, kind: ActionKind): boolean {
  if (pr.muted || isFinished(pr)) return false;
  const authored = pr.sources.includes("authored");
  switch (kind) {
    case "fix":
      return authored && (ciTone(pr) === "bad" || mergeTone(pr) === "bad");
    case "address":
      return authored && pr.reviewDecision === "CHANGES_REQUESTED";
    case "merge":
      return (
        authored &&
        pr.reviewDecision === "APPROVED" &&
        ciTone(pr) !== "bad" &&
        ciTone(pr) !== "warn" &&
        mergeTone(pr) !== "bad" &&
        !pr.isDraft
      );
    case "review":
      return (
        pr.sources.includes("review-requested") && !pr.isDraft && pr.reviewDecision !== "APPROVED"
      );
    case "new":
      return pr.unread.includes("new");
    case "comments":
      return pr.unread.includes("new-comments");
  }
}

/** One-line explanation for a PR's presence in a bucket. */
export function bucketReason(pr: TrackedPr, kind: ActionKind): string {
  switch (kind) {
    case "fix": {
      const parts: string[] = [];
      if (ciTone(pr) === "bad") parts.push("CI failing");
      if (mergeTone(pr) === "bad") parts.push("merge conflicts");
      return parts.join(" · ");
    }
    case "address":
      return "changes requested";
    case "merge":
      return "approved · checks green";
    case "review": {
      const gates: string[] = [];
      if (ciTone(pr) === "warn") gates.push("CI running");
      if (ciTone(pr) === "bad") gates.push("CI red");
      if (mergeTone(pr) === "bad") gates.push("conflicts");
      return gates.length > 0 ? gates.join(" · ") : "ready for review";
    }
    case "new":
      return "you haven't opened this yet";
    case "comments":
      return "new comments since you looked";
  }
}
