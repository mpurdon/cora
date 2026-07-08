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
      // Progress events don't carry the focus id, so fan out to every run of
      // this (pr, level) that is currently running (there's at most one).
      const prefix = `${p.prId}:${p.level}:`;
      const step = { at: new Date().toISOString(), message: p.message };
      set((s) => {
        const next = { ...s.runs };
        let matched = false;
        for (const key of Object.keys(next)) {
          if (key.startsWith(prefix) && next[key].status === "running") {
            next[key] = { ...next[key], progress: [...next[key].progress.slice(-400), step] };
            matched = true;
          }
        }
        if (!matched) {
          const key = analysisKey(p.prId, p.level);
          next[key] = { status: "running", progress: [step] };
        }
        return { runs: next };
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
      // Like progress, error events lack the focus id — fail every running
      // run of this (pr, level).
      const prefix = `${e.prId}:${e.level}:`;
      set((s) => {
        const next = { ...s.runs };
        let matched = false;
        for (const key of Object.keys(next)) {
          if (key.startsWith(prefix) && next[key].status === "running") {
            next[key] = { status: "error", progress: [], error: e.error, errorKind: e.kind };
            matched = true;
          }
        }
        if (!matched) {
          next[analysisKey(e.prId, e.level)] = {
            status: "error",
            progress: [],
            error: e.error,
            errorKind: e.kind,
          };
        }
        return { runs: next };
      });
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
