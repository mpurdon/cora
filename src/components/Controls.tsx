import type { ReactNode } from "react";
import type React from "react";

/** Form controls shared by the settings panes and the experiment editor. */

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="toggle-row">
      <input
        type="checkbox"
        disabled={disabled}
        className="toggle-input"
        role="switch"
        aria-checked={checked}
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="toggle-track">
        <span className="toggle-knob" />
      </span>
      {label}
    </label>
  );
}

// Output-token ceiling steps, chosen to land on real Claude-on-Bedrock hard
// output caps: 4k/8k (older Claude), 16k, and 24k–64k (newer models). The
// model behind an inference-profile ARN is opaque, so we can't know which cap
// applies — picking a real value keeps the choice meaningful, and the
// over-cap Bedrock error (engine.rs) is the backstop if a step is too high.
/** Effort levels the Bedrock request accepts, in order; "default" sends
 *  nothing and leaves the model at its own level (high on current tiers). */
export const EFFORT_LEVELS = ["default", "low", "medium", "high", "xhigh", "max"] as const;
export type EffortLevel = (typeof EFFORT_LEVELS)[number];
const EFFORT_STEPS = EFFORT_LEVELS.map((_, i) => i);
const effortIdx = (v: string) => Math.max(0, EFFORT_LEVELS.indexOf((v || "default") as EffortLevel));

/** The effort scale as a step slider, so it reads like the ceilings below it. */
export function EffortSlider({ value, onChange }: { value: string; onChange: (v: EffortLevel) => void }) {
  return (
    <StepSlider
      steps={EFFORT_STEPS}
      value={effortIdx(value)}
      onChange={(i) => onChange(EFFORT_LEVELS[i])}
      fmt={(i) => EFFORT_LEVELS[i]}
    />
  );
}

export const TOKEN_STEPS = [4096, 8192, 16384, 24576, 32768, 49152, 65536];

export function fmtTokens(n: number): string {
  const k = n / 1024;
  return Number.isInteger(k) ? `${k}k` : `${k.toFixed(1)}k`;
}

export function nearestIdx(steps: number[], value: number): number {
  let best = 0;
  steps.forEach((v, i) => {
    if (Math.abs(v - value) < Math.abs(steps[best] - value)) best = i;
  });
  return best;
}

/** A slider over a small set of discrete `steps`, with a tick label under each
 *  value. WKWebView (Tauri's macOS engine) paints the native range thumb at a
 *  position that doesn't track the standard geometry, so external labels can't
 *  be aligned to it. We hide the native thumb and draw our own thumb and labels
 *  from one shared formula — thumb left edge at frac·(track − thumb), label
 *  centre at frac·(track − thumb) + thumb/2 — so they align by construction at
 *  any CSS zoom. The thumb width lives once in CSS as --thumb-w. */
export function StepSlider({
  steps,
  value,
  onChange,
  fmt,
}: {
  steps: number[];
  value: number;
  onChange: (v: number) => void;
  fmt?: (v: number) => string;
}) {
  const fracOf = (i: number) => (steps.length > 1 ? i / (steps.length - 1) : 0);
  const idx = nearestIdx(steps, value);
  // --frac positions the custom thumb; --fill (same ratio, as a %) colours the
  // native track gradient. Both derive from one computation here.
  const style = {
    "--frac": String(fracOf(idx)),
    "--fill": `${fracOf(idx) * 100}%`,
  } as React.CSSProperties;
  return (
    <div className="step-slider" style={style}>
      <input
        type="range"
        className="interval-slider step-slider-input"
        min={0}
        max={steps.length - 1}
        value={idx}
        onChange={(e) => onChange(steps[Number(e.target.value)])}
      />
      <span className="step-slider-thumb" aria-hidden="true" />
      <div className="slider-scale mono">
        {steps.map((v, i) => (
          <span
            key={v}
            style={{ left: `calc(${fracOf(i)} * (100% - var(--thumb-w)) + var(--thumb-w) / 2)` }}
          >
            {fmt ? fmt(v) : v}
          </span>
        ))}
      </div>
    </div>
  );
}
