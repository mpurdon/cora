import type { CodeFinding } from "../../bindings/CodeFinding";
import type { TrackedPr } from "../../bindings/TrackedPr";
import { analysisKey, useAnalysisStore } from "../../state/analysisStore";
import { codePassFailure } from "./AssessmentView";
import { useDiffStore } from "../../state/diffStore";
import { parseDiffCached } from "./DiffView";
import { findingSeed } from "../../lib/comments";

/** Per-file insights for the file currently in view on the Diff tab —
 *  follows the scroll spy, so it always describes what the reviewer is
 *  reading: why it was ranked and its code findings. */
export function FileInsights({ pr }: { pr: TrackedPr }) {
  const visiblePath = useDiffStore((s) => s.visiblePath);
  const entry = useDiffStore((s) => s.entries[pr.id]);
  const requestFocusFile = useDiffStore((s) => s.requestFocusFile);
  const requestCompose = useDiffStore((s) => s.requestCompose);
  const run = useAnalysisStore((s) => s.runs[analysisKey(pr.id, "context")]);
  const result = run?.status === "done" ? run.result : undefined;

  const files = entry?.raw ? parseDiffCached(entry.raw) : [];
  const file = visiblePath ? files.find((f) => f.path === visiblePath) : undefined;
  const plan = result?.assessment.reviewPlan.find((p) => p.path === visiblePath);
  const findings = (result?.codeFindings ?? []).filter((f) => f.path === visiblePath);
  const otherFindings = new Map<string, number>();
  for (const f of result?.codeFindings ?? []) {
    if (f.path !== visiblePath) {
      otherFindings.set(f.path, (otherFindings.get(f.path) ?? 0) + 1);
    }
  }

  const commentCode = (f: CodeFinding) => {
    requestCompose({ target: "diff", path: f.path, line: f.line, seed: findingSeed(f) });
  };

  if (!visiblePath || !file) {
    return (
      <div className="insights">
        <div className="chat-empty">
          <p>Scroll the diff — insights follow the file in view.</p>
        </div>
      </div>
    );
  }

  const slash = file.path.lastIndexOf("/");
  return (
    <div className="insights">
      <div className="insights-file" title={file.path}>
        {slash >= 0 && (
          <div className="insights-dir mono">{file.path.slice(0, slash + 1)}</div>
        )}
        <div className="insights-name mono">
          {file.path.slice(slash + 1)}
          {file.added && <span className="file-state-tag added">new</span>}
          {file.deleted && <span className="file-state-tag deleted">deleted</span>}
        </div>
      </div>

      {plan ? (
        <div className={`insights-plan ${plan.significance}`}>
          <span className={`insights-sig ${plan.significance}`}>{plan.significance}</span>
          <p className="insights-reason">{plan.reason}</p>
        </div>
      ) : (
        result && <p className="insights-reason muted">Not called out in the review plan.</p>
      )}

      {!result && (
        <div className="chat-empty">
          <p>Run the analysis to get per-file findings and ranking rationale.</p>
        </div>
      )}

      {result && (
        <div className="insights-findings">
          <span className="eyebrow">Code findings</span>
          {findings.length === 0 &&
            (codePassFailure(result.codePass) != null ? (
              <p className="insights-reason muted">
                Code pass failed — findings may be missing. Re-analyze to retry.
              </p>
            ) : (
              <p className="insights-reason muted">None for this file.</p>
            ))}
          {findings.map((f, i) => (
            <div key={i} className="code-finding-row">
              <span className={`sev sev-${f.severity}`}>{f.severity}</span>
              <span className="kind-tag mono">{f.kind}</span>
              <div className="code-finding-body">
                <div>
                  {f.line != null && <span className="mono insights-line">L{f.line} · </span>}
                  {f.finding}
                </div>
                <div className="code-finding-suggestion">→ {f.suggestion}</div>
              </div>
              <span
                className="finding-comment-btn"
                role="button"
                title="Draft a review comment from this finding"
                onClick={() => commentCode(f)}
              >
                ± comment
              </span>
            </div>
          ))}
        </div>
      )}

      {otherFindings.size > 0 && (
        <div className="insights-others">
          <span className="eyebrow">Findings elsewhere</span>
          {[...otherFindings.entries()].map(([path, count]) => (
            <button
              key={path}
              className="insights-other mono"
              title={`Jump to ${path}`}
              onClick={() => requestFocusFile(path)}
            >
              <span className="anchor-name">{path.slice(path.lastIndexOf("/") + 1)}</span>
              <span className="group-count">{count}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
