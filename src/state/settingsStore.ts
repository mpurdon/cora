import { create } from "zustand";
import type { Settings } from "../bindings/Settings";
import { ipc } from "../lib/ipc";

interface SettingsState {
  settings: Settings | null;
  loaded: boolean;
  refresh: () => Promise<void>;
  save: (patch: Partial<Settings>) => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: null,
  loaded: false,

  refresh: async () => {
    const settings = await ipc.getSettings();
    set({ settings, loaded: true });
  },

  save: async (patch) => {
    const current = get().settings;
    if (!current) return;
    const next = { ...current, ...patch };
    set({ settings: next });
    await ipc.setSettings(next);
  },
}));

let subscribed = false;

/** Subscribe once, from a mounted component. Settings are edited from the
 *  General/Repositories panes and the repo settings drawer — all sharing
 *  this one cache — so a save in one place needs to reach the others without
 *  a restart. A window-focus refresh covers edits made while this cache sat
 *  stale (e.g. a poll-triggered org switch that reloaded settings elsewhere). */
export function initSettingsStore(): void {
  if (subscribed) return;
  subscribed = true;
  void useSettingsStore.getState().refresh();
  window.addEventListener("focus", () => void useSettingsStore.getState().refresh());
}
