import type { AnalysisUsage } from "../../bindings/AnalysisUsage";
import type { TraceStep } from "../../bindings/TraceStep";

const KIND_GLYPH: Record<string, string> = {
  tool: "⚒",
  thought: "…",
  status: "●",
};

export function formatTokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : String(n);
}

/**
 * Right-hand slide-over showing the agent's exploration, live during a run
 * and re-readable afterwards (the trace is persisted with the result).
 */
export function ActivityDrawer({
  open,
  onClose,
  steps,
  usage,
  live,
}: {
  open: boolean;
  onClose: () => void;
  steps: TraceStep[];
  usage?: AnalysisUsage;
  live: boolean;
}) {
  return (
    <>
      {open && <div className="drawer-backdrop" onClick={onClose} />}
      <aside className={`activity-drawer${open ? " open" : ""}`} aria-hidden={!open}>
        <header className="drawer-header">
          <span className="drawer-title">
            {live && <span className="sync-dot live" />}
            Activity
          </span>
          {usage && usage.turns > 0 && (
            <span className="drawer-usage mono">
              {usage.turns} turns · {formatTokens(usage.inputTokens)} in /{" "}
              {formatTokens(usage.outputTokens)} out
            </span>
          )}
          <button className="icon-btn" title="Close" onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          {steps.length === 0 && <div className="drawer-empty">no activity yet</div>}
          {steps.map((step, i) => (
            <div key={i} className={`trace-step kind-${step.kind}`}>
              <span className="trace-glyph">{KIND_GLYPH[step.kind] ?? "·"}</span>
              <div className="trace-content">
                <span className="trace-time">{step.at.slice(11, 19)}</span>
                <span className="trace-msg">{step.message}</span>
              </div>
            </div>
          ))}
        </div>
      </aside>
    </>
  );
}
