import type { TrackedPr } from "../bindings/TrackedPr";
import { ciTone, mergeTone, reviewTone } from "../state/prStore";

/** The signature 3-lamp strip: CI / review / mergeability, top to bottom. */
export function StatusStrip({ pr, pulsing }: { pr: TrackedPr; pulsing?: boolean }) {
  const lamps = [
    { tone: ciTone(pr), label: `checks: ${pr.ciStatus ?? "none"}` },
    { tone: reviewTone(pr), label: `review: ${pr.reviewDecision ?? "none"}` },
    { tone: mergeTone(pr), label: `merge: ${pr.mergeable.toLowerCase()}` },
  ];
  return (
    <div className={`strip${pulsing ? " pulsing" : ""}`} aria-hidden={false}>
      {lamps.map((l, i) => (
        <span key={i} className={`lamp ${l.tone}`} title={l.label} role="img" aria-label={l.label} />
      ))}
    </div>
  );
}
