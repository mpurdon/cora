import { create } from "zustand";
import type { ChatItem } from "../bindings/ChatItem";
import type { ChatContext } from "../bindings/ChatContext";
import type { ChatPendingAction } from "../bindings/ChatPendingAction";
import { ipc, onChatEvent } from "../lib/ipc";

interface SessionView {
  items: ChatItem[];
  busy: boolean;
  pending: ChatPendingAction | null;
}

const EMPTY: SessionView = { items: [], busy: false, pending: null };

interface ChatState {
  sessions: Record<string, SessionView>;
  /** Per-PR context sizes for the panel's running total. Sizes only — the
   *  verbatim text is fetched on demand when the inspector opens. */
  contexts: Record<string, ChatContext>;
  init: () => Promise<void>;
  /** Hydrate a PR's transcript from the backend (idempotent re-mount). */
  load: (prId: string) => Promise<void>;
  loadContext: (prId: string) => Promise<void>;
  send: (prId: string, text: string) => Promise<void>;
  /** `edited` posts the user's rewrite of the action's text instead. */
  confirm: (prId: string, approve: boolean, edited?: string) => Promise<void>;
  clear: (prId: string) => Promise<void>;
  /** Org switch: drop every session view. */
  reset: () => void;
}

let initialized = false;

/** Debounce per PR: a turn emits an event per tool call, result and reply. */
const contextTimers: Record<string, ReturnType<typeof setTimeout>> = {};
function refreshContextSoon(prId: string) {
  clearTimeout(contextTimers[prId]);
  contextTimers[prId] = setTimeout(() => {
    delete contextTimers[prId];
    void useChatStore.getState().loadContext(prId).catch(() => {});
  }, 250);
}

export const useChatStore = create<ChatState>((set) => ({
  sessions: {},
  contexts: {},

  init: async () => {
    if (initialized) return;
    initialized = true;
    await onChatEvent((e) => {
      // Every chat event can have moved the context — a tool result landing,
      // a reply, an analysis finishing. Coalesced because a turn emits several
      // in quick succession and each refetch walks the whole session.
      refreshContextSoon(e.prId);
      set((s) => {
        const prev = s.sessions[e.prId] ?? EMPTY;
        return {
          sessions: {
            ...s.sessions,
            [e.prId]: {
              items: e.item ? [...prev.items, e.item] : prev.items,
              busy: e.busy,
              pending: e.pending ?? null,
            },
          },
        };
      });
    });
  },

  load: async (prId) => {
    const t = await ipc.chatHistory(prId);
    set((s) => ({
      sessions: {
        ...s.sessions,
        [prId]: { items: t.items, busy: t.busy, pending: t.pending ?? null },
      },
    }));
  },

  loadContext: async (prId) => {
    const context = await ipc.getChatContext(prId, false);
    set((s) => ({ contexts: { ...s.contexts, [prId]: context } }));
  },

  send: (prId, text) => ipc.chatSend(prId, text),
  confirm: (prId, approve, edited) => ipc.chatConfirm(prId, approve, edited),

  clear: async (prId) => {
    await ipc.chatClear(prId);
    set((s) => ({ sessions: { ...s.sessions, [prId]: EMPTY } }));
  },

  reset: () => set({ sessions: {}, contexts: {} }),
}));
