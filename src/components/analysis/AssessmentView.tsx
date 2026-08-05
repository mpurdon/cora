import { useState } from "react";
import type { Assessment } from "../../bindings/Assessment";
import type { CodeFinding } from "../../bindings/CodeFinding";
import type { ImpactKind } from "../../bindings/ImpactKind";
import type { Pillar } from "../../bindings/Pillar";
import type { WaFinding } from "../../bindings/WaFinding";

const IMPACT_LABEL: Record<ImpactKind, string> = {
  external: "external system",
  service: "service boundary",
  internal: "internal",
};

const PILLAR_LABEL: Record<Pillar, string> = {
  "operational-excellence": "Operational excellence",
  security: "Security",
  reliability: "Reliability",
  "performance-efficiency": "Performance efficiency",
  "cost-optimization": "Cost optimization",
  sustainability: "Sustainability",
};

const FIT_LABEL = {
  fits: "fits the architecture",
  tension: "in tension with the architecture",
  misfit: "does not fit the architecture",
} as const;

function DetailExpander({ detail }: { detail: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="detail-expander">
      <button className="detail-toggle" onClick={() => setOpen((o) => !o)}>
        {open ? "▾ less" : "▸ more detail"}
      </button>
      {open && <p className="assess-detail">{detail}</p>}
    </div>
  );
}

/** Hover affordance on a finding: open a comment composer pre-filled with
 *  the finding, anchored in the diff when a file can be resolved. */
function CommentFindingButton({ onClick }: { onClick: () => void }) {
  return (
    <span
      className="finding-comment-btn"
      role="button"
      data-tip="Draft a review comment from this finding"
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
    >
      ± comment
    </span>
  );
}

/** Collapsed by default: severity + the actionable "→ …" line. Expanding
 *  reveals the full finding detail and the canvas link. */
