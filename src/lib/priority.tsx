import type { RepoPriority } from "../bindings/RepoPriority";
import type { PrPriority } from "../bindings/PrPriority";

/** Ascending height/width bars; the first `filled` are solid, the rest hollow. */
function MagnitudeBars({ filled }: { filled: 1 | 2 | 3 | 4 | 5 }) {
  const bars = [
    { x: 0, y: 8, w: 2, h: 6 },
    { x: 3.5, y: 6, w: 2, h: 8 },
    { x: 7, y: 4, w: 2, h: 10 },
    { x: 10.5, y: 2, w: 2, h: 12 },
    { x: 14, y: 0, w: 2, h: 14 },
  ];
  return (
    <svg viewBox="0 0 16 14" width="16" height="14" aria-hidden="true">
      {bars.map((bar, i) =>
        i < filled ? (
          <rect
            key={i}
            x={bar.x}
            y={bar.y}
            width={bar.w}
            height={bar.h}
            fill="currentColor"
          />
        ) : (
          <rect
            key={i}
            x={bar.x}
            y={bar.y}
            width={bar.w}
            height={bar.h}
            fill="none"
            stroke="currentColor"
            strokeOpacity={0.4}
          />
        ),
      )}
    </svg>
  );
}

/** Same 5-bar silhouette, entirely hollow, struck through — reads as off-scale, not "0 bars". */
function IgnoredMark() {
  const bars = [
    { x: 0, y: 8, w: 2, h: 6 },
    { x: 3.5, y: 6, w: 2, h: 8 },
    { x: 7, y: 4, w: 2, h: 10 },
    { x: 10.5, y: 2, w: 2, h: 12 },
    { x: 14, y: 0, w: 2, h: 14 },
  ];
  return (
    <svg viewBox="0 0 16 14" width="16" height="14" aria-hidden="true">
      {bars.map((bar, i) => (
        <rect
          key={i}
          x={bar.x}
          y={bar.y}
          width={bar.w}
          height={bar.h}
          fill="none"
          stroke="currentColor"
          strokeOpacity={0.4}
        />
      ))}
      <line
        x1="0"
        y1="14"
        x2="16"
        y2="0"
        stroke="currentColor"
        strokeWidth="1"
      />
    </svg>
  );
}

/** Attention-ascending, matching the backend enum's declaration order. */
export const REPO_PRIORITY_ORDER: RepoPriority[] = [
  "ignored",
  "unimportant",
  "someday",
  "standard",
  "important",
  "critical",
];

/** Attention-ascending. Has no `ignored` entry — that's a repo-only concept. */
export const PR_PRIORITY_ORDER: PrPriority[] = [
  "unimportant",
  "someday",
  "standard",
  "important",
  "critical",
];

/**
 * Both records share one numeric scale so cross-dimension arithmetic (e.g.
 * combining a PR's own priority with its repo's) stays consistent — a PR's
 * `important` and a repo's `important` carry the same weight.
 */
export const REPO_PRIORITY_WEIGHT: Record<RepoPriority, number> = {
  ignored: 0,
  unimportant: 1,
  someday: 2,
  standard: 3,
  important: 4,
  critical: 5,
};

export const PR_PRIORITY_WEIGHT: Record<PrPriority, number> = {
  unimportant: 1,
  someday: 2,
  standard: 3,
  important: 4,
  critical: 5,
};

export const REPO_PRIORITY_LABEL: Record<RepoPriority, string> = {
  ignored: "Ignored",
  unimportant: "Unimportant",
  someday: "Someday",
  standard: "Standard",
  important: "Important",
  critical: "Critical",
};

export const PR_PRIORITY_LABEL: Record<PrPriority, string> = {
  unimportant: "Unimportant",
  someday: "Someday",
  standard: "Standard",
  important: "Important",
  critical: "Critical",
};

export const REPO_PRIORITY_TOOLTIP: Record<RepoPriority, string> = {
  ignored: "Never tracked or surfaced.",
  unimportant: "Tracked, but never raises a notification.",
  someday: "Low attention — worth a glance now and then.",
  standard: "Default attention level.",
  important: "Elevated attention — surfaced in activity.",
  critical: "Highest attention — always notifies.",
};

export const PR_PRIORITY_TOOLTIP: Record<PrPriority, string> = {
  unimportant: "Never raises a notification.",
  someday: "Low attention — worth a glance now and then.",
  standard: "Default attention level.",
  important: "Elevated attention — surfaced in activity.",
  critical: "Highest attention — always notifies.",
};

/** UI-only: attention-descending, for rendering menus top-to-bottom. Never use for sorting/weighting. */
export const REPO_PRIORITY_DISPLAY_ORDER: RepoPriority[] = [
  ...REPO_PRIORITY_ORDER,
].reverse();

/** UI-only: attention-descending, for rendering menus top-to-bottom. Never use for sorting/weighting. */
export const PR_PRIORITY_DISPLAY_ORDER: PrPriority[] = [
  ...PR_PRIORITY_ORDER,
].reverse();

export const REPO_PRIORITY_ICON: Record<
  RepoPriority,
  { icon: React.ReactNode; label: string }
> = {
  ignored: { icon: <IgnoredMark />, label: REPO_PRIORITY_LABEL.ignored },
  unimportant: {
    icon: <MagnitudeBars filled={1} />,
    label: REPO_PRIORITY_LABEL.unimportant,
  },
  someday: {
    icon: <MagnitudeBars filled={2} />,
    label: REPO_PRIORITY_LABEL.someday,
  },
  standard: {
    icon: <MagnitudeBars filled={3} />,
    label: REPO_PRIORITY_LABEL.standard,
  },
  important: {
    icon: <MagnitudeBars filled={4} />,
    label: REPO_PRIORITY_LABEL.important,
  },
  critical: {
    icon: <MagnitudeBars filled={5} />,
    label: REPO_PRIORITY_LABEL.critical,
  },
};

export const PR_PRIORITY_ICON: Record<
  PrPriority,
  { icon: React.ReactNode; label: string }
> = {
  unimportant: {
    icon: <MagnitudeBars filled={1} />,
    label: PR_PRIORITY_LABEL.unimportant,
  },
  someday: {
    icon: <MagnitudeBars filled={2} />,
    label: PR_PRIORITY_LABEL.someday,
  },
  standard: {
    icon: <MagnitudeBars filled={3} />,
    label: PR_PRIORITY_LABEL.standard,
  },
  important: {
    icon: <MagnitudeBars filled={4} />,
    label: PR_PRIORITY_LABEL.important,
  },
  critical: {
    icon: <MagnitudeBars filled={5} />,
    label: PR_PRIORITY_LABEL.critical,
  },
};

export function isDefaultRepoPriority(p?: RepoPriority): boolean {
  return p === undefined || p === "standard";
}
