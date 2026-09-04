import { create } from "zustand";
import { ipc } from "../lib/ipc";
import type { Explainable } from "../lib/comments";
import type { C4Node } from "../bindings/C4Node";

/** One PR's raw diff + viewed-file digests, shared between the file rail and
 *  the Diff tab so opening a PR fetches from GitHub once, not per consumer. */
export interface DiffEntry {
  headSha: string;
  status: "loading" | "done" | "error";
  raw: string | null;
  error: string | null;
  /** path → digest of the patch that was marked viewed */
  viewed: Record<string, string>;
}

/** One-shot "open a pre-filled comment composer" request, e.g. from an
 *  assessment finding. Lands on the file's first changed line when a diff
 *  path resolved, else in the PR conversation composer. */
export interface ComposeRequest {
  target: "diff" | "conversation";
  path: string | null;
  /** Exact new-side line to anchor on; falls back to the first changed line. */
  line?: number | null;
  seed: string;
  /** Appended to the body on submit, never shown in the draft: an HTML
   *  comment GitHub renders as nothing, by which a finding with no file or
   *  line of its own is later recognised as commented. */
  marker?: string;
}

/** One-shot "explain this finding in the assistant chat" request, raised from a
 *  finding row — a code finding, a Well-Architected finding, or a boundary
 *  impact. `MainApp` opens the assistant panel; `AssistantPanel` sends the
 *  seeded prompt and switches to the chat. Carries the PR id so a stale request
 *  can't fire against a different PR's panel. */
export interface ExplainRequest {
  prId: string;
  finding: Explainable;
}

/** A canvas node whose diff the assistant panel is showing — the "code peek"
 *  raised by clicking a node on the Architecture tab. Stays put until closed
 *  (or the PR changes), so it survives a switch between panel views. */
export interface CodePeek {
  prId: string;
  node: C4Node;
}

interface DiffState {
  entries: Record<string, DiffEntry>;
  /** One-shot "scroll the Diff tab to this file" request from the file rail. */
  focusPath: string | null;
  composeRequest: ComposeRequest | null;
  explainRequest: ExplainRequest | null;
  peek: CodePeek | null;
  /** File currently at the top of the Diff tab's viewport — the file rail
   *  highlights it and keeps it in view (scroll spy). */
  visiblePath: string | null;
  ensure: (prId: string, headSha: string) => Promise<void>;
  setViewed: (prId: string, path: string, digest: string, viewed: boolean) => void;
  requestFocusFile: (path: string) => void;
  clearFocusFile: () => void;
  requestCompose: (req: ComposeRequest) => void;
  clearCompose: () => void;
  requestExplain: (prId: string, finding: Explainable) => void;
  clearExplain: () => void;
  openPeek: (prId: string, node: C4Node) => void;
  closePeek: () => void;
  setVisiblePath: (path: string | null) => void;
  /** Org switch: drop every cached diff and transient focus state. */
  reset: () => void;
}

export const useDiffStore = create<DiffState>((set, get) => ({
  entries: {},
  focusPath: null,
  composeRequest: null,
  explainRequest: null,
  peek: null,
  visiblePath: null,

  reset: () =>
    set({
      entries: {},
      focusPath: null,
      composeRequest: null,
      explainRequest: null,
      peek: null,
      visiblePath: null,
    }),

  ensure: async (prId, headSha) => {
    const existing = get().entries[prId];
    if (existing && existing.headSha === headSha && existing.status !== "error") return;
    set((s) => ({
      entries: {
        ...s.entries,
        [prId]: {
          headSha,
          status: "loading",
          raw: null,
          error: null,
          viewed: existing?.viewed ?? {},
        },
      },
    }));
    try {
      const [raw, viewedRows] = await Promise.all([
        ipc.getPrDiff(prId),
        ipc.getViewedFiles(prId),
      ]);
      if (get().entries[prId]?.headSha !== headSha) return; // superseded by a newer head
      set((s) => ({
        entries: {
          ...s.entries,
          [prId]: {
            headSha,
            status: "done",
            raw,
            error: null,
            viewed: Object.fromEntries(viewedRows.map((r) => [r.path, r.digest])),
          },
        },
      }));
    } catch (e) {
      if (get().entries[prId]?.headSha !== headSha) return;
      set((s) => ({
        entries: {
          ...s.entries,
          [prId]: { headSha, status: "error", raw: null, error: String(e), viewed: {} },
        },
      }));
    }
  },

  setViewed: (prId, path, digest, viewed) => {
    set((s) => {
      const entry = s.entries[prId];
      if (!entry) return s;
      const next = { ...entry.viewed };
      if (viewed) next[path] = digest;
      else delete next[path];
      return { entries: { ...s.entries, [prId]: { ...entry, viewed: next } } };
    });
    void ipc.setFileViewed(prId, path, digest, viewed);
  },

  requestFocusFile: (path) => set({ focusPath: path }),
  clearFocusFile: () => set({ focusPath: null }),
  requestCompose: (req) => set({ composeRequest: req }),
  clearCompose: () => set({ composeRequest: null }),
  requestExplain: (prId, finding) => set({ explainRequest: { prId, finding } }),
  clearExplain: () => set({ explainRequest: null }),
  openPeek: (prId, node) => set({ peek: { prId, node } }),
  closePeek: () => set({ peek: null }),
  setVisiblePath: (path) => set({ visiblePath: path }),
}));
