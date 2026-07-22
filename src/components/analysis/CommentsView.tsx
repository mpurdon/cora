import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { PrComment } from "../../bindings/PrComment";
import type { PrConversation } from "../../bindings/PrConversation";
import type { ReviewThread } from "../../bindings/ReviewThread";
import type { ReviewVerdict } from "../../bindings/ReviewVerdict";
import { isNonBlockingComment } from "../../lib/comments";
import { ipc } from "../../lib/ipc";
import { useDiffStore } from "../../state/diffStore";
import { timeAgo } from "../../state/prStore";
import { useReviewStore } from "../../state/reviewStore";
import { DiffJump, parseDiff, type DiffFile } from "./DiffView";

/** Comment id → DOM anchor, so reply notifications can deep-link here. */
export const commentAnchor = (commentId: string) => `comment-${commentId}`;

const REACTION_EMOJI: Record<string, string> = {
  THUMBS_UP: "👍",
  THUMBS_DOWN: "👎",
  LAUGH: "😄",
  HOORAY: "🎉",
  CONFUSED: "😕",
  HEART: "❤️",
  ROCKET: "🚀",
  EYES: "👀",
};

/** Existing reactions as toggleable chips + a picker for new ones. */
export function ReactionBar({
  comment,
  onChanged,
}: {
  comment: PrComment;
  onChanged: () => void;
}) {
  const [picker, setPicker] = useState(false);
  const toggle = async (content: string, has: boolean) => {
    setPicker(false);
    try {
      await ipc.toggleReaction(comment.id, content, has);
      onChanged();
    } catch {
      // reaction failures are non-critical; the reload will show truth
      onChanged();
    }
  };
  const reacted = new Map(comment.reactions.map((r) => [r.content, r]));
  return (
    <div className="reaction-bar">
      {comment.reactions.map((r) => (
        <button
          key={r.content}
          className={`reaction-chip${r.viewerHasReacted ? " mine" : ""}`}
          title={r.content.toLowerCase().replace(/_/g, " ")}
          onClick={() => void toggle(r.content, r.viewerHasReacted)}
        >
          {REACTION_EMOJI[r.content] ?? r.content} {r.count}
        </button>
      ))}
      <button className="reaction-add" title="Add reaction" onClick={() => setPicker((p) => !p)}>
        ☺+
      </button>
      {picker && (
        <div className="reaction-picker">
          {Object.entries(REACTION_EMOJI).map(([content, emoji]) => (
            <button
              key={content}
              className="reaction-option"
              onClick={() => void toggle(content, reacted.get(content)?.viewerHasReacted ?? false)}
            >
              {emoji}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** GitHub-flavored markdown, with links opening in the system browser. */
export function CommentBody({ body }: { body: string }) {
  return (
    <div className="comment-body markdown">
      <Markdown
        remarkPlugins={[remarkGfm]}
        // Bots (coverage reports, badges) write raw HTML in comment bodies;
        // parse it, then strip anything outside GitHub's own allowlist.
        rehypePlugins={[rehypeRaw, rehypeSanitize]}
        components={{
          code: ({ className, children }) => {
            // ```suggestion fences render as an applyable change, like GitHub.
            if ((className ?? "").includes("language-suggestion")) {
              const text = String(children).replace(/\n$/, "");
              return (
                <span className="suggestion-block">
                  <span className="suggestion-head">suggested change</span>
                  {text.split("\n").map((l, i) => (
                    <span key={i} className="suggestion-line">
                      {l || " "}
                    </span>
                  ))}
                </span>
              );
            }
            return <code className={className}>{children}</code>;
          },
          a: ({ href, children }) => (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                if (href) void openUrl(href);
              }}
            >
              {children}
            </a>
          ),
          img: ({ src, alt }) => (
            // Badges and screenshots — keep them small and never broken-huge.
            <img src={src ?? ""} alt={alt ?? ""} className="md-img" loading="lazy" />
          ),
        }}
      >
        {body}
      </Markdown>
    </div>
  );
}

function Comment({
  comment,
  isReply,
  onChanged,
  onQuoteReply,
}: {
  comment: PrComment;
  isReply: boolean;
  onChanged: () => void;
  onQuoteReply?: (comment: PrComment) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  // Bot comments clamp aggressively — visible, but never dominant.
  const long = comment.body.length > (comment.isBot ? 400 : 1500);
  return (
    <div className={`pr-comment${isReply ? " reply" : ""}`} id={commentAnchor(comment.id)}>
      <div className="comment-head">
        <span className="comment-author">{comment.author}</span>
        {comment.isBot && <span className="thread-tag">bot</span>}
        <span className="comment-when">{timeAgo(comment.createdAt)} ago</span>
        {onQuoteReply && (
          <button
            className="comment-action"
            title="Quote this comment in a reply"
            onClick={() => onQuoteReply(comment)}
          >
            Reply
          </button>
        )}
        <a className="comment-link" href={comment.url} target="_blank" rel="noreferrer" title="Open on GitHub">
          ↗
        </a>
      </div>
      <div className={long && !expanded ? "comment-clamped" : undefined}>
        <CommentBody body={comment.body} />
      </div>
      {long && (
        <button className="comment-expand" onClick={() => setExpanded((e) => !e)}>
          {expanded ? "show less" : "show more"}
        </button>
      )}
      <ReactionBar comment={comment} onChanged={onChanged} />
    </div>
  );
}

/** Textarea + submit, shared by the conversation composer, thread replies,
 *  and the diff view's inline composers. */
export function Composer({
  placeholder,
  submitLabel,
  onSubmit,
  onCancel,
  initialBody = "",
  autoFocus = false,
  suggestionSeed,
}: {
  placeholder: string;
  submitLabel: string;
  onSubmit: (body: string) => Promise<void>;
  onCancel?: () => void;
  initialBody?: string;
  autoFocus?: boolean;
  /** Current content of the anchored line — enables the ± suggestion button. */
  suggestionSeed?: string;
}) {
  const [body, setBody] = useState(initialBody);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // GitHub accepts one suggestion block per comment; grey the button out
  // once a fence is present.
  const hasSuggestion = body.includes("```suggestion");
  const insertSuggestion = () => {
    const block = `\`\`\`suggestion\n${suggestionSeed ?? ""}\n\`\`\`\n`;
    setBody((b) => (b.trim() ? `${b.replace(/\s+$/, "")}\n\n${block}` : block));
  };

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await onSubmit(body);
      setBody("");
      onCancel?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="composer">
      <textarea
        placeholder={placeholder}
        value={body}
        disabled={busy}
        autoFocus={autoFocus}
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && body.trim()) void submit();
        }}
      />
      {error && <div className="settings-error">{error}</div>}
      <div className="row composer-actions">
        {suggestionSeed != null && (
          <button
            className="composer-tool"
            title="Insert a suggestion the author can apply directly on GitHub"
            disabled={busy || hasSuggestion}
            onClick={insertSuggestion}
          >
            ± suggest
          </button>
        )}
        <span className="composer-hint mono">markdown · ⌘↩ to send</span>
        <span className="spacer" />
        {onCancel && (
          <button className="action-btn" disabled={busy} onClick={onCancel}>
            Cancel
          </button>
        )}
        <button className="action-btn" disabled={busy || !body.trim()} onClick={() => void submit()}>
          {busy ? "Sending…" : submitLabel}
        </button>
      </div>
    </div>
  );
}

const VERDICT_META: Record<string, { label: string; cls: string }> = {
  APPROVED: { label: "approved", cls: "approved" },
  CHANGES_REQUESTED: { label: "requested changes", cls: "changes" },
  COMMENTED: { label: "reviewed", cls: "commented" },
  DISMISSED: { label: "review dismissed", cls: "dismissed" },
};

/** A submitted review's verdict + summary body, inline in the conversation —
 *  this text lives on the review object, not in any comment or thread. */
function ReviewVerdictCard({ review }: { review: ReviewVerdict }) {
  const meta = VERDICT_META[review.state] ?? { label: review.state.toLowerCase(), cls: "commented" };
  return (
    <div className={`review-verdict ${meta.cls}`}>
      <div className="verdict-head">
        <span className="comment-author">{review.author}</span>
        <span className={`verdict-chip ${meta.cls}`}>{meta.label}</span>
        <span className="comment-ago mono">{timeAgo(review.submittedAt)} ago</span>
      </div>
      {review.body.trim() && <CommentBody body={review.body} />}
    </div>
  );
}

function Thread({
  thread,
  onShowCode,
  onReplied,
}: {
  thread: ReviewThread;
  onShowCode: (thread: ReviewThread) => void;
  onReplied: () => void;
}) {
  const [replying, setReplying] = useState(false);
  const [root, ...replies] = thread.comments;
  if (!root) return null;
  return (
    <div className={`review-thread${thread.resolved ? " resolved" : ""}`}>
      <div className="thread-anchor-row">
        {thread.path && (
          <button
            className="thread-anchor mono"
            title={`Show this code — ${thread.path}`}
            onClick={() => onShowCode(thread)}
          >
            {thread.path.includes("/") && (
              <span className="anchor-dir">
                {thread.path.slice(0, thread.path.lastIndexOf("/") + 1)}
              </span>
            )}
            <span className="anchor-name">
              {thread.path.slice(thread.path.lastIndexOf("/") + 1)}
              {thread.line != null &&
                `:${thread.startLine != null ? `${thread.startLine}–` : ""}${thread.line}`}
            </span>
          </button>
        )}
        {thread.resolved && <span className="thread-tag resolved-tag">resolved</span>}
        {thread.outdated && <span className="thread-tag">outdated</span>}
        {!thread.resolved && isNonBlockingComment(root.body) && (
          <span className="thread-tag" title="Doesn't block approval">
            non-blocking
          </span>
        )}
        <span className="spacer" />
        {!replying && (
          <>
            <button
              className="thread-reply-btn"
              onClick={() => void ipc.resolveThread(thread.id, !thread.resolved).then(onReplied)}
            >
              {thread.resolved ? "Unresolve" : "Resolve"}
            </button>
            {!thread.resolved && (
              <button className="thread-reply-btn" onClick={() => setReplying(true)}>
                Reply
              </button>
            )}
          </>
        )}
      </div>
      <Comment comment={root} isReply={false} onChanged={onReplied} />
      {replies.map((c) => (
        <Comment key={c.id} comment={c} isReply onChanged={onReplied} />
      ))}
      {replying && (
        <Composer
          placeholder="Reply to this thread…"
          submitLabel="Reply"
          onCancel={() => setReplying(false)}
          onSubmit={async (body) => {
            await ipc.replyToThread(thread.id, body);
            onReplied();
          }}
        />
      )}
    </div>
  );
}

/** Slide-over showing the DIFF of the referenced file, scrolled to the
 *  commented lines. Falls back to the full file when the thread's file isn't
 *  in the current diff (outdated threads). */
function CodeDrawer({
  prId,
  thread,
  onClose,
}: {
  prId: string;
  thread: ReviewThread;
  onClose: () => void;
}) {
  const [diffFile, setDiffFile] = useState<DiffFile | null | undefined>(undefined);
  const [fileContent, setFileContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const targetRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setDiffFile(undefined);
    setFileContent(null);
    setError(null);
    if (!thread.path) return;
    void ipc
      .getPrDiff(prId)
      .then((raw) => {
        const match = parseDiff(raw).find((f) => f.path === thread.path) ?? null;
        setDiffFile(match);
        if (!match) {
          // Not in the diff (outdated anchor) — show the file instead.
          void ipc
            .getFileAtHead(prId, thread.path!)
            .then(setFileContent)
            .catch((e) => setError(String(e)));
        }
      })
      .catch((e) => setError(String(e)));
  }, [prId, thread]);

  useEffect(() => {
    if (diffFile != null || fileContent != null) {
      // Let the lines render, then center the referenced range.
      requestAnimationFrame(() =>
        targetRef.current?.scrollIntoView({ block: "center", behavior: "instant" as ScrollBehavior }),
      );
    }
  }, [diffFile, fileContent]);

  const from = thread.startLine ?? thread.line ?? 0;
  const to = thread.line ?? from;
  const inRange = (n: number | null) => n != null && n >= from && n <= to && from > 0;

  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <aside className="activity-drawer code-drawer open">
        <header className="drawer-header">
          <span className="drawer-title mono code-drawer-path">
            {thread.path}
            {thread.line != null && `:${to}`}
            {diffFile && (
              <span className="diffstat mono">
                {" "}
                <span className="add">+{diffFile.additions}</span>{" "}
                <span className="del">−{diffFile.deletions}</span>
              </span>
            )}
          </span>
          <button className="icon-btn" title="Close" onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body code-drawer-body">
          {error && <div className="analysis-error">{error}</div>}
          {diffFile === undefined && !error && <div className="drawer-empty">fetching diff…</div>}

          {diffFile != null && (
            <pre className="diff-body">
              <div className="diff-scroll-inner">
              {diffFile.lines.map((l, i) => {
                if (l.kind === "hunk") return <DiffJump key={i} text={l.text} />;
                const referenced = l.kind !== "del" && inRange(l.newLine);
                return (
                  <div
                    key={i}
                    ref={
                      referenced && (l.newLine === from || from === 0) ? targetRef : undefined
                    }
                    className={`diff-line ${l.kind}${referenced ? " referenced-line" : ""}`}
                  >
                    <span className="code-lineno">{l.newLine ?? ""}</span>
                    <span className="diff-gutter">
                      {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                    </span>
                    {l.text}
                  </div>
                );
              })}
              </div>
            </pre>
          )}

          {diffFile === null && fileContent != null && (
            <pre className="code-file">
              {fileContent.split("\n").map((line, i) => {
                const n = i + 1;
                const referenced = n >= from && n <= to && from > 0;
                return (
                  <div
                    key={i}
                    ref={referenced && n === from ? targetRef : undefined}
                    className={`code-line${referenced ? " referenced" : ""}`}
                  >
                    <span className="code-lineno">{n}</span>
                    {line}
                  </div>
                );
              })}
            </pre>
          )}
        </div>
      </aside>
    </>
  );
}

export function CommentsView({
  prId,
  focusCommentId,
  onFocusHandled,
}: {
  prId: string;
  focusCommentId?: string | null;
  onFocusHandled?: () => void;
}) {
  const [conversation, setConversation] = useState<PrConversation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [codeThread, setCodeThread] = useState<ReviewThread | null>(null);
  const [prefill, setPrefill] = useState("");

  // A "± comment" on a finding with no resolvable diff file lands here,
  // pre-filling the conversation composer instead.
  const composeRequest = useDiffStore((s) => s.composeRequest);
  useEffect(() => {
    if (composeRequest?.target !== "conversation") return;
    setPrefill(composeRequest.seed);
    useDiffStore.getState().clearCompose();
    requestAnimationFrame(() => {
      document.getElementById("conversation-composer")?.scrollIntoView({ block: "center" });
    });
  }, [composeRequest]);

  const quoteReply = (comment: PrComment) => {
    const quoted = comment.body
      .split("\n")
      .slice(0, 6)
      .map((l) => `> ${l}`)
      .join("\n");
    setPrefill(`${quoted}\n\n@${comment.author} `);
    document.getElementById("conversation-composer")?.scrollIntoView({ block: "center" });
  };

  const load = () => {
    setError(null);
    void ipc
      .getPrComments(prId)
      .then(setConversation)
      .catch((e) => setError(String(e)));
  };

  useEffect(() => {
    setConversation(null);
    setCodeThread(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prId]);

  // A submitted review, a resolved thread or a refresh all change what belongs
  // in this conversation — without this, your own approval only showed up on
  // the next PR switch.
  useEffect(() => {
    const un = listen("reviews:changed", load);
    return () => void un.then((fn) => fn());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prId]);

  // Your just-submitted verdict, held until this query returns it — GitHub's
  // read-back lags its own mutation. Dropping it here (rather than in the
  // header) keeps the card from blinking out between the two refetches.
  const pendingVerdict = useReviewStore((s) => s.pending[prId]);
  useEffect(() => {
    if (!pendingVerdict) return;
    if (conversation?.reviews?.some((r) => r.id === pendingVerdict.id)) {
      useReviewStore.getState().clear(prId);
    }
  }, [conversation, pendingVerdict, prId]);

  // Deep-link: scroll to a specific comment once loaded (reply notifications).
  useEffect(() => {
    if (!focusCommentId || conversation == null) return;
    const el = document.getElementById(commentAnchor(focusCommentId));
    if (el) {
      el.scrollIntoView({ block: "center" });
      el.classList.add("comment-flash");
      setTimeout(() => el.classList.remove("comment-flash"), 2400);
    }
    onFocusHandled?.();
  }, [focusCommentId, conversation, onFocusHandled]);

  if (error) {
    return (
      <div className="placeholder">
        <p className="analysis-error">{error}</p>
        <button className="action-btn" onClick={load}>
          Retry
        </button>
      </div>
    );
  }
  if (conversation == null) {
    return <div className="canvas-loading">fetching comments…</div>;
  }

  const open = conversation.threads.filter((t) => !t.resolved);
  const resolved = conversation.threads.filter((t) => t.resolved);
  const served = conversation.reviews ?? [];
  const verdicts =
    pendingVerdict && !served.some((r) => r.id === pendingVerdict.id)
      ? [...served, pendingVerdict]
      : served;

  return (
    <div className="comments-view">
      {conversation.comments.length === 0 && conversation.threads.length === 0 && (
        <div className="placeholder">No comments yet.</div>
      )}

      {open.length > 0 && (
        <section>
          <span className="eyebrow">Review threads ({open.length} open)</span>
          {open.map((t) => (
            <Thread key={t.id} thread={t} onShowCode={setCodeThread} onReplied={load} />
          ))}
        </section>
      )}

      <section>
        <span className="eyebrow">Conversation</span>
        {[
          ...conversation.comments.map((c) => ({ at: c.createdAt, el: (
            <Comment
              key={c.id}
              comment={c}
              isReply={false}
              onChanged={load}
              onQuoteReply={quoteReply}
            />
          ) })),
          ...verdicts.map((r) => ({ at: r.submittedAt, el: (
            <ReviewVerdictCard key={r.id} review={r} />
          ) })),
        ]
          .sort((a, b) => a.at.localeCompare(b.at))
          .map((x) => x.el)}
        <div id="conversation-composer">
          <Composer
            key={prefill} // remount to adopt a new quote prefill
            initialBody={prefill}
            autoFocus={prefill.length > 0}
            placeholder="Comment on this pull request…"
            submitLabel="Comment"
            onSubmit={async (body) => {
              await ipc.addPrComment(prId, body);
              setPrefill("");
              load();
            }}
          />
        </div>
      </section>

      {resolved.length > 0 && (
        <section>
          <span className="eyebrow">Resolved threads ({resolved.length})</span>
          {resolved.map((t) => (
            <Thread key={t.id} thread={t} onShowCode={setCodeThread} onReplied={load} />
          ))}
        </section>
      )}

      {codeThread && (
        <CodeDrawer prId={prId} thread={codeThread} onClose={() => setCodeThread(null)} />
      )}
    </div>
  );
}
