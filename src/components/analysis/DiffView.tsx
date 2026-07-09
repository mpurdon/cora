import { useEffect, useMemo, useState } from "react";
import type { PrConversation } from "../../bindings/PrConversation";
import type { ReviewThread } from "../../bindings/ReviewThread";
import { matchesAny, reviewOrderScore } from "../../lib/globs";
import { ipc } from "../../lib/ipc";
import { timeAgo } from "../../state/prStore";
import { CommentBody, Composer, ReactionBar } from "./CommentsView";

export interface DiffLine {
  kind: "add" | "del" | "ctx" | "hunk";
  text: string;
  /** Line number on the new (RIGHT) side — where review comments anchor. */
  newLine: number | null;
}

export interface DiffFile {
  path: string;
  oldPath?: string;
  additions: number;
  deletions: number;
  lines: DiffLine[];
}

/** Cheap stable digest of a file's patch — viewed-state goes stale when the
 *  file's diff changes, without unviewing untouched files on every push. */
export function fileDigest(file: DiffFile): string {
  const s = file.lines.map((l) => l.kind + l.text).join("\n");
  let h = 5381;
  for (let i = 0; i < s.length; i++) {
    h = ((h * 33) ^ s.charCodeAt(i)) >>> 0;
  }
  return `${h.toString(36)}:${s.length}`;
}

export function parseDiff(raw: string): DiffFile[] {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  let newLine = 0;
  for (const line of raw.split("\n")) {
    if (line.startsWith("diff --git ")) {
      if (current) files.push(current);
      const m = line.match(/^diff --git a\/(.*) b\/(.*)$/);
      current = {
        path: m?.[2] ?? line.slice(11),
        oldPath: m && m[1] !== m[2] ? m[1] : undefined,
        additions: 0,
        deletions: 0,
        lines: [],
      };
      newLine = 0;
      continue;
    }
    if (!current) continue;
    if (
      line.startsWith("index ") ||
      line.startsWith("--- ") ||
      line.startsWith("+++ ") ||
      line.startsWith("new file") ||
      line.startsWith("deleted file") ||
      line.startsWith("similarity") ||
      line.startsWith("rename ") ||
      line.startsWith("old mode") ||
      line.startsWith("new mode") ||
      line.startsWith("Binary files")
    ) {
      continue;
    }
    if (line.startsWith("@@")) {
      const m = line.match(/\+(\d+)/);
      newLine = m ? Number(m[1]) : 0;
      current.lines.push({ kind: "hunk", text: line, newLine: null });
    } else if (line.startsWith("+")) {
      current.additions += 1;
      current.lines.push({ kind: "add", text: line.slice(1), newLine });
      newLine += 1;
    } else if (line.startsWith("-")) {
      current.deletions += 1;
      current.lines.push({ kind: "del", text: line.slice(1), newLine: null });
    } else {
      current.lines.push({
        kind: "ctx",
        text: line.startsWith(" ") ? line.slice(1) : line,
        newLine,
      });
      newLine += 1;
    }
  }
  if (current) files.push(current);
  return files;
}

/** Compact review thread rendered inline under its diff line. */
function InlineThread({
  thread,
  onChanged,
}: {
  thread: ReviewThread;
  onChanged: () => void;
}) {
  const [replying, setReplying] = useState(false);
  return (
    <div className="inline-thread">
      {thread.comments.map((c) => (
        <div key={c.id} className="inline-comment">
          <div className="comment-head">
            <span className="comment-author">{c.author}</span>
            {c.isBot && <span className="thread-tag">bot</span>}
            <span className="comment-when">{timeAgo(c.createdAt)} ago</span>
          </div>
          <CommentBody body={c.body} />
          <ReactionBar comment={c} onChanged={onChanged} />
        </div>
      ))}
      {replying ? (
        <Composer
          placeholder="Reply…"
          submitLabel="Reply"
          autoFocus
          onCancel={() => setReplying(false)}
          onSubmit={async (body) => {
            await ipc.replyToThread(thread.id, body);
            onChanged();
          }}
        />
      ) : (
        <div className="row">
          <button className="thread-reply-btn" onClick={() => setReplying(true)}>
            Reply
          </button>
          <button
            className="thread-reply-btn"
            onClick={() => void ipc.resolveThread(thread.id, true).then(onChanged)}
          >
            Resolve
          </button>
        </div>
      )}
    </div>
  );
}

