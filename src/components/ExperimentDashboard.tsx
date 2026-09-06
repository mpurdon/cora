import type React from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { layoutPoint } from "../lib/zoom";
import type { Experiment } from "../bindings/Experiment";
import type { RunRecord } from "../bindings/RunRecord";

/** Categorical slots, dark-surface steps, fixed order — variant 1 is always
 *  blue, never re-painted when a variant is removed. Validated against the
 *  app surface (#151a21): CVD ΔE ≥ 8.4, normal-vision ≥ 19.8, contrast ≥ 3:1. */
export const SERIES = ["#3987e5", "#d95926", "#199e70", "#c98500"];
/** Severity is magnitude, so one hue light→dark: info → high. */
const SEVERITY: { key: string; color: string; weight: number }[] = [
  { key: "high", color: "#1c5cab", weight: 3 },
  { key: "medium", color: "#3987e5", weight: 2 },
  { key: "low", color: "#6da7ec", weight: 1 },
  { key: "info", color: "#9ec5f4", weight: 0.5 },
];
const CODE_WEIGHT: Record<string, number> = { defect: 2, reuse: 1 };

type Result = NonNullable<RunRecord["result"]>;

/** A run's findings, weighted: pillar findings by severity, code findings by
 *  kind. A count is not quality — the side-by-side view is the quality
 *  check — but weight tells "found more that matters" from "found more". */
export function findingsWeight(r: Result): number {
  let w = 0;
  for (const f of r.assessment.wellArchitected) w += SEVERITY.find((s) => s.key === f.severity)?.weight ?? 1;
  for (const f of r.codeFindings) w += CODE_WEIGHT[f.kind] ?? 1;
  return w;
}

const fmtMs = (ms: number) =>
  ms < 60_000 ? `${Math.round(ms / 1000)}s` : `${Math.floor(ms / 60_000)}m${String(Math.round((ms % 60_000) / 1000)).padStart(2, "0")}s`;
const fmtUsd = (n: number) => (n >= 10 ? `$${n.toFixed(1)}` : n >= 1 ? `$${n.toFixed(2)}` : `$${n.toFixed(3)}`);
const fmtK = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(Math.round(n)));
const fmtW = (n: number) => (Number.isInteger(n) ? String(n) : n.toFixed(1));

type Metric = {
  key: string;
  title: string;
  hint: string;
  fmt: (n: number) => string;
  /** Which direction counts as better for the delta colouring. */
  lowerIsBetter: boolean;
  /** Gridline steps: durations tick on the clock (30s, 1m, 2m…), everything else on 1-2-5. */
  ticks: "time" | "number";
  of: (run: RunRecord) => number | null;
};
const METRICS: Metric[] = [
  { key: "wall", title: "Wall time", hint: "Queue to result, both passes", fmt: fmtMs, lowerIsBetter: true, ticks: "time", of: (r) => r.elapsedMs },
  { key: "cost", title: "Cost", hint: "Priced from the usage rows the run recorded", fmt: fmtUsd, lowerIsBetter: true, ticks: "number", of: (r) => r.costUsd },
  { key: "tokens", title: "Output tokens", hint: "Architecture pass, thinking included", fmt: fmtK, lowerIsBetter: true, ticks: "number", of: (r) => r.outputTokens },
  {
    key: "weight",
    title: "Findings weight",
    hint: "Pillar findings: high 3 · medium 2 · low 1 · info ½. Code findings: defect 2 · reuse 1",
    fmt: fmtW,
    lowerIsBetter: false,
    ticks: "number",
    of: (r) => (r.result ? findingsWeight(r.result) : null),
  },
];

type Tip = { x: number; y: number; lines: string[] } | null;

