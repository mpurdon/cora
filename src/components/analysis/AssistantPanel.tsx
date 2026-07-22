import { useEffect, useRef, useState } from "react";
import type { ChatContext } from "../../bindings/ChatContext";
import type { ChatPendingAction } from "../../bindings/ChatPendingAction";
import type { TrackedPr } from "../../bindings/TrackedPr";
import { analysisKey, useAnalysisStore } from "../../state/analysisStore";
import { useChatStore } from "../../state/chatStore";
import { CommentBody } from "./CommentsView";
import { ContextDrawer } from "./ContextDrawer";
import { FileInsights } from "./FileInsights";
import { formatTokens, TraceSteps } from "./TraceSteps";
import { IconArrowUp } from "../icons";

/** Right-hand assistant panel: the analysis run's activity plus a
 *  conversation grounded in it. Mutating actions surface as confirm cards.
 *  On the Diff tab a second view, File insights, follows the file in view. */
export function AssistantPanel({
  pr,
  width,
  insightsEnabled,
  onClose,
}: {
  pr: TrackedPr;
  width?: number;
  insightsEnabled: boolean;
  onClose: () => void;
}) {
  const [view, setView] = useState<"chat" | "insights">("chat");
  // Leaving the Diff tab pulls the panel back to the chat, its default.
  useEffect(() => {
    if (!insightsEnabled) setView("chat");
  }, [insightsEnabled]);
  const session = useChatStore((s) => s.sessions[pr.id]);
  const context = useChatStore((s) => s.contexts[pr.id]);
  const init = useChatStore((s) => s.init);
  const load = useChatStore((s) => s.load);
  const loadContext = useChatStore((s) => s.loadContext);
  const send = useChatStore((s) => s.send);
  const confirm = useChatStore((s) => s.confirm);
  const clear = useChatStore((s) => s.clear);
  const [showContext, setShowContext] = useState(false);

  useEffect(() => {
    void init().then(() => load(pr.id));
  }, [pr.id, init, load]);

  // The L1 analysis run's steps — live during a run, the trace afterwards.
  const run = useAnalysisStore((s) => s.runs[analysisKey(pr.id, "context")]);

  // Chat events keep the running total live (see chatStore); this covers the
  // rest: opening the panel, a new head, and — keyed on the analysis itself
  // rather than trusting the event that announces it — a new assessment
  // landing, which rewrites the system prompt without touching the
  // conversation.
  useEffect(() => {
    void loadContext(pr.id).catch(() => {});
  }, [pr.id, pr.headSha, run?.result?.createdAt, run?.status, loadContext]);
  const live = run?.status === "running";
  const steps = live
    ? run.progress.map((p) => ({ at: p.at, kind: "status", message: p.message }))
    : (run?.result?.trace ?? []);
  const usage = run?.status === "done" ? run.result?.usage : undefined;
  const [showActivity, setShowActivity] = useState(false);

  const [draft, setDraft] = useState("");
  const [sendError, setSendError] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const activityRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [session?.items.length, session?.busy, session?.pending]);
  useEffect(() => {
    if (live) activityRef.current?.scrollTo({ top: activityRef.current.scrollHeight });
  }, [steps.length, live]);

  const canSend = !!draft.trim() && !session?.busy && !session?.pending;
  const submit = () => {
    if (!canSend) return;
    const text = draft.trim();
    setDraft("");
    setSendError(null);
    void send(pr.id, text).catch((e) => setSendError(String(e)));
  };

  return (
    <aside className="assistant-panel" style={width ? { width } : undefined}>
      <header className="assistant-header">
        <div className="panel-switch" role="tablist" aria-label="Panel view">
          <button
            role="tab"
            aria-selected={view === "chat"}
            className={view === "chat" ? "on" : ""}
            onClick={() => setView("chat")}
          >
            {(live || session?.busy) && <span className="sync-dot live" />}
            Assistant
          </button>
          <button
            role="tab"
            aria-selected={view === "insights"}
            className={view === "insights" ? "on" : ""}
            disabled={!insightsEnabled}
            title={insightsEnabled ? undefined : "Open the Diff tab to see file insights"}
            onClick={() => setView("insights")}
          >
            File insights
          </button>
        </div>
        <span className="eyebrow">#{pr.number}</span>
        <span className="spacer" />
        {view === "chat" && (session?.items.length ?? 0) > 0 && (
          <button
            className="icon-btn"
            title="Clear this conversation"
            onClick={() => void clear(pr.id)}
          >
            ↺
          </button>
        )}
        <button className="icon-btn" title="Close panel" onClick={onClose}>
          ✕
        </button>
      </header>

      {view === "insights" ? (
        <FileInsights pr={pr} />
      ) : (
        <>
      <div className="assistant-activity">
        <button
          className="activity-toggle eyebrow"
          onClick={() => setShowActivity((s) => !s)}
          aria-expanded={showActivity}
        >
          <span className="chevron">{showActivity ? "▾" : "▸"}</span>
          analysis activity
          {live && <span className="sync-dot live" />}
          {steps.length > 0 && <span className="group-count">{steps.length}</span>}
          {usage && usage.turns > 0 && (
            <span className="drawer-usage mono">
              {usage.turns} turns · {formatTokens(usage.inputTokens)} in /{" "}
              {formatTokens(usage.outputTokens)} out
            </span>
          )}
        </button>
        {showActivity && (
          <div className="activity-steps" ref={activityRef}>
            {steps.length === 0 && <div className="drawer-empty">no analysis run yet</div>}
            <TraceSteps steps={steps} />
          </div>
        )}
      </div>

      <div className="chat-list" ref={listRef}>
        {(session?.items.length ?? 0) === 0 && (
          <div className="chat-empty">
            <p>
              Ask about this PR — the analysis, its research, and the diff stay in context.
            </p>
            <p>
              The assistant can also comment, reply, resolve threads, or submit your review.
              Every action asks you first and lands in your history.
            </p>
          </div>
        )}
        {session?.items.map((item, i) => {
          switch (item.kind) {
            case "user":
              return (
                <div key={i} className="chat-user">
                  {item.text}
                </div>
              );
            case "assistant":
              return (
                <div key={i} className="chat-assistant">
                  <CommentBody body={item.text} />
                </div>
              );
            case "tool":
              return (
                <div key={i} className="chat-tool mono">
                  ⚒ {item.text}
                </div>
              );
            case "action":
              return (
                <div key={i} className="chat-action mono">
                  {item.text}
                </div>
              );
            default:
              return (
                <div key={i} className="chat-error mono">
                  {item.text}
                </div>
              );
          }
        })}
        {session?.busy && (
          <div className="chat-tool mono">
            <span className="sync-dot live" /> working…
          </div>
        )}
        {session?.pending && (
          <PendingCard
            key={session.pending.summary}
            pending={session.pending}
            onConfirm={(approve, edited) => void confirm(pr.id, approve, edited)}
          />
        )}
      </div>

      <div className="chat-composer">
        {sendError && <div className="settings-error">{sendError}</div>}
        <textarea
          placeholder={
            session?.pending ? "Confirm the action above first…" : "Ask about this PR…"
          }
          value={draft}
          disabled={!!session?.pending}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <button
          className="chat-send"
          disabled={!canSend}
          title="Send  (Enter — Shift+Enter for a new line)"
          onClick={submit}
        >
          <IconArrowUp />
        </button>
      </div>

      <ContextMeter context={context} onOpen={() => setShowContext(true)} />
      {showContext && (
        <ContextDrawer prId={pr.id} onClose={() => setShowContext(false)} />
      )}
        </>
      )}
    </aside>
  );
}

