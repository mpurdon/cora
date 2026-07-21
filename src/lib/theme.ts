/** Color themes: every chrome color in the app flows through the CSS
 *  variables in :root, so a theme is just a set of overrides applied to
 *  documentElement — changes render live. Custom themes are clones of the
 *  canned ones, persisted in localStorage; the storage event keeps other
 *  windows (the callout) in sync. */

export const COLOR_KEYS = [
  "ink0",
  "ink1",
  "ink2",
  "line",
  "text",
  "muted",
  "idle",
  "ok",
  "warn",
  "bad",
  "chat",
] as const;
export type ColorKey = (typeof COLOR_KEYS)[number];

export const COLOR_LABELS: Record<ColorKey, string> = {
  ink0: "Background",
  ink1: "Panels",
  ink2: "Hover / raised",
  line: "Borders",
  text: "Text",
  muted: "Muted text",
  idle: "Faint / disabled",
  ok: "Positive",
  warn: "Warning",
  bad: "Danger",
  chat: "Accent",
};

const CSS_VAR: Record<ColorKey, string> = {
  ink0: "--ink-0",
  ink1: "--ink-1",
  ink2: "--ink-2",
  line: "--line",
  text: "--text",
  muted: "--muted",
  idle: "--idle",
  ok: "--ok",
  warn: "--warn",
  bad: "--bad",
  chat: "--chat",
};

export interface Theme {
  id: string;
  name: string;
  builtin: boolean;
  colors: Record<ColorKey, string>;
  fonts: {
    /** Application UI font stack. */
    sans: string;
    /** Code / data font stack. */
    mono: string;
  };
}

export const DEFAULT_SANS = '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
export const DEFAULT_MONO = 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace';

const DEFAULT_FONTS = { sans: DEFAULT_SANS, mono: DEFAULT_MONO };

const t = (
  id: string,
  name: string,
  colors: Record<ColorKey, string>,
): Theme => ({ id, name, builtin: true, colors, fonts: { ...DEFAULT_FONTS } });

export const BUILTIN_THEMES: Theme[] = [
  t("cora-dark", "CORA Dark", {
    ink0: "#0e1116", ink1: "#151a21", ink2: "#1d242e", line: "#2a3340",
    text: "#dee4ec", muted: "#77828f", idle: "#3d4754",
    ok: "#3fb68b", warn: "#d9a93f", bad: "#e5534b", chat: "#9d8cff",
  }),
  t("github-dark", "GitHub Dark", {
    ink0: "#0d1117", ink1: "#161b22", ink2: "#21262d", line: "#30363d",
    text: "#e6edf3", muted: "#8b949e", idle: "#484f58",
    ok: "#3fb950", warn: "#d29922", bad: "#f85149", chat: "#a371f7",
  }),
  t("one-dark", "One Dark", {
    ink0: "#21252b", ink1: "#282c34", ink2: "#2c313a", line: "#3e4451",
    text: "#abb2bf", muted: "#5c6370", idle: "#4b5263",
    ok: "#98c379", warn: "#e5c07b", bad: "#e06c75", chat: "#c678dd",
  }),
  t("dracula", "Dracula", {
    ink0: "#21222c", ink1: "#282a36", ink2: "#343746", line: "#44475a",
    text: "#f8f8f2", muted: "#6272a4", idle: "#44475a",
    ok: "#50fa7b", warn: "#ffb86c", bad: "#ff5555", chat: "#bd93f9",
  }),
  t("nord", "Nord", {
    ink0: "#2e3440", ink1: "#353c4a", ink2: "#434c5e", line: "#4c566a",
    text: "#eceff4", muted: "#94a3b8", idle: "#4c566a",
    ok: "#a3be8c", warn: "#ebcb8b", bad: "#bf616a", chat: "#b48ead",
  }),
  t("gruvbox-dark", "Gruvbox Dark", {
    ink0: "#1d2021", ink1: "#282828", ink2: "#3c3836", line: "#504945",
    text: "#ebdbb2", muted: "#928374", idle: "#504945",
    ok: "#b8bb26", warn: "#fabd2f", bad: "#fb4934", chat: "#d3869b",
  }),
  t("catppuccin-mocha", "Catppuccin Mocha", {
    ink0: "#11111b", ink1: "#181825", ink2: "#313244", line: "#45475a",
    text: "#cdd6f4", muted: "#7f849c", idle: "#45475a",
    ok: "#a6e3a1", warn: "#f9e2af", bad: "#f38ba8", chat: "#cba6f7",
  }),
  t("solarized-dark", "Solarized Dark", {
    ink0: "#002b36", ink1: "#073642", ink2: "#0e4250", line: "#29525d",
    text: "#aebfc0", muted: "#7d979c", idle: "#33555e",
    ok: "#859900", warn: "#b58900", bad: "#dc322f", chat: "#6c71c4",
  }),
  t("solarized-light", "Solarized Light", {
    ink0: "#fdf6e3", ink1: "#f3ecd9", ink2: "#e9e2cd", line: "#d4ccb4",
    text: "#073642", muted: "#7d8a8a", idle: "#c9c1a9",
    ok: "#859900", warn: "#b58900", bad: "#dc322f", chat: "#6c71c4",
  }),
  t("github-light", "GitHub Light", {
    ink0: "#ffffff", ink1: "#f6f8fa", ink2: "#eaeef2", line: "#d0d7de",
    text: "#1f2328", muted: "#656d76", idle: "#a8b0b9",
    ok: "#1a7f37", warn: "#9a6700", bad: "#cf222e", chat: "#8250df",
  }),
];

