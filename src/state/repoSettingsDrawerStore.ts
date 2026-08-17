import { create } from "zustand";

interface RepoSettingsDrawerState {
  isOpen: boolean;
  repoFullName: string | null;
  openFor: (repoFullName: string) => void;
  close: () => void;
}

export const useRepoSettingsDrawerStore = create<RepoSettingsDrawerState>((set) => ({
  isOpen: false,
  repoFullName: null,
  openFor: (repoFullName) => set({ isOpen: true, repoFullName }),
  close: () => set({ isOpen: false }),
}));