function WaFindingRow({
  finding,
  onFocusNodes,
  onCommentFinding,
}: {
  finding: WaFinding;
  onFocusNodes: (nodeIds: string[]) => void;
  onCommentFinding: (seed: string, nodeIds: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const seed = `**${PILLAR_LABEL[finding.pillar]} · ${finding.severity}**: ${finding.finding}\n\n→ ${finding.recommendation}`;
  return (
    <div className={`wa-finding-row${open ? " open" : ""}`}>
      <button className="wa-finding-header" onClick={() => setOpen((o) => !o)}>
        <span className="chevron">{open ? "▾" : "▸"}</span>
        <span className={`sev sev-${finding.severity}`}>{finding.severity}</span>
        <span className="wa-rec-line">→ {finding.recommendation}</span>
        <CommentFindingButton onClick={() => onCommentFinding(seed, finding.nodeIds)} />
      </button>
      {open && (
        <div className="wa-finding-detail">
          <p className="wa-text">{finding.finding}</p>
          {finding.nodeIds.length > 0 && (
            <button className="thread-anchor" onClick={() => onFocusNodes(finding.nodeIds)}>
              show on canvas
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** The failure reason from a `code_pass` value ("ok" | "failed: …" | "off"),
 *  or null when it didn't fail — the one place that string format is parsed. */
export function codePassFailure(codePass: string | null | undefined): string | null {
  return codePass?.startsWith("failed") ? codePass.replace(/^failed:\s*/, "") : null;
}

export function AssessmentView({
  assessment,
  codeFindings,
  codePass,
  onFocusNodes,
  onCommentFinding,
  onCommentCode,
  onExplainCode,
  isCommented,
}: {
  assessment: Assessment;
  codeFindings: CodeFinding[];
  codePass: string | null;
  onFocusNodes: (nodeIds: string[]) => void;
  onCommentFinding: (seed: string, nodeIds: string[]) => void;
  onCommentCode: (finding: CodeFinding) => void;
  /** Hand a finding to the assistant chat for a plain-language, actionable read. */
  onExplainCode: (finding: CodeFinding) => void;
  /** Whether a finding already has a review comment from the viewer. */
  isCommented?: (finding: CodeFinding) => boolean;
}) {
  const codeByFile = new Map<string, CodeFinding[]>();
  for (const f of codeFindings) {
    codeByFile.set(f.path, [...(codeByFile.get(f.path) ?? []), f]);
  }
  const byPillar = new Map<Pillar, WaFinding[]>();
  for (const f of assessment.wellArchitected) {
    byPillar.set(f.pillar, [...(byPillar.get(f.pillar) ?? []), f]);
  }

  return (
    <div className="assessment">
      <section>
        <span className="eyebrow">Summary</span>
        <p className="assess-summary">{assessment.summary}</p>
        {assessment.detail && <DetailExpander detail={assessment.detail} />}
        <div className={`fit-verdict ${assessment.fit}`}>
          <span className="fit-chip">{assessment.fit}</span>
          <span>{FIT_LABEL[assessment.fit]}</span>
        </div>
        <p className="assess-rationale">{assessment.fitRationale}</p>
      </section>

      {assessment.contextNotes.length > 0 && (
        <section>
          <span className="eyebrow">What you'd need to know without full context</span>
          <ul className="context-notes">
            {assessment.contextNotes.map((note, i) => (
              <li key={i}>{note}</li>
            ))}
          </ul>
        </section>
      )}

      {assessment.boundaryImpacts.length > 0 && (
        <section>
          <span className="eyebrow">Boundary impacts — most important first</span>
          <ol className="impact-list">
            {assessment.boundaryImpacts.map((impact, i) => (
              <li key={i}>
                <button
                  className={`impact kind-${impact.kind}`}
                  onClick={() => onFocusNodes(impact.nodeIds)}
                  data-tip={impact.nodeIds.length > 0 ? "Show on the architecture canvas" : undefined}
                >
                  <span className={`impact-kind ${impact.kind}`}>
                    {IMPACT_LABEL[impact.kind]}
                  </span>
                  <span className="impact-desc">{impact.description}</span>
                  <CommentFindingButton
                    onClick={() =>
                      onCommentFinding(
                        `**${IMPACT_LABEL[impact.kind]} impact**: ${impact.description}`,
                        impact.nodeIds,
                      )
                    }
                  />
                </button>
              </li>
            ))}
          </ol>
        </section>
      )}

      {/* Empty is only meaningful when the pass actually ran — say which. */}
      {codeByFile.size === 0 && codePassFailure(codePass) != null && (
        <section>
          <span className="eyebrow">Code findings — defects &amp; reuse</span>
          <p className="code-pass-note bad">
            The code-level pass failed — this section may be missing findings. Re-analyze to
            retry. ({codePassFailure(codePass)})
          </p>
        </section>
      )}
      {codeByFile.size === 0 && codePass === "ok" && (
        <section>
          <span className="eyebrow">Code findings — defects &amp; reuse</span>
          <p className="code-pass-note">None — the code pass ran clean.</p>
        </section>
      )}
      {codeByFile.size > 0 && (
        <section>
          <span className="eyebrow">Code findings — defects &amp; reuse</span>
          {[...codeByFile.entries()].map(([path, findings]) => (
            <div key={path} className="code-file-group">
              <div className="code-file-path mono" data-tip={path}>
                {path.includes("/") && (
                  <span className="anchor-dir">{path.slice(0, path.lastIndexOf("/") + 1)}</span>
                )}
                <span className="anchor-name">{path.slice(path.lastIndexOf("/") + 1)}</span>
              </div>
              {findings.map((f, i) => {
                const commented = isCommented?.(f) ?? false;
                return (
                  <div key={i} className={`code-finding-row${commented ? " commented" : ""}`}>
                    <span className={`sev sev-${f.severity}`}>{f.severity}</span>
                    <span className="kind-tag mono">{f.kind}</span>
                    <div className="code-finding-body">
                      <div>{f.finding}</div>
                      <div className="code-finding-suggestion">→ {f.suggestion}</div>
                    </div>
                    <div className="finding-actions">
                      <span
                        className="finding-comment-btn"
                        role="button"
                        data-tip="Explain this finding in plain terms — and what to do about it — in the assistant chat"
                        onClick={() => onExplainCode(f)}
                      >
                        explain
                      </span>
                      {commented ? (
                        <span
                          className="finding-comment-btn commented"
                          aria-disabled="true"
                          data-tip="You have a review comment at this line"
                        >
                          ✓ commented
                        </span>
                      ) : (
                        <CommentFindingButton onClick={() => onCommentCode(f)} />
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          ))}
        </section>
      )}

      {byPillar.size > 0 && (
        <section>
          <span className="eyebrow">Well-Architected findings</span>
          {[...byPillar.entries()].map(([pillar, findings]) => (
            <div key={pillar} className="pillar-group">
              <div className="pillar-name">{PILLAR_LABEL[pillar]}</div>
              {findings.map((f, i) => (
                <WaFindingRow
                  key={i}
                  finding={f}
                  onFocusNodes={onFocusNodes}
                  onCommentFinding={onCommentFinding}
                />
              ))}
            </div>
          ))}
        </section>
      )}
    </div>
  );
}
