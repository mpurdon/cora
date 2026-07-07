import type { TrackedPr } from "../bindings/TrackedPr";
import { ciTone, mergeTone, reviewTone } from "../state/prStore";

/** The signature 3-lamp strip: CI / review / mergeability, top to bottom. */
export function StatusStrip({ pr, pulsing }: { pr: TrackedPr; pulsing?: boolean }) {
  const lamps = [
    { tone: ciTone(pr), label: `CI checks: ${(pr.ciStatus ?? "none").toLowerCase()}` },
    { tone: reviewTone(pr), label: `review: ${(pr.reviewDecision ?? "none").toLowerCase().replace(/_/g, " ")}` },
    { tone: mergeTone(pr), label: `merge conflicts: ${pr.mergeable === "CONFLICTING" ? "yes" : pr.mergeable === "MERGEABLE" ? "none" : "unknown"}` },
  ];
  return (
    <div
      className={`strip${pulsing ? " pulsing" : ""}`}
      title={lamps.map((l) => l.label).join("\n")}
    >
      {lamps.map((l, i) => (
        <span key={i} className={`lamp ${l.tone}`} role="img" aria-label={l.label} />
      ))}
    </div>
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
