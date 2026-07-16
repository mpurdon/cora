import type { TraceStep } from "../../bindings/TraceStep";

const KIND_GLYPH: Record<string, string> = {
  tool: "⚒",
  thought: "…",
  status: "●",
};

export function formatTokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : String(n);
}

/** The agent's exploration steps — live progress during a run, the persisted
 *  trace afterwards. */
export function TraceSteps({ steps }: { steps: TraceStep[] }) {
  return (
    <>
      {steps.map((step, i) => (
        <div key={i} className={`trace-step kind-${step.kind}`}>
          <span className="trace-glyph">{KIND_GLYPH[step.kind] ?? "·"}</span>
          <div className="trace-content">
            <span className="trace-time">{step.at.slice(11, 19)}</span>
            <span className="trace-msg">{step.message}</span>
          </div>
        </div>
      ))}
    </>
  );
}
