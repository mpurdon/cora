import type { ChangeKind } from "../bindings/ChangeKind";
import type { TrackedPr } from "../bindings/TrackedPr";
import { ciTone, mergeTone, reviewTone } from "../state/prStore";
import { Tooltip } from "./Tooltip";

/** The signature 3-lamp strip: CI / review / mergeability, top to bottom. */
export function StatusStrip({ pr, pulsing }: { pr: TrackedPr; pulsing?: boolean }) {
  const lamps = [
    { tone: ciTone(pr), label: `CI checks: ${(pr.ciStatus ?? "none").toLowerCase()}` },
    { tone: reviewTone(pr), label: `review: ${(pr.reviewDecision ?? "none").toLowerCase().replace(/_/g, " ")}` },
    { tone: mergeTone(pr), label: `merge conflicts: ${pr.mergeable === "CONFLICTING" ? "yes" : pr.mergeable === "MERGEABLE" ? "none" : "unknown"}` },
  ];
  return (
    <Tooltip
      className={`strip${pulsing ? " pulsing" : ""}`}
      content={
        <ul className="tooltip-list">
          {lamps.map((l, i) => (
            <li key={i}>
              <span className={`lamp ${l.tone}`} />
              <span>{l.label}</span>
            </li>
          ))}
        </ul>
      }
    >
      {lamps.map((l, i) => (
        <span key={i} className={`lamp ${l.tone}`} role="img" aria-label={l.label} />
      ))}
    </Tooltip>
  );
}

/** What each change kind is, in words — the badge counts them all equally, so
 *  the breakdown is the only place you learn a "3" was a push, a comment and a
 *  CI flip rather than three commits. */
const KIND_LABEL: Record<ChangeKind, string> = {
  new: "First time you've seen this PR",
  "new-commits": "New commits pushed",
  "new-comments": "New comments",
  "ci-changed": "CI status changed",
  "review-changed": "Review decision changed",
  "title-changed": "Title edited",
  "draft-changed": "Draft status changed",
  merged: "Merged",
  closed: "Closed",
  reopened: "Reopened",
};

/** The two kinds that outlive merely opening the PR, and what does clear them.
 *  Worth saying outright — otherwise a badge that won't go away looks broken. */
const KIND_CLEARS_ON: Partial<Record<ChangeKind, string>> = {
  "new-comments": "clears in Comments",
  "new-commits": "clears in Diff",
};

function unreadBreakdown(pr: TrackedPr) {
  const counts = new Map<ChangeKind, number>();
  for (const kind of pr.unread) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  return [...counts.entries()];
}

/** Left-edge marker: ◆ for never-opened PRs, update count otherwise. */
export function UnreadMarker({ pr }: { pr: TrackedPr }) {
  if (pr.unread.includes("new")) {
    return (
      <span className="marker new" data-tip="You haven't opened this PR yet">
        ◆
      </span>
    );
  }
  if (pr.unread.length === 0) return <span className="marker" aria-hidden="true" />;

  const rows = unreadBreakdown(pr);
  return (
    <Tooltip
      className="marker count"
      content={
        <>
          <div className="tooltip-head">
            {pr.unread.length} update{pr.unread.length > 1 ? "s" : ""} since you last opened
            this PR
          </div>
          <ul className="tooltip-list">
            {rows.map(([kind, n]) => (
              <li key={kind}>
                <span className="tooltip-count">{n > 1 ? `${n}×` : "•"}</span>
                <span>{KIND_LABEL[kind] ?? kind.replace(/-/g, " ")}</span>
                {KIND_CLEARS_ON[kind] && (
                  <span className="tooltip-hint">{KIND_CLEARS_ON[kind]}</span>
                )}
              </li>
            ))}
          </ul>
        </>
      }
    >
      <span aria-label={unreadTitle(pr)}>{pr.unread.length}</span>
    </Tooltip>
  );
}

/** Human description of a PR's unacknowledged changes, for badge tooltips. */
export function unreadTitle(pr: TrackedPr): string {
  if (pr.unread.length === 0) return "";
  const counts = new Map<string, number>();
  for (const kind of pr.unread) {
    counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  const parts = [...counts.entries()].map(
    ([kind, n]) => `${kind.replace(/-/g, " ")}${n > 1 ? ` ×${n}` : ""}`,
  );
  return `${pr.unread.length} update${pr.unread.length > 1 ? "s" : ""} since you last opened it: ${parts.join(", ")}`;
}
