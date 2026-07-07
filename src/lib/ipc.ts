import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { PollStatus } from "../bindings/PollStatus";
import type { PrChangedEvent } from "../bindings/PrChangedEvent";
import type { Settings } from "../bindings/Settings";
import type { TrackedPr } from "../bindings/TrackedPr";

export const events = {
  prsSnapshot: "prs:snapshot",
  prChanged: "pr:changed",
  pollStatus: "poll:status",
  focusPr: "focus:pr",
} as const;

export const ipc = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  setGithubPat: (token: string) => invoke<void>("set_github_pat", { token }),
  githubPatPresent: () => invoke<boolean>("github_pat_present"),
  clearGithubPat: () => invoke<void>("clear_github_pat"),
  listPrs: () => invoke<TrackedPr[]>("list_prs"),
  markPrRead: (id: string) => invoke<void>("mark_pr_read", { id }),
  setPrMuted: (id: string, muted: boolean) => invoke<void>("set_pr_muted", { id, muted }),
  untrackPr: (id: string) => invoke<void>("untrack_pr", { id }),
  trackPrUrl: (url: string) => invoke<TrackedPr>("track_pr_url", { url }),
  pollNow: () => invoke<void>("poll_now"),
  showMainWindow: (prId?: string) => invoke<void>("show_main_window", { prId: prId ?? null }),
  toggleCallout: () => invoke<void>("toggle_callout"),
};

export function onPrsSnapshot(cb: (prs: TrackedPr[]) => void): Promise<UnlistenFn> {
  return listen<TrackedPr[]>(events.prsSnapshot, (e) => cb(e.payload));
}

export function onPrChanged(cb: (e: PrChangedEvent) => void): Promise<UnlistenFn> {
  return listen<PrChangedEvent>(events.prChanged, (e) => cb(e.payload));
}

export function onPollStatus(cb: (s: PollStatus) => void): Promise<UnlistenFn> {
  return listen<PollStatus>(events.pollStatus, (e) => cb(e.payload));
}

export function onFocusPr(cb: (id: string) => void): Promise<UnlistenFn> {
  return listen<string>(events.focusPr, (e) => cb(e.payload));
}