/** The confirm card. Text-carrying actions are editable in place — the text
 *  shown is exactly what will post, so what you send is what you approved,
 *  not the model's draft of it. */
function PendingCard({
  pending,
  onConfirm,
}: {
  pending: ChatPendingAction;
  onConfirm: (approve: boolean, edited?: string) => void;
}) {
  const [draft, setDraft] = useState(pending.detail);
  const edited = draft !== pending.detail;
  const empty = pending.editable && !draft.trim();
  return (
    <div className="pending-action">
      <div className="pending-title">
        {pending.summary}
        {edited && <span className="pending-edited">edited</span>}
      </div>
      {pending.editable ? (
        <textarea
          className="pending-edit"
          value={draft}
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
        />
      ) : (
        pending.detail && <pre className="pending-detail">{pending.detail}</pre>
      )}
      <div className="row">
        <button
          className="action-btn btn-ok"
          disabled={empty}
          title={empty ? "This action needs text" : undefined}
          onClick={() => onConfirm(true, edited ? draft : undefined)}
        >
          {edited ? "Run my version" : "Run it"}
        </button>
        <button className="action-btn" onClick={() => onConfirm(false)}>
          Don't
        </button>
      </div>
    </div>
  );
}

/** Running total of what the next turn will carry, under the composer. It
 *  moves as the conversation does: each tool result, reply and finished
 *  analysis is announced, and the store refetches (debounced). */
function ContextMeter({
  context,
  onOpen,
}: {
  context: ChatContext | undefined;
  onOpen: () => void;
}) {
  if (!context) return null;
  // Projected, not measured: the last request's cost excludes everything
  // added since (its own reply, the tool results it triggered).
  const total = context.projectedTokens;
  const window = context.windowTokens;
  const pct = window > 0 ? Math.min(100, (total / window) * 100) : null;
  return (
    <button
      className="context-meter"
      onClick={onOpen}
      title="See exactly what is in the assistant's context"
    >
      <span className="eyebrow">context</span>
      <span className="mono context-meter-total">≈{formatTokens(total)}</span>
      {pct != null && (
        <>
          <span className="context-meter-bar">
            <span className={`fill${pct > 80 ? " hot" : ""}`} style={{ width: `${pct}%` }} />
          </span>
          <span className="mono context-meter-pct">{Math.round(pct)}%</span>
        </>
      )}
    </button>
  );
}
