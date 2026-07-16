import { create } from "zustand";
import type { ChatItem } from "../bindings/ChatItem";
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
  init: () => Promise<void>;
  /** Hydrate a PR's transcript from the backend (idempotent re-mount). */
  load: (prId: string) => Promise<void>;
  send: (prId: string, text: string) => Promise<void>;
  confirm: (prId: string, approve: boolean) => Promise<void>;
  clear: (prId: string) => Promise<void>;
}

let initialized = false;

export const useChatStore = create<ChatState>((set) => ({
  sessions: {},

  init: async () => {
    if (initialized) return;
    initialized = true;
    await onChatEvent((e) => {
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

  send: (prId, text) => ipc.chatSend(prId, text),
  confirm: (prId, approve) => ipc.chatConfirm(prId, approve),

  clear: async (prId) => {
    await ipc.chatClear(prId);
    set((s) => ({ sessions: { ...s.sessions, [prId]: EMPTY } }));
  },
}));