const CUSTOM_KEY = "cora.customThemes";
/// Legacy/global key — doubles as "most recently chosen anywhere", which is
/// what a newly enabled org inherits until it picks its own.
const ACTIVE_KEY = "cora.activeTheme";

/** Theme SELECTION is per-org (the palette library is shared): each org
 *  remembers its theme, and switching orgs re-skins the app. */
let themeOrg: string | null = null;

function orgKey(): string | null {
  return themeOrg ? `${ACTIVE_KEY}.${themeOrg}` : null;
}

/** Call when the active org becomes known or changes — re-applies that
 *  org's remembered theme (falling back to the most recent global choice). */
export function setThemeOrg(org: string) {
  themeOrg = org;
  applyTheme(getTheme(activeThemeId()));
}

export function customThemes(): Theme[] {
  try {
    const raw = JSON.parse(localStorage.getItem(CUSTOM_KEY) ?? "[]") as Theme[];
    return raw.filter((c) => c && c.id && c.colors && c.fonts);
  } catch {
    return [];
  }
}

export function allThemes(): Theme[] {
  return [...BUILTIN_THEMES, ...customThemes()];
}

export function getTheme(id: string): Theme {
  return allThemes().find((th) => th.id === id) ?? BUILTIN_THEMES[0];
}

export function activeThemeId(): string {
  const k = orgKey();
  return (
    (k ? localStorage.getItem(k) : null) ??
    localStorage.getItem(ACTIVE_KEY) ??
    BUILTIN_THEMES[0].id
  );
}

export function applyTheme(theme: Theme) {
  const root = document.documentElement.style;
  for (const key of COLOR_KEYS) root.setProperty(CSS_VAR[key], theme.colors[key]);
  root.setProperty("--sans", theme.fonts.sans || DEFAULT_SANS);
  root.setProperty("--mono", theme.fonts.mono || DEFAULT_MONO);
}

export function setActiveTheme(id: string) {
  const k = orgKey();
  if (k) localStorage.setItem(k, id);
  // The global key tracks the latest choice — the seed for future orgs.
  localStorage.setItem(ACTIVE_KEY, id);
  applyTheme(getTheme(id));
}

/** Persist a custom theme (insert or update) and re-apply if it's active. */
export function saveCustomTheme(theme: Theme) {
  const rest = customThemes().filter((c) => c.id !== theme.id);
  localStorage.setItem(CUSTOM_KEY, JSON.stringify([...rest, { ...theme, builtin: false }]));
  if (activeThemeId() === theme.id) applyTheme(theme);
}

export function deleteCustomTheme(id: string) {
  localStorage.setItem(CUSTOM_KEY, JSON.stringify(customThemes().filter((c) => c.id !== id)));
  if (activeThemeId() === id) setActiveTheme(BUILTIN_THEMES[0].id);
}

export function cloneTheme(src: Theme): Theme {
  const base = `${src.id}-copy`;
  const taken = new Set(allThemes().map((th) => th.id));
  let id = base;
  for (let n = 2; taken.has(id); n++) id = `${base}-${n}`;
  const clone: Theme = {
    id,
    name: `${src.name} (copy)`,
    builtin: false,
    colors: { ...src.colors },
    fonts: { ...src.fonts },
  };
  saveCustomTheme(clone);
  return clone;
}

/** Apply the persisted theme, and follow changes made by other windows. */
export function initTheme() {
  applyTheme(getTheme(activeThemeId()));
  window.addEventListener("storage", (e) => {
    if (e.key === CUSTOM_KEY || e.key?.startsWith(ACTIVE_KEY)) {
      applyTheme(getTheme(activeThemeId()));
    }
  });
}
