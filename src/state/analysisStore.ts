import { create } from "zustand";
import type { AnalysisErrorKind } from "../bindings/AnalysisErrorKind";
import type { AnalysisLevel } from "../bindings/AnalysisLevel";
import type { AnalysisResult } from "../bindings/AnalysisResult";
import {
  ipc,
  onAnalysisComplete,
  onAnalysisError,
  onAnalysisProgress,
} from "../lib/ipc";

export const analysisKey = (prId: string, level: AnalysisLevel, focus?: string) =>
  `${prId}:${level}:${focus ?? ""}`;

export interface ProgressStep {
  at: string;
  message: string;
}

interface Run {
  status: "idle" | "running" | "done" | "error";
  progress: ProgressStep[];
  result?: AnalysisResult;
  error?: string;
  errorKind?: AnalysisErrorKind;
}

interface AnalysisState {
  runs: Record<string, Run>;
  init: () => Promise<void>;
  /** Load from cache; if absent and start=true, kick off a run. */
  ensure: (prId: string, level: AnalysisLevel, focus?: string, start?: boolean) => Promise<void>;
  start: (prId: string, level: AnalysisLevel, focus?: string) => Promise<void>;
}

let initialized = false;

export const useAnalysisStore = create<AnalysisState>((set, get) => ({
  runs: {},

  init: async () => {
    if (initialized) return;
    initialized = true;
    await onAnalysisProgress((p) => {
      const key = analysisKey(p.prId, p.level);
      set((s) => {
        const run = s.runs[key] ?? { status: "running", progress: [] };
        return {
          runs: {
            ...s.runs,
            [key]: {
              ...run,
              status: "running",
              progress: [
                ...run.progress.slice(-400),
                { at: new Date().toISOString(), message: p.message },
              ],
            },
          },
        };
      });
    });
    await onAnalysisComplete((r) => {
      const key = analysisKey(r.prId, r.level, r.focusNodeId ?? undefined);
      const plain = analysisKey(r.prId, r.level);
      set((s) => ({
        runs: {
          ...s.runs,
          [key]: { status: "done", progress: [], result: r },
          ...(key !== plain ? { [plain]: { status: "done", progress: [], result: r } } : {}),
        },
      }));
    });
    await onAnalysisError((e) => {
      const key = analysisKey(e.prId, e.level);
      set((s) => ({
        runs: {
          ...s.runs,
          [key]: { status: "error", progress: [], error: e.error, errorKind: e.kind },
        },
      }));
    });
  },

  ensure: async (prId, level, focus, start = false) => {
    const key = analysisKey(prId, level, focus);
    const existing = get().runs[key];
    if (existing && existing.status !== "idle" && existing.status !== "error") return;
    const cached = await ipc.getAnalysis(prId, level, focus);
    if (cached) {
      set((s) => ({
        runs: { ...s.runs, [key]: { status: "done", progress: [], result: cached } },
      }));
      return;
    }
    if (start) await get().start(prId, level, focus);
  },

  start: async (prId, level, focus) => {
    const key = analysisKey(prId, level, focus);
    set((s) => ({
      runs: {
        ...s.runs,
        [key]: {
          status: "running",
          progress: [{ at: new Date().toISOString(), message: "starting analysis" }],
        },
      },
    }));
    try {
      await ipc.runAnalysis(prId, level, focus);
    } catch (e) {
      set((s) => ({
        runs: { ...s.runs, [key]: { status: "error", progress: [], error: String(e) } },
      }));
    }
  },
}));