export function ExperimentDashboard({ exp, label }: { exp: Experiment; label: (prId: string) => string }) {
  const [tip, setTip] = useState<Tip>(null);
  const box = useRef<HTMLDivElement>(null);

  const model = useMemo(() => {
    const variants = exp.variants.slice(0, 4);
    const ok = (vid: string, prId: string) =>
      exp.runs.find((r) => r.variantId === vid && r.prId === prId && r.status === "ok" && r.result) ?? null;
    // Totals compare like for like: only PRs every variant has finished.
    const comparable = exp.prIds.filter((p) => variants.every((v) => ok(v.id, p)));
    const totals = variants.map((v) => {
      const runs = comparable.map((p) => ok(v.id, p)!);
      const sum = (f: (r: RunRecord) => number | null) => {
        const xs = runs.map(f);
        return xs.some((x) => x == null) ? null : xs.reduce((a: number, b) => a + (b ?? 0), 0);
      };
      const bySeverity = SEVERITY.map((s) => runs.reduce((n, r) => n + r.result!.assessment.wellArchitected.filter((w) => w.severity === s.key).length, 0));
      return {
        variant: v,
        metrics: Object.fromEntries(METRICS.map((m) => [m.key, sum(m.of)])) as Record<string, number | null>,
        pillars: runs.reduce((n, r) => n + r.result!.assessment.wellArchitected.length, 0),
        code: runs.reduce((n, r) => n + r.result!.codeFindings.length, 0),
        bySeverity,
      };
    });
    const perPr = METRICS.map((m) => ({
      metric: m,
      values: variants.map((v) => exp.prIds.map((p) => (ok(v.id, p) ? m.of(ok(v.id, p)!) : null))),
    }));
    return { variants, comparable, totals, perPr };
  }, [exp]);

  const { variants, comparable, totals, perPr } = model;
  if (variants.length === 0 || !exp.runs.some((r) => r.status === "ok")) return null;

  const showTip = (e: React.MouseEvent, lines: string[]) => {
    // The app zooms its root with CSS, so pointer coordinates have to be
    // brought into layout pixels before they position a fixed element.
    const at = layoutPoint(e.clientX, e.clientY);
    setTip({ x: at.x + 14, y: at.y + 14, lines });
  };

  return (
    <div
      className="dash"
      ref={box}
      onMouseLeave={() => setTip(null)}
      onMouseMove={(e) => {
        // A tooltip belongs to a mark under the pointer and nothing else —
        // moving onto padding, text, or a tile clears it.
        if (tip && !(e.target as Element).closest(".dash-bar, .dash-sev-seg")) setTip(null);
      }}
      onWheel={() => setTip(null)}
    >
      <div className="dash-legend">
        {variants.map((v, i) => (
          <span key={v.id} className="dash-key">
            <span className="dash-swatch" style={{ background: SERIES[i] }} />
            {v.name}
            {i === 0 && <span className="dash-muted"> · baseline</span>}
          </span>
        ))}
      </div>
      <div className="dash-muted dash-scope">
        Totals cover the {comparable.length} of {exp.prIds.length} bench PRs with a result in every variant.
        Deltas read against the baseline.
      </div>

      <div className="dash-tiles" style={{ gridTemplateColumns: `repeat(${variants.length}, minmax(0, 1fr))` }}>
        {totals.map((t, i) => (
          <div key={t.variant.id} className="dash-tile">
            <div className="dash-tile-name">
              <span className="dash-swatch" style={{ background: SERIES[i] }} />
              {t.variant.name}
            </div>
            {METRICS.map((m) => {
              const v = t.metrics[m.key];
              const b = totals[0].metrics[m.key];
              const delta = i > 0 && v != null && b != null && b !== 0 ? (v - b) / b : null;
              const better = delta == null ? null : m.lowerIsBetter ? delta < 0 : delta > 0;
              return (
                <div key={m.key} className="dash-stat">
                  <span className="dash-stat-label">{m.title}</span>
                  <span className="dash-stat-value">{v == null ? "—" : m.fmt(v)}</span>
                  {delta != null && Math.abs(delta) >= 0.005 && (
                    <span className={`dash-delta ${better ? "good" : "worse"}`}>
                      {delta < 0 ? "▼" : "▲"} {Math.round(Math.abs(delta) * 100)}%
                    </span>
                  )}
                </div>
              );
            })}
            <div className="dash-stat">
              <span className="dash-stat-label">Findings</span>
              <span className="dash-stat-value dash-small">
                {t.pillars} pillar · {t.code} code
              </span>
            </div>
          </div>
        ))}
      </div>

      <div className="dash-charts">
        {perPr.map(({ metric, values }) => (
          <GroupedBars
            key={metric.key}
            title={metric.title}
            hint={metric.hint}
            groups={exp.prIds.map(label)}
            series={variants.map((v) => v.name)}
            values={values}
            fmt={metric.fmt}
            ticks={metric.ticks}
            onHover={showTip}
            onLeave={() => setTip(null)}
          />
        ))}
      </div>

      <div className="dash-severity">
        <div className="dash-chart-title">
          Pillar findings by severity
          <span className="dash-muted"> · over the comparable PRs</span>
        </div>
        {totals.map((t) => {
          const total = t.bySeverity.reduce((a, b) => a + b, 0);
          return (
            <div key={t.variant.id} className="dash-sev-row">
              <span className="dash-sev-name">{t.variant.name}</span>
              <div className="dash-sev-bar">
                {total === 0 && <span className="dash-muted">none</span>}
                {SEVERITY.map((s, i) =>
                  t.bySeverity[i] > 0 ? (
                    <span
                      key={s.key}
                      className="dash-sev-seg"
                      style={{ flex: t.bySeverity[i], background: s.color }}
                      onMouseMove={(e) => showTip(e, [`${t.variant.name}`, `${s.key}: ${t.bySeverity[i]}`])}
                      onMouseLeave={() => setTip(null)}
                    >
                      {t.bySeverity[i] >= 2 && <span className="dash-sev-n">{t.bySeverity[i]}</span>}
                    </span>
                  ) : null,
                )}
              </div>
              <span className="dash-muted dash-sev-total">{total}</span>
            </div>
          );
        })}
        <div className="dash-legend dash-sev-legend">
          {SEVERITY.map((s) => (
            <span key={s.key} className="dash-key">
              <span className="dash-swatch" style={{ background: s.color }} />
              {s.key}
            </span>
          ))}
        </div>
      </div>

      {tip &&
        // Portalled to the body: a transformed ancestor would otherwise
        // become the reference for fixed positioning and drag it off the pointer.
        createPortal(
          <div className="dash-tip" style={{ left: tip.x, top: tip.y }}>
            {tip.lines.map((l, i) => (
              <div key={i}>{l}</div>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

/** Grouped columns: one group per PR, one column per variant in fixed
 *  order. Thin marks, rounded at the data end, square on the baseline,
 *  2px of surface between neighbours, three recessive gridlines. */
function GroupedBars({
  title,
  hint,
  groups,
  series,
  values,
  fmt,
  ticks: tickStyle,
  onHover,
  onLeave,
}: {
  title: string;
  hint: string;
  groups: string[];
  series: string[];
  values: (number | null)[][];
  fmt: (n: number) => string;
  ticks: "time" | "number";
  onHover: (e: React.MouseEvent, lines: string[]) => void;
  onLeave: () => void;
}) {
  // Drawn at real pixels, not scaled: a column is at most 24px wide no
  // matter how wide the pane is.
  const [host, W] = useWidth(440);
  const H = 190;
  const pad = { l: 48, r: 8, t: 10, b: 26 };
  const plotW = W - pad.l - pad.r;
  const plotH = H - pad.t - pad.b;
  const max = Math.max(1e-9, ...values.flat().map((v) => v ?? 0)) * 1.12;
  const ticks = tickStyle === "time" ? timeTicks(max) : niceTicks(max, 3);
  const labelChars = Math.max(8, Math.floor((plotW / Math.max(1, groups.length)) / 6.5));
  const y = (v: number) => pad.t + plotH - (v / max) * plotH;
  const groupW = plotW / Math.max(1, groups.length);
  const slot = (groupW * 0.82) / Math.max(1, series.length);
  const bar = Math.min(24, Math.max(4, slot - 2));

  return (
    <div className="dash-chart" ref={host}>
      <div className="dash-chart-title">
        {title}
        <span className="dash-muted"> · {hint}</span>
      </div>
      <svg width={W} height={H} role="img" aria-label={`${title} by PR and variant`}>
        {ticks.map((t) => (
          <g key={t}>
            <line x1={pad.l} x2={W - pad.r} y1={y(t)} y2={y(t)} className="dash-grid" />
            <text x={pad.l - 6} y={y(t) + 3} className="dash-axis" textAnchor="end">
              {fmt(t)}
            </text>
          </g>
        ))}
        <line x1={pad.l} x2={W - pad.r} y1={y(0)} y2={y(0)} className="dash-baseline" />
        {groups.map((g, gi) => {
          const gx = pad.l + gi * groupW + (groupW - slot * series.length) / 2;
          return (
            <g key={g}>
              {series.map((name, si) => {
                const v = values[si]?.[gi];
                const x = gx + si * slot + (slot - bar) / 2;
                if (v == null)
                  return (
                    <text key={name} x={x + bar / 2} y={y(0) - 4} className="dash-axis" textAnchor="middle">
                      –
                    </text>
                  );
                const top = y(v);
                const base = y(0);
                const r = Math.min(4, Math.max(0, (base - top) / 2));
                const d = `M${x},${base} L${x},${top + r} Q${x},${top} ${x + r},${top} L${x + bar - r},${top} Q${x + bar},${top} ${x + bar},${top + r} L${x + bar},${base} Z`;
                return (
                  <path
                    key={name}
                    d={d}
                    fill={SERIES[si]}
                    className="dash-bar"
                    onMouseMove={(e) => onHover(e, [g, `${name}: ${fmt(v)}`])}
                    onMouseLeave={onLeave}
                  />
                );
              })}
              <text x={pad.l + gi * groupW + groupW / 2} y={H - 8} className="dash-axis" textAnchor="middle">
                {g.length > labelChars ? `${g.slice(0, labelChars - 1)}…` : g}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

/** Measure the host element; re-render on resize. */
function useWidth(fallback: number): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(fallback);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setW(Math.max(240, Math.floor(entry.contentRect.width))));
    ro.observe(el);
    setW(Math.max(240, Math.floor(el.getBoundingClientRect().width)));
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

/** Gridlines for a millisecond axis, stepping on the clock. */
function timeTicks(maxMs: number): number[] {
  const steps = [5, 10, 15, 30, 60, 90, 120, 180, 300, 600, 900, 1800].map((s) => s * 1000);
  const raw = maxMs / 3;
  const step = steps.find((s) => s >= raw) ?? steps[steps.length - 1];
  const out: number[] = [];
  for (let v = step; v <= maxMs; v += step) out.push(v);
  return out;
}

function niceTicks(max: number, count: number): number[] {
  const raw = max / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
  const out: number[] = [];
  for (let v = step; v <= max; v += step) out.push(Number(v.toFixed(6)));
  return out;
}
