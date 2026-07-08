import { useState } from "react";
import type { Assessment } from "../../bindings/Assessment";
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

/** Collapsed by default: severity + the actionable "→ …" line. Expanding
 *  reveals the full finding detail and the canvas link. */
function WaFindingRow({
  finding,
  onFocusNodes,
}: {
  finding: WaFinding;
  onFocusNodes: (nodeIds: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`wa-finding-row${open ? " open" : ""}`}>
      <button className="wa-finding-header" onClick={() => setOpen((o) => !o)}>
        <span className="chevron">{open ? "▾" : "▸"}</span>
        <span className={`sev sev-${finding.severity}`}>{finding.severity}</span>
        <span className="wa-rec-line">→ {finding.recommendation}</span>
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

export function AssessmentView({
  assessment,
  onFocusNodes,
}: {
  assessment: Assessment;
  onFocusNodes: (nodeIds: string[]) => void;
}) {
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

      {assessment.boundaryImpacts.length > 0 && (
        <section>
          <span className="eyebrow">Boundary impacts — most important first</span>
          <ol className="impact-list">
            {assessment.boundaryImpacts.map((impact, i) => (
              <li key={i}>
                <button
                  className={`impact kind-${impact.kind}`}
                  onClick={() => onFocusNodes(impact.nodeIds)}
                  title={impact.nodeIds.length > 0 ? "Show on the architecture canvas" : undefined}
                >
                  <span className={`impact-kind ${impact.kind}`}>
                    {IMPACT_LABEL[impact.kind]}
                  </span>
                  <span className="impact-desc">{impact.description}</span>
                </button>
              </li>
            ))}
          </ol>
        </section>
      )}

      {byPillar.size > 0 && (
        <section>
          <span className="eyebrow">Well-Architected findings</span>
          {[...byPillar.entries()].map(([pillar, findings]) => (
            <div key={pillar} className="pillar-group">
              <div className="pillar-name">{PILLAR_LABEL[pillar]}</div>
              {findings.map((f, i) => (
                <WaFindingRow key={i} finding={f} onFocusNodes={onFocusNodes} />
              ))}
            </div>
          ))}
        </section>
      )}

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
    </div>
  );
}
