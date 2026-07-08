import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { PrComment } from "../../bindings/PrComment";
import type { PrConversation } from "../../bindings/PrConversation";
import type { ReviewThread } from "../../bindings/ReviewThread";
import { ipc } from "../../lib/ipc";
import { timeAgo } from "../../state/prStore";

/** Comment id → DOM anchor, so reply notifications can deep-link here. */
export const commentAnchor = (commentId: string) => `comment-${commentId}`;

/** GitHub-flavored markdown, with links opening in the system browser. */
function CommentBody({ body }: { body: string }) {
  return (
    <div className="comment-body markdown">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
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

function Comment({ comment, isReply }: { comment: PrComment; isReply: boolean }) {
  const [expanded, setExpanded] = useState(false);
  // Bot comments clamp aggressively — visible, but never dominant.
  const long = comment.body.length > (comment.isBot ? 400 : 1500);
  return (
    <div className={`pr-comment${isReply ? " reply" : ""}`} id={commentAnchor(comment.id)}>
      <div className="comment-head">
        <span className="comment-author">{comment.author}</span>
        {comment.isBot && <span className="thread-tag">bot</span>}
        <span className="comment-when">{timeAgo(comment.createdAt)} ago</span>
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
    </div>
  );
}

function Thread({
  thread,
  onShowCode,
}: {
  thread: ReviewThread;
  onShowCode: (thread: ReviewThread) => void;
}) {
  const [root, ...replies] = thread.comments;
  if (!root) return null;
  return (
    <div className={`review-thread${thread.resolved ? " resolved" : ""}`}>
      <div className="thread-anchor-row">
        {thread.path && (
          <button
            className="thread-anchor mono"
            title="Show this code"
            onClick={() => onShowCode(thread)}
          >
            {thread.path}
            {thread.line != null && `:${thread.startLine != null ? `${thread.startLine}–` : ""}${thread.line}`}
          </button>
        )}
        {thread.resolved && <span className="thread-tag resolved-tag">resolved</span>}
        {thread.outdated && <span className="thread-tag">outdated</span>}
      </div>
      <Comment comment={root} isReply={false} />
      {replies.map((c) => (
        <Comment key={c.id} comment={c} isReply />
      ))}
    </div>
  );
}

/** Slide-over showing the whole file, scrolled to the referenced lines. */
function CodeDrawer({
  prId,
  thread,
  onClose,
}: {
  prId: string;
  thread: ReviewThread;
  onClose: () => void;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const targetRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setContent(null);
    setError(null);
    if (!thread.path) return;
    void ipc
      .getFileAtHead(prId, thread.path)
      .then(setContent)
      .catch((e) => setError(String(e)));
  }, [prId, thread]);

  useEffect(() => {
    if (content != null) {
      // Let the lines render, then center the referenced range.
      requestAnimationFrame(() =>
        targetRef.current?.scrollIntoView({ block: "center", behavior: "instant" as ScrollBehavior }),
      );
    }
  }, [content]);

  const from = thread.startLine ?? thread.line ?? 0;
  const to = thread.line ?? from;

  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <aside className="activity-drawer code-drawer open">
        <header className="drawer-header">
          <span className="drawer-title mono code-drawer-path">
            {thread.path}
            {thread.line != null && `:${to}`}
          </span>
          <button className="icon-btn" title="Close" onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body code-drawer-body">
          {error && <div className="analysis-error">{error}</div>}
          {content == null && !error && <div className="drawer-empty">fetching file…</div>}
          {content != null && (
            <pre className="code-file">
              {content.split("\n").map((line, i) => {
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

  return (
    <div className="comments-view">
      {conversation.comments.length === 0 && conversation.threads.length === 0 && (
        <div className="placeholder">No comments yet.</div>
      )}

      {open.length > 0 && (
        <section>
          <span className="eyebrow">Review threads ({open.length} open)</span>
          {open.map((t) => (
            <Thread key={t.id} thread={t} onShowCode={setCodeThread} />
          ))}
        </section>
      )}

      {conversation.comments.length > 0 && (
        <section>
          <span className="eyebrow">Conversation</span>
          {conversation.comments.map((c) => (
            <Comment key={c.id} comment={c} isReply={false} />
          ))}
        </section>
      )}

      {resolved.length > 0 && (
        <section>
          <span className="eyebrow">Resolved threads ({resolved.length})</span>
          {resolved.map((t) => (
            <Thread key={t.id} thread={t} onShowCode={setCodeThread} />
          ))}
        </section>
      )}

      {codeThread && (
        <CodeDrawer prId={prId} thread={codeThread} onClose={() => setCodeThread(null)} />
      )}
    </div>
  );
}
