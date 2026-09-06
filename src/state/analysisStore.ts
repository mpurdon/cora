import { create } from "zustand";
import type { AnalysisDraft } from "../bindings/AnalysisDraft";
import type { AnalysisErrorKind } from "../bindings/AnalysisErrorKind";
import type { AnalysisLevel } from "../bindings/AnalysisLevel";
import type { AnalysisResult } from "../bindings/AnalysisResult";
import {
  ipc,
  onAnalysisComplete,
  onAnalysisDraft,
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
  /** The assessment as the write-up streams — summary and detail so far. */
  draft?: AnalysisDraft;
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
  /** Org switch: drop every cached run view. */
  reset: () => void;
}

let initialized = false;

export const useAnalysisStore = create<AnalysisState>((set, get) => ({
  runs: {},

  reset: () => set({ runs: {} }),

  init: async () => {
    if (initialized) return;
    initialized = true;
    await onAnalysisProgress((p) => {
      const key = analysisKey(p.prId, p.level, p.focus || undefined);
      const step = { at: new Date().toISOString(), message: p.message };
      set((s) => {
        const run = s.runs[key];
        const next =
          run && run.status === "running"
            ? { ...run, progress: [...run.progress.slice(-400), step] }
            : { status: "running" as const, progress: [step] };
        return { runs: { ...s.runs, [key]: next } };
      });
    });
    await onAnalysisDraft((d) => {
      const key = analysisKey(d.prId, d.level, d.focus || undefined);
      set((s) => {
        const run = s.runs[key];
        if (!run || run.status !== "running") return {};
        return { runs: { ...s.runs, [key]: { ...run, draft: d } } };
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
      const key = analysisKey(e.prId, e.level, e.focus || undefined);
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
      // Explicit starts always rebuild — cached results come via ensure().
      await ipc.runAnalysis(prId, level, focus, true);
    } catch (e) {
      set((s) => ({
        runs: { ...s.runs, [key]: { status: "error", progress: [], error: String(e) } },
      }));
    }
  },
}));
