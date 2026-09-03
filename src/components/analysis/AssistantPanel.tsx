import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { tip } from "../Tooltip";
import type { ChatContext } from "../../bindings/ChatContext";
import type { ChatPendingAction } from "../../bindings/ChatPendingAction";
import type { TrackedPr } from "../../bindings/TrackedPr";
import { analysisKey, useAnalysisStore } from "../../state/analysisStore";
import { useChatStore } from "../../state/chatStore";
import { useDiffStore } from "../../state/diffStore";
import { eli5Prompt } from "../../lib/comments";
import { CommentBody } from "./CommentsView";
import { ContextDrawer } from "./ContextDrawer";
import { DiffPeek } from "./DiffPeek";
import { FileInsights } from "./FileInsights";
import { formatTokens, TraceSteps } from "./TraceSteps";
import { IconArrowUp } from "../icons";

type View = "chat" | "insights" | "code";

/** Right-hand assistant panel: the analysis run's activity plus a
 *  conversation grounded in it. Mutating actions surface as confirm cards.
 *  On the Diff tab a second view, File insights, follows the file in view;
 *  a code node clicked on the Architecture tab adds a third, its diff. */
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
  const [view, setView] = useState<View>("chat");
  const viewRef = useRef(view);
  viewRef.current = view;
  // Leaving the Diff tab pulls file insights back to the chat, its default.
  useEffect(() => {
    if (!insightsEnabled) setView((v) => (v === "insights" ? "chat" : v));
  }, [insightsEnabled]);

  // A code peek takes the panel over while it's open and hands back whatever
  // was showing before when it closes. Another node while peeking just swaps
  // the diff — the view to return to is the one before the first.
  const peek = useDiffStore((s) => s.peek);
  const closePeek = useDiffStore((s) => s.closePeek);
  const peekNode = peek?.prId === pr.id ? peek.node : null;
  const viewBeforePeek = useRef<View>("chat");
  const hadPeek = useRef(false);
  useEffect(() => {
    if (peekNode) {
      if (!hadPeek.current) viewBeforePeek.current = viewRef.current;
      setView("code");
    } else if (hadPeek.current) {
      setView((v) => (v === "code" ? viewBeforePeek.current : v));
    }
    hadPeek.current = !!peekNode;
  }, [peekNode]);
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
  const bodyRef = useRef<HTMLDivElement>(null);
  const activityRef = useRef<HTMLDivElement>(null);

  // Keeping the newest thing in view. Scrolling once, when the store changes,
  // is a snapshot: it misses whatever moves the list afterwards — the context
  // meter landing under the composer, a panel resize, the card's textarea
  // sizing to its text — and it misses entirely when the list isn't mounted
  // at the time (the panel on File insights, or the code peek, which switches
  // over by itself); coming back, nothing has changed, so nothing scrolls,
  // and the card sits below the fold. So the store effect only decides
  // whether to follow, and a ResizeObserver on the list and its content does
  // the scrolling — whenever either changes size, the mount included.
  //
  // Following is the reader's choice: at (or within 80px of) the bottom, new
  // messages pull the view down; scrolled up to read, they don't. A pending
  // action overrides that — it blocks the composer, so its buttons have to
  // be seen.
  const follow = useRef(true);
  const mustShowPending = useRef(false);
  const scrollDown = () => {
    const el = listRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    mustShowPending.current = false;
  };
  const onListScroll = () => {
    const el = listRef.current;
    if (el) follow.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 80;
  };
  useEffect(() => {
    if (session?.pending) mustShowPending.current = true;
    if (follow.current || mustShowPending.current) scrollDown();
  }, [session?.items.length, session?.busy, session?.pending]);
  useEffect(() => {
    const list = listRef.current;
    const body = bodyRef.current;
    if (!list || !body) return;
    const ro = new ResizeObserver(() => {
      if (follow.current || mustShowPending.current) scrollDown();
    });
    ro.observe(list);
    ro.observe(body);
    return () => ro.disconnect();
  }, [view, peekNode]);
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

  // "Explain" on a finding (from either findings view) routes through the diff
  // store so it works when the panel was closed. Send the seeded prompt and
  // surface the chat so the answer lands where the conversation can continue.
  const explainRequest = useDiffStore((s) => s.explainRequest);
  const clearExplain = useDiffStore((s) => s.clearExplain);
  useEffect(() => {
    if (!explainRequest || explainRequest.prId !== pr.id) return;
    setView("chat");
    setSendError(null);
    void send(pr.id, eli5Prompt(explainRequest.finding)).catch((e) => setSendError(String(e)));
    clearExplain();
  }, [explainRequest, pr.id, send, clearExplain]);

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
            data-tip={insightsEnabled ? undefined : "Open the Diff tab to see file insights"}
            onClick={() => setView("insights")}
          >
            File insights
          </button>
          {peekNode && (
            <button
              role="tab"
              aria-selected={view === "code"}
              className={view === "code" ? "on" : ""}
              onClick={() => setView("code")}
            >
              Code
            </button>
          )}
        </div>
        <span className="eyebrow">#{pr.number}</span>
        <span className="spacer" />
        {view === "chat" && (session?.items.length ?? 0) > 0 && (
          <button
            className="icon-btn"
            {...tip("Clear this conversation")}
            onClick={() => void clear(pr.id)}
          >
            ↺
          </button>
        )}
        <button className="icon-btn" {...tip("Close panel")} onClick={onClose}>
          ✕
        </button>
      </header>

      {view === "code" && peekNode ? (
        <DiffPeek prId={pr.id} node={peekNode} onClose={closePeek} />
      ) : view === "insights" ? (
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

      <div className="chat-list" ref={listRef} onScroll={onListScroll}>
        <div className="chat-list-body" ref={bodyRef}>
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
          data-tip="Send  (Enter — Shift+Enter for a new line)"
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
  const [tab, setTab] = useState<"write" | "preview">("write");
  const edited = draft !== pending.detail;
  const empty = pending.editable && !draft.trim();

  // The box grows to the text, so a proposed comment arrives whole — nothing
  // to drag open before you can read what you're approving. Sizing here in a
  // layout effect means it has its final height before the chat list's own
  // effect scrolls to the bottom, landing the card's tail and buttons in view.
  const editRef = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const el = editRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [draft, tab]);

  return (
    <div className="pending-action">
      <div className="pending-title">
        <PendingTitle summary={pending.summary} />
        {edited && <span className="pending-edited">edited</span>}
        {pending.editable && (
          <>
            <span className="spacer" />
            <button
              className={`composer-tab${tab === "write" ? " on" : ""}`}
              onClick={() => setTab("write")}
            >
              Write
            </button>
            <button
              className={`composer-tab${tab === "preview" ? " on" : ""}`}
              disabled={!draft.trim()}
              onClick={() => setTab("preview")}
            >
              Preview
            </button>
          </>
        )}
      </div>
      {!pending.editable ? (
        pending.detail && <pre className="pending-detail">{pending.detail}</pre>
      ) : tab === "write" ? (
        <textarea
          ref={editRef}
          className="pending-edit"
          value={draft}
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
        />
      ) : (
        <div className="composer-preview pending-preview">
          <CommentBody body={draft} />
        </div>
      )}
      <div className="row">
        <button
          className="action-btn btn-ok"
          disabled={empty}
          data-tip={empty ? "This action needs text" : undefined}
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

/** The card's one-line title. A diff comment's summary carries the file path,
 *  and a monorepo path wraps it three lines deep at panel width — so it's
 *  split the way the diff view's file header is: the directory gives way
 *  first, the filename and line never do. The full text lives in the tip. */
function PendingTitle({ summary }: { summary: string }) {
  const m = /^Comment on (.+):(\d+)$/.exec(summary);
  if (!m) {
    return (
      <span className="pending-summary" data-tip={summary}>
        {summary}
      </span>
    );
  }
  const [, path, line] = m;
  const dirIdx = path.lastIndexOf("/");
  return (
    <span className="pending-summary pending-summary-path" data-tip={`${path}:${line}`}>
      <span>Comment on</span>
      <span className="diff-path mono">
        {dirIdx >= 0 && <span className="diff-path-dir">{path.slice(0, dirIdx + 1)}</span>}
        <span className="diff-path-name">
          {path.slice(dirIdx + 1)}:{line}
        </span>
      </span>
    </span>
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
      data-tip="See exactly what is in the assistant's context"
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
