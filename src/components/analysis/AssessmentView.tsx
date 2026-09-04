import { useState } from "react";
import type { Assessment } from "../../bindings/Assessment";
import type { CodeFinding } from "../../bindings/CodeFinding";
import type { Pillar } from "../../bindings/Pillar";
import type { BoundaryImpact } from "../../bindings/BoundaryImpact";
import type { WaFinding } from "../../bindings/WaFinding";
import { findingMarker, IMPACT_LABEL, PILLAR_LABEL, type Explainable } from "../../lib/comments";

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

/** The row of actions on a finding — explain it, or turn it into a review
 *  comment. Shared by every finding row (code, Well-Architected, boundary
 *  impact) here and by the per-file insights panel, which shows the same code
 *  finding in a second place and must offer the same verbs for it.
 *  `commented`: a code finding is matched by its line; a Well-Architected
 *  finding or boundary impact by the marker its comment carries. */
export function FindingActions({
  commented = false,
  onExplain,
  onComment,
}: {
  commented?: boolean;
  onExplain: () => void;
  onComment: () => void;
}) {
  return (
    <div className="finding-actions">
      <span
        className="finding-comment-btn"
        role="button"
        data-tip="Explain this finding in plain terms — and what to do about it — in the assistant chat"
        onClick={(e) => {
          // The WA and impact rows are themselves buttons (expand / focus
          // canvas); explaining shouldn't also trigger those.
          e.stopPropagation();
          onExplain();
        }}
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
        <CommentFindingButton onClick={onComment} />
      )}
    </div>
  );
}

/** Collapsed by default: severity + the actionable "→ …" line. Expanding
 *  reveals the full finding detail and the canvas link. */
function WaFindingRow({
  finding,
  commented,
  onFocusNodes,
  onCommentFinding,
  onExplain,
}: {
  finding: WaFinding;
  commented: boolean;
  onFocusNodes: (nodeIds: string[]) => void;
  onCommentFinding: (seed: string, nodeIds: string[], marker: string) => void;
  onExplain: (finding: WaFinding) => void;
}) {
  const [open, setOpen] = useState(false);
  const seed = `**${PILLAR_LABEL[finding.pillar]} · ${finding.severity}**: ${finding.finding}\n\n→ ${finding.recommendation}`;
  return (
    <div className={`wa-finding-row${open ? " open" : ""}`}>
      <button className="wa-finding-header" onClick={() => setOpen((o) => !o)}>
        <span className="chevron">{open ? "▾" : "▸"}</span>
        <span className={`sev sev-${finding.severity}`}>{finding.severity}</span>
        <span className="wa-rec-line">→ {finding.recommendation}</span>
        <FindingActions
          commented={commented}
          onExplain={() => onExplain(finding)}
          onComment={() => onCommentFinding(seed, finding.nodeIds, findingMarker(finding))}
        />
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
  isMarkedCommented,
}: {
  assessment: Assessment;
  codeFindings: CodeFinding[];
  codePass: string | null;
  onFocusNodes: (nodeIds: string[]) => void;
  onCommentFinding: (seed: string, nodeIds: string[], marker: string) => void;
  onCommentCode: (finding: CodeFinding) => void;
  /** Hand a finding — code, Well-Architected, or boundary impact — to the
   *  assistant chat for a plain-language, actionable read. */
  onExplainCode: (finding: Explainable) => void;
  /** Whether a finding already has a review comment from the viewer. */
  isCommented?: (finding: CodeFinding) => boolean;
  /** Same, for the findings matched by marker rather than by line. */
  isMarkedCommented?: (finding: WaFinding | BoundaryImpact) => boolean;
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
                  <FindingActions
                    commented={isMarkedCommented?.(impact) ?? false}
                    onExplain={() => onExplainCode(impact)}
                    onComment={() =>
                      onCommentFinding(
                        `**${IMPACT_LABEL[impact.kind]} impact**: ${impact.description}`,
                        impact.nodeIds,
                        findingMarker(impact),
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
                    <FindingActions
                      commented={commented}
                      onExplain={() => onExplainCode(f)}
                      onComment={() => onCommentCode(f)}
                    />
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
                  commented={isMarkedCommented?.(f) ?? false}
                  key={i}
                  finding={f}
                  onFocusNodes={onFocusNodes}
                  onCommentFinding={onCommentFinding}
                  onExplain={onExplainCode}
                />
              ))}
            </div>
          ))}
        </section>
      )}
    </div>
  );
}