function FileDiff({
  file,
  prId,
  threadsByLine,
  onChanged,
  viewed,
  onViewedChange,
}: {
  file: DiffFile;
  prId: string;
  threadsByLine: Map<number, ReviewThread[]>;
  onChanged: () => void;
  viewed: boolean;
  onViewedChange: (viewed: boolean) => void;
}) {
  const hasThreads = threadsByLine.size > 0;
  const [open, setOpen] = useState(!viewed && (file.lines.length <= 400 || hasThreads));

  // Marking viewed collapses the file (and vice versa), like GitHub.
  useEffect(() => {
    setOpen(!viewed && (file.lines.length <= 400 || hasThreads));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewed]);

  const [composeLine, setComposeLine] = useState<number | null>(null);

  return (
    <div className={`diff-file${viewed ? " viewed" : ""}`}>
      <div className="diff-file-header">
        <button className="diff-file-toggle" onClick={() => setOpen((o) => !o)}>
          <span className="chevron">{open ? "▾" : "▸"}</span>
          <span className="diff-path mono">
            {file.oldPath ? `${file.oldPath} → ` : ""}
            {file.path}
          </span>
          {hasThreads && <span className="thread-tag">{threadsByLine.size} threads</span>}
          <span className="spacer" />
          <span className="diffstat mono">
            <span className="add">+{file.additions}</span>{" "}
            <span className="del">−{file.deletions}</span>
          </span>
        </button>
        <label className="viewed-check check-row" title="Mark as viewed — clears if the file changes again">
          <input
            type="checkbox"
            checked={viewed}
            onChange={(e) => onViewedChange(e.target.checked)}
          />
          viewed
        </label>
      </div>
      {open && (
        <pre className="diff-body">
          {file.lines.map((l, i) => {
            const threads = l.newLine != null ? threadsByLine.get(l.newLine) : undefined;
            const commentable = l.newLine != null && l.kind !== "hunk";
            return (
              <div key={i}>
                <div className={`diff-line ${l.kind}`}>
                  <span className="diff-gutter">
                    {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                  </span>
                  {commentable && (
                    <button
                      className="line-comment-btn"
                      title={`Comment on line ${l.newLine}`}
                      onClick={() => setComposeLine(composeLine === l.newLine ? null : l.newLine)}
                    >
                      +
                    </button>
                  )}
                  {l.text}
                </div>
                {threads?.map((t) => (
                  <InlineThread key={t.id} thread={t} onChanged={onChanged} />
                ))}
                {composeLine != null && composeLine === l.newLine && (
                  <div className="inline-thread">
                    <Composer
                      placeholder={`Comment on ${file.path}:${l.newLine}…`}
                      submitLabel="Comment"
                      autoFocus
                      onCancel={() => setComposeLine(null)}
                      onSubmit={async (body) => {
                        await ipc.addDiffComment(prId, file.path, composeLine, body);
                        setComposeLine(null);
                        onChanged();
                      }}
                    />
                  </div>
                )}
              </div>
            );
          })}
        </pre>
      )}
    </div>
  );
}

export function DiffView({ prId, headSha }: { prId: string; headSha: string }) {
  const [raw, setRaw] = useState<string | null>(null);
  const [conversation, setConversation] = useState<PrConversation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [viewedMap, setViewedMap] = useState<Map<string, string>>(new Map());
  const [ignoreGlobs, setIgnoreGlobs] = useState<string[]>([]);
  const [showSkipped, setShowSkipped] = useState(false);

  const loadComments = () =>
    void ipc
      .getPrComments(prId)
      .then(setConversation)
      .catch(() => setConversation(null)); // comments are enrichment; diff still shows

  useEffect(() => {
    setRaw(null);
    setError(null);
    void ipc
      .getPrDiff(prId)
      .then(setRaw)
      .catch((e) => setError(String(e)));
    loadComments();
    void ipc
      .getViewedFiles(prId)
      .then((rows) => setViewedMap(new Map(rows.map((r) => [r.path, r.digest]))));
    void ipc.getSettings().then((s) => setIgnoreGlobs(s.reviewIgnoreGlobs));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prId, headSha]);

  const allFiles = useMemo(() => (raw ? parseDiff(raw) : []), [raw]);

  // Insignificant files (lockfiles, generated, snapshots) are skipped:
  // parked in their own collapsed section and excluded from progress.
  const { files, skipped } = useMemo(() => {
    const significant: DiffFile[] = [];
    const insignificant: DiffFile[] = [];
    for (const f of allFiles) {
      (matchesAny(f.path, ignoreGlobs) ? insignificant : significant).push(f);
    }
    // Reading order: interfaces/source → styles → config → docs → tests,
    // bigger churn first within each band.
    significant.sort(
      (a, b) =>
        reviewOrderScore(a.path) - reviewOrderScore(b.path) ||
        b.additions + b.deletions - (a.additions + a.deletions),
    );
    return { files: significant, skipped: insignificant };
  }, [allFiles, ignoreGlobs]);

  // A file counts as viewed only while its patch digest still matches —
  // an update after viewing clears it automatically.
  const isViewed = (file: DiffFile) => viewedMap.get(file.path) === fileDigest(file);

  const setViewed = (file: DiffFile, viewed: boolean) => {
    const digest = fileDigest(file);
    setViewedMap((prev) => {
      const next = new Map(prev);
      if (viewed) next.set(file.path, digest);
      else next.delete(file.path);
      return next;
    });
    void ipc.setFileViewed(prId, file.path, digest, viewed);
  };

  // Unresolved threads grouped per file → line.
  const threadsByFile = useMemo(() => {
    const map = new Map<string, Map<number, ReviewThread[]>>();
    for (const t of conversation?.threads ?? []) {
      if (t.resolved || !t.path || t.line == null) continue;
      const byLine = map.get(t.path) ?? new Map<number, ReviewThread[]>();
      byLine.set(t.line, [...(byLine.get(t.line) ?? []), t]);
      map.set(t.path, byLine);
    }
    return map;
  }, [conversation]);

  if (error) {
    return <div className="placeholder analysis-error">{error}</div>;
  }
  if (raw === null) {
    return <div className="canvas-loading">fetching diff…</div>;
  }
  if (allFiles.length === 0) {
    return <div className="placeholder">Empty diff.</div>;
  }
  const viewedCount = files.filter(isViewed).length;

  return (
    <div className="diff-view">
      <div className="eyebrow diff-summary">
        {files.length} files ·{" "}
        <span className="add">+{files.reduce((n, f) => n + f.additions, 0)}</span>{" "}
        <span className="del">−{files.reduce((n, f) => n + f.deletions, 0)}</span>
        {" · "}
        <span className={viewedCount === files.length ? "add" : undefined}>
          {viewedCount}/{files.length} viewed
        </span>
        {" · hover a line and hit + to comment"}
      </div>
      {files.map((f) => (
        <FileDiff
          key={f.path}
          file={f}
          prId={prId}
          threadsByLine={threadsByFile.get(f.path) ?? new Map()}
          onChanged={loadComments}
          viewed={isViewed(f)}
          onViewedChange={(v) => setViewed(f, v)}
        />
      ))}

      {skipped.length > 0 && (
        <div className="skipped-section">
          <button className="skipped-toggle eyebrow" onClick={() => setShowSkipped((s) => !s)}>
            {showSkipped ? "▾" : "▸"} {skipped.length} insignificant file
            {skipped.length === 1 ? "" : "s"} skipped (lockfiles, generated, snapshots)
          </button>
          {showSkipped &&
            skipped.map((f) => (
              <div key={f.path} className="skipped-file">
                <FileDiff
                  file={f}
                  prId={prId}
                  threadsByLine={threadsByFile.get(f.path) ?? new Map()}
                  onChanged={loadComments}
                  viewed={isViewed(f)}
                  onViewedChange={(v) => setViewed(f, v)}
                />
              </div>
            ))}
        </div>
      )}
    </div>
  );
}
