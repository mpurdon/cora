import type { RepoPriority } from "../bindings/RepoPriority";
import type { PrPriority } from "../bindings/PrPriority";

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
  { icon: string; label: string }
> = {
  ignored: { icon: "⊘", label: REPO_PRIORITY_LABEL.ignored },
  unimportant: { icon: "○", label: REPO_PRIORITY_LABEL.unimportant },
  someday: { icon: "◔", label: REPO_PRIORITY_LABEL.someday },
  standard: { icon: "◑", label: REPO_PRIORITY_LABEL.standard },
  important: { icon: "◕", label: REPO_PRIORITY_LABEL.important },
  critical: { icon: "●", label: REPO_PRIORITY_LABEL.critical },
};

export const PR_PRIORITY_ICON: Record<
  PrPriority,
  { icon: string; label: string }
> = {
  unimportant: { icon: "○", label: PR_PRIORITY_LABEL.unimportant },
  someday: { icon: "◔", label: PR_PRIORITY_LABEL.someday },
  standard: { icon: "◑", label: PR_PRIORITY_LABEL.standard },
  important: { icon: "◕", label: PR_PRIORITY_LABEL.important },
  critical: { icon: "●", label: PR_PRIORITY_LABEL.critical },
};

export function isDefaultRepoPriority(p?: RepoPriority): boolean {
  return p === undefined || p === "standard";
}
