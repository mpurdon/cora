import { useEffect, useState } from "react";
import type { ChatContext } from "../../bindings/ChatContext";
import type { ContextGroup } from "../../bindings/ContextGroup";
import type { ContextPart } from "../../bindings/ContextPart";
import { ipc } from "../../lib/ipc";
import { useChatStore } from "../../state/chatStore";
import { formatTokens } from "./TraceSteps";

/** Presentation for the groups we know about. Rendering walks the parts, not
 *  this table, so a group added on the Rust side still shows up (under its own
 *  key) instead of vanishing from a view whose total still counts it. */
const GROUP_COPY: Partial<Record<ContextGroup, { title: string; blurb: string }>> = {
  system: {
    title: "System prompt",
    blurb: "Sent ahead of every message: instructions, the PR, the analysis.",
  },
  tools: {
    title: "Tool definitions",
    blurb: "Names, descriptions and input schemas — in context on every turn.",
  },
  messages: {
    title: "Conversation",
    blurb: "Your messages, the model's replies, and everything its tools read back.",
  },
};

/** Each part's exact bytes, revealed on demand — a 4k-line file in a tool
 *  result is the whole point of looking, so it renders verbatim. */
function Part({ part }: { part: ContextPart }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`context-part${open ? " open" : ""}`}>
      <button className="context-part-head" onClick={() => setOpen((o) => !o)}>
        <span className="chevron">{open ? "▾" : "▸"}</span>
        <span className="context-label">{part.label}</span>
        <span className="context-origin">{part.origin}</span>
        <span className="context-size mono">{formatTokens(part.estTokens)}</span>
      </button>
      {open && (
        <pre className="context-text">
          {part.text || "(empty)"}
        </pre>
      )}
    </div>
  );
}

/** Exactly what the assistant is being given, itemised by where it came
 *  from. Opens over the app because the contents are full files. */
export function ContextDrawer({ prId, onClose }: { prId: string; onClose: () => void }) {
  const [context, setContext] = useState<ChatContext | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Mounted only while open, and fetched with text — so a drawer left open
  // while an analysis lands or a turn runs refetches rather than showing the
  // context as it was when opened.
  // Depend on the size, not the snapshot's identity: the panel replaces that
  // object on every chat event, and refetching here pulls the whole context —
  // the largest payload this app moves — back across the IPC bridge.
  const tokens = useChatStore((s) => s.contexts[prId]?.projectedTokens);
  useEffect(() => {
    setError(null);
    void ipc
      .getChatContext(prId, true)
      .then(setContext)
      .catch((e) => setError(String(e)));
  }, [prId, tokens]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const usage = context?.usage;
  const est = context?.estTokens ?? 0;
  const windowSize = context?.windowTokens ?? 0;
  const total = context?.projectedTokens ?? 0;
  const pct = windowSize > 0 ? Math.min(100, Math.round((total / windowSize) * 100)) : null;

  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <aside className="activity-drawer context-drawer open">
        <header className="drawer-header">
          <div className="drawer-title">
            Assistant context
            {pct != null && (
              <span className="drawer-usage mono">
                {formatTokens(total)} of {formatTokens(windowSize)} · {pct}%
              </span>
            )}
          </div>
          <button className="icon-btn" title="Close" onClick={onClose}>
            ✕
          </button>
        </header>

        <div className="drawer-body">
          {error && <div className="settings-error">{error}</div>}
          {!context && !error && <div className="drawer-empty">reading context…</div>}

          {context && (
            <>
              <div className="context-summary">
                <div className="context-stat">
                  <span className="context-stat-num mono">≈{formatTokens(total)}</span>
                  <span className="eyebrow">
                    next request{usage ? " · calibrated" : ` · raw estimate (${formatTokens(est)})`}
                  </span>
                </div>
                {usage ? (
                  <div className="context-stat">
                    <span className="context-stat-num mono">
                      {formatTokens(usage.promptTokens)}
                    </span>
                    <span className="eyebrow">
                      measured on the last request
                      {usage.cacheReadTokens > 0 &&
                        ` · ${formatTokens(usage.cacheReadTokens)} of it from cache`}
                    </span>
                  </div>
                ) : (
                  <div className="context-stat">
                    <span className="context-stat-num mono">—</span>
                    <span className="eyebrow">nothing sent yet this session</span>
                  </div>
                )}
                <div className="context-stat">
                  <span className="context-stat-num mono">{context.requests}</span>
                  <span className="eyebrow">
                    requests · {context.modelId.split("/").pop()}
                  </span>
                </div>
              </div>
              <p className="context-note">
                Per-part sizes are estimated at ~4 characters per token. Only Bedrock counts
                exactly, so the total above is that estimate scaled by how far it missed on the
                last measured request. Everything below is sent verbatim on the next turn.
              </p>

              {[...new Set(context.parts.map((p) => p.group))].map((key) => {
                const parts = context.parts.filter((p) => p.group === key);
                const { title, blurb } = GROUP_COPY[key] ?? { title: key, blurb: "" };
                const sum = parts.reduce((n, p) => n + p.estTokens, 0);
                return (
                  <section key={key} className="context-group">
                    <div className="context-group-head">
                      <span className="eyebrow">{title}</span>
                      <span className="group-count">{parts.length}</span>
                      <span className="spacer" />
                      <span className="context-size mono">{formatTokens(sum)}</span>
                    </div>
                    <p className="context-blurb">{blurb}</p>
                    {parts.map((p) => (
                      <Part key={p.id} part={p} />
                    ))}
                  </section>
                );
              })}
            </>
          )}
        </div>
      </aside>
    </>
  );
}
