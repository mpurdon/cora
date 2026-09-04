import { useEffect, useMemo, useRef, useState } from "react";
import { tip } from "../Tooltip";
import { CopyButton } from "../CopyButton";
import type { PrConversation } from "../../bindings/PrConversation";
import type { ReviewMark } from "../../bindings/ReviewMark";
import type { ReviewPlanEntry } from "../../bindings/ReviewPlanEntry";
import type { ReviewThread } from "../../bindings/ReviewThread";

import { treeFileOrder } from "../../lib/fileTree";
import { matchesAny } from "../../lib/globs";
import { usePersistedFlag } from "../../lib/persisted";
import { ipc } from "../../lib/ipc";
import { analysisKey, useAnalysisStore } from "../../state/analysisStore";
import { useDiffStore } from "../../state/diffStore";
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
  /** Where this file's slice of the original unified diff sits in `src` —
   *  including the `diff --git` / `index` / mode headers the parser otherwise
   *  drops. Offsets rather than a copy: every file shares one reference to the
   *  raw diff, so nothing is duplicated or re-joined at parse time, and the
   *  patch text is built only when someone actually asks for it. */
  src: string;
  patchStart: number;
  patchEnd: number;
  /** The whole file was deleted — every line is a removal. */
  deleted?: boolean;
  /** The whole file is new in this PR. */
  added?: boolean;
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

/** A file's unified diff, cut from the raw diff on demand. Null for a DiffFile
 *  that predates these offsets — one parsed by an older build of this module and
 *  still held in a React memo across a hot reload — which disables the copy
 *  button rather than putting "undefined" on the clipboard. */
export function filePatch(file: DiffFile): string | null {
  if (file.src == null) return null;
  // Exactly one trailing newline: the slice ends wherever the next file's
  // header begins, and patches are line-terminated files.
  return `${file.src.slice(file.patchStart, file.patchEnd).replace(/\n+$/, "")}\n`;
}

export function parseDiff(raw: string): DiffFile[] {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  let newLine = 0;
  // Byte offset of the line being read, so each file can record where its slice
  // of `raw` starts and ends instead of copying the text.
  let offset = 0;
  const flush = (end: number) => {
    if (!current) return;
    current.patchEnd = end;
    files.push(current);
  };
  for (const line of raw.split("\n")) {
    const lineStart = offset;
    offset += line.length + 1; // +1 for the "\n" split consumed
    if (line.startsWith("diff --git ")) {
      flush(lineStart);
      const m = line.match(/^diff --git a\/(.*) b\/(.*)$/);
      current = {
        path: m?.[2] ?? line.slice(11),
        oldPath: m && m[1] !== m[2] ? m[1] : undefined,
        additions: 0,
        deletions: 0,
        lines: [],
        src: raw,
        patchStart: lineStart,
        patchEnd: raw.length,
      };
      newLine = 0;
      continue;
    }
    if (!current) continue;
    if (line.startsWith("deleted file")) {
      current.deleted = true;
      continue;
    }
    if (line.startsWith("new file")) {
      current.added = true;
      continue;
    }
    if (
      line.startsWith("index ") ||
      line.startsWith("--- ") ||
      line.startsWith("+++ ") ||
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
  flush(raw.length);
  return files;
}

// The file rail and the Diff tab parse the same raw diff — a one-slot cache
// keyed on string identity makes the multi-hundred-KB parse shared work.
let lastParsed: { raw: string; files: DiffFile[] } | null = null;
export function parseDiffCached(raw: string): DiffFile[] {
  if (lastParsed?.raw !== raw) lastParsed = { raw, files: parseDiff(raw) };
  return lastParsed.files;
}

/** A hunk worth collapsing: every changed line is imports/housekeeping.
 *  Bare identifier lines (multi-line import blocks) count as trivial only
 *  when the hunk visibly sits in import context. */
const IMPORTISH =
  /^\s*(import\b|from\s+\S+\s+import\b|export\s+(\*|\{[^}]*\})\s+from\b|use\s+[\w:{}, ]+;|#include\b|package\s|(const|let|var)\s+[\w{},\s]+=\s*require\()/;
const BARE_CONTINUATION = /^\s*[)\]}]*,?\s*$|^\s*[\w.$@/"'-]+,?\s*$/;

export function isTrivialHunk(lines: DiffLine[]): boolean {
  const changed = lines.filter((l) => l.kind === "add" || l.kind === "del");
  if (changed.length === 0) return false;
  const importContext = lines.some((l) => IMPORTISH.test(l.text));
  return changed.every(
    (l) =>
      /^\s*$/.test(l.text) ||
      IMPORTISH.test(l.text) ||
      (importContext && BARE_CONTINUATION.test(l.text)),
  );
}

/** Mixed hunks (import churn sharing a hunk with real changes) still deserve
 *  collapsing: split into contiguous runs of import-compatible lines. A run
 *  is trivial when every line is import-ish, it contains a change, and at
 *  least one line is an actual import — bare continuations alone (arrays,
 *  closing braces) never qualify without that anchor. */
export function splitTrivialRuns(lines: DiffLine[]): { trivial: boolean; lines: DiffLine[] }[] {
  const compatible = (t: string) =>
    /^\s*$/.test(t) || IMPORTISH.test(t) || BARE_CONTINUATION.test(t);
  const runs: { trivial: boolean; lines: DiffLine[] }[] = [];
  let cur: DiffLine[] = [];
  let curCompat = false;
  const flush = () => {
    if (cur.length === 0) return;
    const trivial =
      curCompat &&
      cur.some((l) => l.kind === "add" || l.kind === "del") &&
      cur.some((l) => IMPORTISH.test(l.text));
    runs.push({ trivial, lines: cur });
    cur = [];
  };
  for (const l of lines) {
    const c = compatible(l.text);
    if (cur.length > 0 && c !== curCompat) flush();
    curCompat = c;
    cur.push(l);
  }
  flush();
  return runs;
}

const HUNK_RE = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@ ?(.*)$/;

/** Hunk headers render as a torn/dashed "jump" divider instead of raw
 *  `@@ -a,b +c,d @@` syntax. The gutter line numbers already say where you
 *  landed, so the label carries only git's function context, kept at its
 *  own indentation and aligned to the code column; raw header in the tooltip. */
export function DiffJump({ text }: { text: string }) {
  // Preserve leading whitespace — it's the function's real indent.
  const context = (text.match(HUNK_RE)?.[2] ?? "").replace(/\s+$/, "");
  return (
    <div className="diff-jump" data-tip={text}>
      <span className="diff-jump-rule" />
      {context && <span className="diff-jump-label">{context}</span>}
      <span className="diff-jump-rule" />
    </div>
  );
}

/** The model's reason plus the objective metrics that ground it. */
export function planTooltip(plan: ReviewPlanEntry): string | undefined {
  const m = plan.metrics;
  const metricsLine = m
    ? `+${m.additions}/−${m.deletions} · ${m.addedBranches} branch pts · ${m.newDefs} new defs · ${Math.round(m.importShare * 100)}% imports · nesting ${m.maxNesting}`
    : undefined;
  const text = [plan.reason, metricsLine].filter(Boolean).join("\n");
  return text || undefined;
}

/** Compact review thread rendered inline under its diff line. */
function InlineThread({
  thread,
  onChanged,
  lineText,
}: {
  thread: ReviewThread;
  onChanged: () => void;
  /** Current content of the thread's anchor line, for ± suggestions. */
  lineText?: string;
}) {
  const [replying, setReplying] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  return (
    <div className="inline-thread">
      {thread.comments.map((c) => (
        <div key={c.id} className="inline-comment">
          <div className="comment-head">
            <span className="comment-author">{c.author}</span>
            {c.isBot && <span className="thread-tag">bot</span>}
            <span className="comment-when">{timeAgo(c.createdAt)} ago</span>
            {c.viewerCanEdit && editingId !== c.id && (
              <button
                className="comment-action"
                data-tip="Edit this comment"
                aria-label="Edit this comment"
                onClick={() => setEditingId(c.id)}
              >
                Edit
              </button>
            )}
          </div>
          {editingId === c.id ? (
            <Composer
              placeholder="Edit your comment…"
              submitLabel="Save"
              autoFocus
              initialBody={c.body}
              suggestionSeed={lineText}
              onCancel={() => setEditingId(null)}
              onSubmit={async (body) => {
                await ipc.updateComment(c.id, body, c.isReviewComment);
                setEditingId(null);
                onChanged();
              }}
            />
          ) : (
            <>
              <CommentBody body={c.body} />
              <ReactionBar comment={c} onChanged={onChanged} />
            </>
          )}
        </div>
      ))}
      {replying ? (
        <Composer
          placeholder="Reply…"
          submitLabel="Reply"
          autoFocus
          suggestionSeed={lineText}
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

/** A range of new-side lines a review comment anchors to. `end` is the line
 *  the comment posts against; `start` (≤ end) the top of the range. */
interface LineRange {
  start: number;
  end: number;
}

/** The press-to-comment machinery, shared by the hunk view and the whole-file
 *  view so both offer exactly the same affordance. A plain click on a line's
 *  + is a one-line range; dragging the + down the gutter grows it, the way
 *  GitHub lets you suggest across several lines. The composer only opens on
 *  release, so it doesn't flicker under the moving end. Posting goes through
 *  `addDiffComment` from either view, so the comment lands in the PR
 *  conversation the same way. */
/** A finding's hidden tag goes on the end of the posted body, after the text
 *  the reviewer saw and edited. Nothing to add for an ordinary comment. */
export function withMarker(body: string, marker: string): string {
  return marker ? `${body.trimEnd()}\n\n${marker}` : body;
}

function useLineComments({
  prId,
  path,
  lineTextByNew,
  onChanged,
}: {
  prId: string;
  path: string;
  /** The current content of every new-side line, so a range can hand the
   *  composer the exact text it would replace in a suggestion. */
  lineTextByNew: Map<number, string>;
  onChanged: () => void;
}) {
  const [range, setRange] = useState<LineRange | null>(null);
  const [seedBody, setSeedBody] = useState("");
  // The finding tag to post with a seeded comment — see ComposeRequest.marker.
  const [seedMarker, setSeedMarker] = useState("");
  // While the pointer is held on a + button we're extending a selection.
  const [selecting, setSelecting] = useState(false);
  // `anchor` is the line pressed; `last` the line the cursor is currently over,
  // so a move only re-ranges when it crosses into a new line. `x`/`y` are the
  // latest cursor position, resolved to a line once per frame — see the move
  // handler.
  const drag = useRef<{
    anchor: number;
    last: number;
    x: number;
    y: number;
    raf: number;
  } | null>(null);

  // A drag can end anywhere on the page, so listen globally while one is live.
  useEffect(() => {
    const onUp = () => {
      if (drag.current?.raf) cancelAnimationFrame(drag.current.raf);
      drag.current = null;
      setSelecting(false);
    };
    window.addEventListener("pointerup", onUp);
    return () => window.removeEventListener("pointerup", onUp);
  }, []);

  const rangeText = (r: LineRange) => {
    const out: string[] = [];
    for (let n = r.start; n <= r.end; n++) {
      const t = lineTextByNew.get(n);
      if (t != null) out.push(t);
    }
    return out.join("\n");
  };

  /** Open a one-line composer on `line`, pre-filled (assessment finding → comment). */
  const seed = (line: number, body: string, marker = "") => {
    setSeedBody(body);
    setSeedMarker(marker);
    setRange({ start: line, end: line });
  };
  const close = () => {
    setRange(null);
    setSeedBody("");
    setSeedMarker("");
  };

  // Press to open a one-line composer; drag down to span a range (the
  // composer opens on release). Pressing the + of a line whose one-line
  // composer is already open closes it again — decided here, before we
  // overwrite the range.
  const onPointerDown = (n: number) => (e: React.PointerEvent<HTMLButtonElement>) => {
    e.preventDefault();
    if (range?.start === n && range?.end === n && !seedBody) {
      setRange(null);
      return;
    }
    drag.current = { anchor: n, last: n, x: e.clientX, y: e.clientY, raf: 0 };
    setSelecting(true);
    setSeedBody("");
    setRange({ start: n, end: n });
    // Capture the pointer so we keep getting moves once the cursor leaves
    // this 16px button — pointerenter on sibling buttons is swallowed by
    // implicit capture, which is what broke dragging.
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* older engines: fall back to bubbling moves */
    }
  };

  // While held, grow the range to the line under the cursor. The captured
  // button receives every move; elementFromPoint maps the cursor to a real
  // line, so dragging anywhere over the diff works. One hit-test per frame,
  // not per event: elementFromPoint forces a layout flush, and the setRange
  // it feeds has just dirtied layout again (every row in the range gains a
  // shadow), so at coalesced trackpad rates the pair thrashes a body of
  // thousands of rows.
  const onPointerMove = (e: React.PointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (!d) return;
    d.x = e.clientX;
    d.y = e.clientY;
    if (d.raf) return;
    d.raf = requestAnimationFrame(() => {
      const cur = drag.current;
      if (!cur) return;
      cur.raf = 0;
      const row = document
        .elementFromPoint(cur.x, cur.y)
        ?.closest<HTMLElement>("[data-newline]");
      const m = row ? Number(row.dataset.newline) : NaN;
      if (Number.isNaN(m) || m === cur.last) return;
      cur.last = m;
      setRange({ start: Math.min(cur.anchor, m), end: Math.max(cur.anchor, m) });
    });
  };

  const submit = async (body: string) => {
    if (!range) return;
    await ipc.addDiffComment(
      prId,
      path,
      range.end,
      withMarker(body, seedMarker),
      range.start === range.end ? undefined : range.start,
    );
    close();
    onChanged();
  };

  return { path, range, selecting, seedBody, rangeText, seed, close, onPointerDown, onPointerMove, submit };
}
type LineComments = ReturnType<typeof useLineComments>;

/** One row of code with the review affordances hung off it: the + press
 *  target, the range highlight, any threads already on the line, and the
 *  composer under the bottom line of an open range. Only lines with a
 *  new-side number get the +: deleted lines have none, and GitHub can't
 *  anchor a comment to them in this app's model, so they carry an inert
 *  spacer instead. */
function CommentableLine({
  kind,
  n,
  text,
  threads,
  comments,
  onChanged,
}: {
  kind: DiffLine["kind"];
  /** New-side line number; null on deleted lines. */
  n: number | null;
  text: string;
  threads?: ReviewThread[];
  comments: LineComments;
  onChanged: () => void;
}) {
  const { path, range, selecting, seedBody } = comments;
  const commentable = n != null && kind !== "hunk";
  const inRange = n != null && range != null && n >= range.start && n <= range.end;
  // The composer hangs under the bottom line of the range, once the drag
  // that selected it has been released.
  const showComposer = n != null && range != null && range.end === n && !selecting;
  return (
    <div>
      <div
        className={`diff-line ${kind}${inRange ? " in-range" : ""}`}
        data-newline={commentable && n != null ? n : undefined}
      >
        <span className="code-lineno">{n ?? ""}</span>
        <span className="diff-gutter">{kind === "add" ? "+" : kind === "del" ? "−" : " "}</span>
        {commentable && n != null ? (
          <button
            className="line-comment-btn"
            data-tip={`Comment on line ${n} — drag to span several`}
            aria-label={`Comment on line ${n} — drag to span several`}
            onPointerDown={comments.onPointerDown(n)}
            onPointerMove={comments.onPointerMove}
          >
            +
          </button>
        ) : (
          // Same-width spacer so del lines' code aligns with add/context
          // lines (which carry the comment button).
          <span className="line-comment-btn ghost" aria-hidden="true" />
        )}
        {text}
      </div>
      {threads?.map((t) => (
        <InlineThread key={t.id} thread={t} onChanged={onChanged} lineText={text} />
      ))}
      {showComposer && range != null && (
        <div className="inline-thread">
          <Composer
            key={seedBody ? "seeded" : "manual"}
            placeholder={
              range.start === range.end
                ? `Comment on ${path}:${range.end}…`
                : `Comment on ${path}:${range.start}–${range.end}…`
            }
            submitLabel="Comment"
            autoFocus
            initialBody={seedBody}
            suggestionSeed={comments.rangeText(range)}
            onCancel={comments.close}
            onSubmit={comments.submit}
          />
        </div>
      )}
    </div>
  );
}

/** Inline threads live inside a max-content scroll layer, so % widths resolve
 *  against the widest code line, not the viewport — that's why they used to
 *  stop short. Publish the body's visible width as a variable the sticky
 *  thread reads, so it always spans exactly the scrollport. `mounted` is
 *  whatever gates the <pre>'s existence, so the observer re-attaches when it
 *  (re)appears. */
function useDiffVisibleWidth(bodyRef: React.RefObject<HTMLPreElement | null>, mounted: boolean) {
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const set = () => el.style.setProperty("--diff-visible-w", `${el.clientWidth}px`);
    set();
    const ro = new ResizeObserver(set);
    ro.observe(el);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mounted]);
}

function FileDiff({
  file,
  prId,
  threadsByLine,
  onChanged,
  viewed,
  onViewedChange,
  plan,
  focused,
  compose,
  onExpand,
  hideTrivial,
}: {
  file: DiffFile;
  prId: string;
  threadsByLine: Map<number, ReviewThread[]>;
  onChanged: () => void;
  viewed: boolean;
  onViewedChange: (viewed: boolean) => void;
  plan?: ReviewPlanEntry;
  /** File-rail navigation targeted this file — expand it. */
  focused?: boolean;
  /** Open a composer on this line, pre-filled (assessment finding → comment). */
  compose?: { line: number; body: string; marker?: string } | null;
  /** Pop the whole file open in the full-file drawer. */
  onExpand: () => void;
  /** Collapse import/housekeeping-only hunks behind a "show" row. */
  hideTrivial: boolean;
}) {
  const hasThreads = threadsByLine.size > 0;
  // Mechanical files (per the review plan) and whole-file deletions start
  // collapsed — the header tag says everything; expand on demand.
  const mechanical = plan?.significance === "mechanical";
  const defaultOpen =
    !viewed && !mechanical && !file.deleted && (file.lines.length <= 400 || hasThreads);
  const [open, setOpen] = useState(defaultOpen);

  // Marking viewed collapses the file (and vice versa), like GitHub.
  useEffect(() => {
    setOpen(defaultOpen);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewed]);

  useEffect(() => {
    if (focused) setOpen(true);
  }, [focused]);

  const lineTextByNew = useMemo(() => {
    const m = new Map<number, string>();
    for (const l of file.lines) {
      if (l.newLine != null && l.kind !== "hunk") m.set(l.newLine, l.text);
    }
    return m;
  }, [file]);
  const comments = useLineComments({ prId, path: file.path, lineTextByNew, onChanged });
  const { range } = comments;

  useEffect(() => {
    if (compose) {
      setOpen(true);
      comments.seed(compose.line, compose.body, compose.marker);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [compose]);

  const bodyRef = useRef<HTMLPreElement>(null);
  useDiffVisibleWidth(bodyRef, open);

  // Hunk segments: a header line plus its body, for per-hunk collapsing.
  // Cut once per file rather than on every render of the header.
  const segments = useMemo(() => {
    const segs: { header: DiffLine | null; lines: DiffLine[] }[] = [];
    for (const l of file.lines) {
      if (l.kind === "hunk") {
        segs.push({ header: l, lines: [] });
      } else {
        if (segs.length === 0) segs.push({ header: null, lines: [] });
        segs[segs.length - 1].lines.push(l);
      }
    }
    return segs;
  }, [file]);
  // Keys: `${segIndex}` for a whole hunk, `${segIndex}:${runIndex}` for a
  // trivial run inside a mixed hunk.
  const [revealed, setRevealed] = useState<Set<string>>(new Set());

  const renderLine = (l: DiffLine, key: string) => (
    <CommentableLine
      key={key}
      kind={l.kind}
      n={l.newLine}
      text={l.text}
      threads={l.newLine != null ? threadsByLine.get(l.newLine) : undefined}
      comments={comments}
      onChanged={onChanged}
    />
  );

  return (
    <div className={`diff-file${viewed ? " viewed" : ""}`} data-diff-path={file.path}>
      <div className="diff-file-header">
        <button className="diff-file-toggle" onClick={() => setOpen((o) => !o)}>
          <span className="chevron">{open ? "▾" : "▸"}</span>
          <span className="diff-path mono" data-tip={file.path}>
            {file.oldPath ? `${file.oldPath} → ` : ""}
            {file.path.includes("/") && (
              <span className="diff-path-dir">
                {file.path.slice(0, file.path.lastIndexOf("/") + 1)}
              </span>
            )}
            <span className="diff-path-name">
              {file.path.slice(file.path.lastIndexOf("/") + 1)}
            </span>
          </span>
          {file.deleted && (
            <span className="file-state-tag deleted" data-tip="This file was deleted in this PR">
              deleted
            </span>
          )}
          {file.added && (
            <span className="file-state-tag added" data-tip="This file is new in this PR">
              new
            </span>
          )}
          {plan && (
            <span className={`plan-chip ${plan.significance}`} data-tip={planTooltip(plan)}>
              {plan.significance}
            </span>
          )}
          {hasThreads && <span className="thread-tag">{threadsByLine.size} threads</span>}
          <span className="spacer" />
          <span className="diffstat mono">
            <span className="add">+{file.additions}</span>{" "}
            <span className="del">−{file.deletions}</span>
          </span>
        </button>
        <CopyButton text={() => filePatch(file)} what={`the diff for ${file.path}`} />
        {!file.deleted && (
          <button
            className="icon-btn file-expand-btn"
            {...tip("View the whole file with changes overlaid")}
            onClick={onExpand}
          >
            ⤢
          </button>
        )}
        <label className="viewed-check check-row" data-tip="Mark as viewed — clears if the file changes again">
          <input
            type="checkbox"
            checked={viewed}
            onChange={(e) => onViewedChange(e.target.checked)}
          />
          viewed
        </label>
      </div>
      {open && (
        <pre className="diff-body" ref={bodyRef}>
          <div className="diff-scroll-inner">
          {segments.map((seg, si) => {
            const changed = seg.lines.filter((l) => l.kind === "add" || l.kind === "del");
            // Never hide lines holding a live thread or an open composer.
            const pinnedIn = (lines: DiffLine[]) =>
              lines.some((l) => l.newLine != null && threadsByLine.has(l.newLine)) ||
              (range != null &&
                lines.some(
                  (l) => l.newLine != null && l.newLine >= range.start && l.newLine <= range.end,
                ));
            const hideChip = (key: string, count: number) => (
              <button
                key={key}
                className="trivial-collapsed"
                data-tip="Import/housekeeping-only changes, hidden to save review time"
                onClick={() => setRevealed((prev) => new Set(prev).add(key))}
              >
                ⋯ {count} import/housekeeping line{count === 1 ? "" : "s"} hidden — show
              </button>
            );
            const collapsed =
              hideTrivial &&
              !pinnedIn(seg.lines) &&
              !revealed.has(`${si}`) &&
              isTrivialHunk(seg.lines);
            return (
              <div key={si}>
                {seg.header && <DiffJump text={seg.header.text} />}
                {collapsed
                  ? hideChip(`${si}`, changed.length)
                  : hideTrivial
                    ? // Mixed hunk: collapse just its trivial import runs.
                      splitTrivialRuns(seg.lines).map((run, ri) => {
                        const key = `${si}:${ri}`;
                        if (run.trivial && !pinnedIn(run.lines) && !revealed.has(key)) {
                          const n = run.lines.filter(
                            (l) => l.kind === "add" || l.kind === "del",
                          ).length;
                          return hideChip(key, n);
                        }
                        return run.lines.map((l, li) => renderLine(l, `${key}:${li}`));
                      })
                    : seg.lines.map((l, li) => renderLine(l, `${si}:${li}`))}
              </div>
            );
          })}
          </div>
        </pre>
      )}
    </div>
  );
}

// Stable empty fallback — a fresh [] each render would invalidate the whole
// planByPath → files memo chain on every analysis progress tick.
const NO_PLAN: ReviewPlanEntry[] = [];


export function DiffView({ prId, headSha }: { prId: string; headSha: string }) {
  // Raw diff + viewed-file digests live in the shared store (the file rail
  // reads the same entry), keyed to this head.
  const entry = useDiffStore((s) => s.entries[prId]);
  const ensureDiff = useDiffStore((s) => s.ensure);
  const storeSetViewed = useDiffStore((s) => s.setViewed);
  const current = entry?.headSha === headSha ? entry : undefined;
  const raw = current?.status === "done" ? current.raw : null;
  const error = current?.status === "error" ? current.error : null;
  const [conversation, setConversation] = useState<PrConversation | null>(null);
  const [ignoreGlobs, setIgnoreGlobs] = useState<string[]>([]);
  const [showSkipped, setShowSkipped] = useState(false);
  const [mark, setMark] = useState<ReviewMark | null>(null);
  const [sinceMode, setSinceMode] = useState(false);
  const [sinceRaw, setSinceRaw] = useState<string | null>(null);
  const [sinceFailed, setSinceFailed] = useState(false);
  /** A finding-seeded composer waiting to open at (file, line). */
  const [seededCompose, setSeededCompose] = useState<{
    path: string;
    line: number;
    body: string;
    marker?: string;
  } | null>(null);
  /** File taken over the panel in the full-file view. */
  const [expanded, setExpanded] = useState<DiffFile | null>(null);
  // Import/housekeeping-only hunks collapse behind "show" rows — persisted.
  const [hideTrivial, toggleTrivial] = usePersistedFlag("cora.hideTrivialHunks", true);
  // Soft-wrap long lines instead of scrolling them off the right edge.
  const [wrap, toggleWrap] = usePersistedFlag("cora.wrapDiffLines");
  const rootRef = useRef<HTMLDivElement>(null);
  /** Scroll position of the panel before the full-file view took over. */
  const savedScroll = useRef(0);

  const scroller = () => rootRef.current?.closest(".content");

  const openExpanded = (f: DiffFile) => {
    savedScroll.current = scroller()?.scrollTop ?? 0;
    setExpanded(f);
    scroller()?.scrollTo({ top: 0 });
  };
  const closeExpanded = () => {
    setExpanded(null);
    requestAnimationFrame(() => {
      const sc = scroller();
      if (sc) sc.scrollTop = savedScroll.current;
    });
  };

  // Per-file significance from the L1 analysis, when one exists for this
  // head. Select only the result so progress ticks don't re-render the diff.
  const ensureAnalysis = useAnalysisStore((s) => s.ensure);
  const l1Result = useAnalysisStore((s) => s.runs[analysisKey(prId, "context")]?.result);
  const reviewPlan =
    l1Result?.headSha === headSha ? l1Result.assessment.reviewPlan : NO_PLAN;

  const loadComments = () =>
    void ipc
      .getPrComments(prId)
      .then(setConversation)
      .catch(() => setConversation(null)); // comments are enrichment; diff still shows

  useEffect(() => {
    setSinceMode(false);
    setSinceRaw(null);
    setSinceFailed(false);
    setSeededCompose(null);
    setExpanded(null);
    useDiffStore.getState().setVisiblePath(null);
    void ensureDiff(prId, headSha);
    loadComments();
    void ipc.getSettings().then((s) => setIgnoreGlobs(s.reviewIgnoreGlobs));
    void ensureAnalysis(prId, "context", undefined, false).catch(() => {});
    // The backend plants the mark on first look; later heads show the
    // "changed since your last look" banner against it.
    void ipc
      .ensureReviewMark(prId)
      .then(setMark)
      .catch(() => setMark(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prId, headSha]);

  const stale = mark != null && mark.headSha !== headSha;

  const toggleSince = () => {
    if (sinceMode) {
      setSinceMode(false);
      return;
    }
    setSinceMode(true);
    if (sinceRaw === null && !sinceFailed) {
      void ipc
        .getDiffSince(prId)
        .then(setSinceRaw)
        .catch(() => {
          // Force-pushes can orphan the marked SHA; fall back to the full diff.
          setSinceMode(false);
          setSinceFailed(true);
        });
    }
  };

  const caughtUp = () => {
    void ipc.setReviewMark(prId).then(setMark).catch(() => {});
    setSinceMode(false);
    setSinceRaw(null);
    setSinceFailed(false);
  };

  // A file counts as viewed only while its patch digest still matches — an
  // update after viewing clears it automatically. Digests are ALWAYS taken
  // from the full diff, never the "since your last look" subset: a mark made
  // in since-mode must survive leaving it (and vice versa).
  const fullDigests = useMemo(
    () => new Map((raw ? parseDiffCached(raw) : []).map((f) => [f.path, fileDigest(f)])),
    [raw],
  );

  const activeRaw = sinceMode && sinceRaw !== null ? sinceRaw : raw;
  const allFiles = useMemo(() => {
    const parsed = activeRaw ? parseDiffCached(activeRaw) : [];
    if (activeRaw === raw) return parsed;
    // The since-diff is `compare/{marked sha}...{head}`, so it carries every
    // change that landed on the branch — including the base-branch churn that
    // arrives when the author merges main. Those files aren't part of this PR:
    // they'd never appear in the full diff, so they have no digest to hold a
    // viewed mark, and the checkbox would silently do nothing. Reviewing means
    // reviewing the PR's own changes, so they don't belong in the list either.
    return parsed.filter((f) => fullDigests.has(f.path));
  }, [activeRaw, raw, fullDigests]);

  // Insignificant files (lockfiles, generated, snapshots) are skipped:
  // parked in their own collapsed section and excluded from progress.
  const planByPath = useMemo(
    () => new Map(reviewPlan.map((p) => [p.path, p])),
    [reviewPlan],
  );
  const { files, skipped } = useMemo(() => {
    const significant: DiffFile[] = [];
    const insignificant: DiffFile[] = [];
    for (const f of allFiles) {
      (matchesAny(f.path, ignoreGlobs) ? insignificant : significant).push(f);
    }
    // Same order as the file rail's tree — the two panes always agree, so
    // the rail's scroll-spy highlight tracks the list one-to-one.
    const order = new Map(treeFileOrder(allFiles).map((p, i) => [p, i]));
    significant.sort((a, b) => (order.get(a.path) ?? 0) - (order.get(b.path) ?? 0));
    return { files: significant, skipped: insignificant };
  }, [allFiles, ignoreGlobs]);

  const isViewed = (file: DiffFile) => {
    const digest = fullDigests.get(file.path);
    return digest != null && entry?.viewed[file.path] === digest;
  };
  const setViewed = (file: DiffFile, viewed: boolean) => {
    const digest = fullDigests.get(file.path);
    if (!digest) return;
    storeSetViewed(prId, file.path, digest, viewed);
    if (!viewed) return;
    // Marking collapses the file and rearranges the layout under the scroll
    // position — give the scroll an explicit destination instead: the next
    // unviewed file after this one, wrapping to the top.
    const ordered = showSkipped ? [...files, ...skipped] : files;
    if (ordered.length === 0) return;
    const i = ordered.findIndex((f) => f.path === file.path);
    const next = [...ordered.slice(i + 1), ...ordered.slice(0, Math.max(i, 0))].find(
      (f) => f.path !== file.path && !isViewed(f),
    );
    if (next) {
      useDiffStore.getState().requestFocusFile(next.path);
      return;
    }
    // That was the last unviewed file — there's nothing left to jump to.
    // Ease back to the top so finishing the review reads as a deliberate
    // return rather than a jarring snap.
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    scroller()?.scrollTo({ top: 0, behavior: reduce ? "auto" : "smooth" });
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

  // A "± comment" click on an assessment finding: open a pre-filled composer
  // on the target file's first changed line and bring it into view.
  const composeRequest = useDiffStore((s) => s.composeRequest);
  useEffect(() => {
    if (composeRequest?.target !== "diff" || raw === null) return;
    const file = allFiles.find((f) => f.path === composeRequest.path);
    if (!file) {
      // Path isn't in this diff (stale or hallucinated) — drop the request.
      useDiffStore.getState().clearCompose();
      return;
    }
    const requested = composeRequest.line ?? null;
    const line =
      (requested != null &&
      file.lines.some((l) => l.newLine === requested && l.kind !== "hunk")
        ? requested
        : null) ??
      file.lines.find((l) => l.kind === "add" && l.newLine != null)?.newLine ??
      file.lines.find((l) => l.newLine != null)?.newLine ??
      1;
    if (skipped.some((f) => f.path === file.path)) setShowSkipped(true);
    setSeededCompose({
      path: file.path,
      line,
      body: composeRequest.seed,
      marker: composeRequest.marker,
    });
    useDiffStore.getState().clearCompose();
    requestAnimationFrame(() => {
      document
        .querySelector(`[data-diff-path="${CSS.escape(file.path)}"]`)
        ?.scrollIntoView({ block: "start" });
    });
  }, [composeRequest, raw, allFiles, skipped]);

  // File-rail navigation: leave any full-file view, expand the target file,
  // reveal the skipped section if it lives there, and scroll it to the top.
  const focusPath = useDiffStore((s) => s.focusPath);
  useEffect(() => {
    if (!focusPath || raw === null) return;
    setExpanded(null);
    if (skipped.some((f) => f.path === focusPath)) setShowSkipped(true);
    // Two frames: focus often arrives together with a layout change above the
    // target (marking a file viewed collapses it) — let that render and paint
    // first, then measure and scroll, or we land off by the collapsed height.
    let inner = 0;
    const id = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => {
        document
          .querySelector(`[data-diff-path="${CSS.escape(focusPath)}"]`)
          ?.scrollIntoView({ block: "start" });
        useDiffStore.getState().clearFocusFile();
      });
    });
    return () => {
      cancelAnimationFrame(id);
      cancelAnimationFrame(inner);
    };
  }, [focusPath, raw, skipped]);

  // Scroll spy: report the file at the top of the viewport so the rail's
  // highlight follows the reading position.
  useEffect(() => {
    const sc = scroller();
    const root = rootRef.current;
    if (!sc || !root || expanded) return;
    let frame = 0;
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const top = sc.getBoundingClientRect().top;
        let current: string | null = null;
        for (const el of root.querySelectorAll<HTMLElement>("[data-diff-path]")) {
          // A file is "current" once its header has reached the sticky zone.
          if (el.getBoundingClientRect().top <= top + 48) {
            current = el.dataset.diffPath ?? null;
          } else {
            break;
          }
        }
        if (current) useDiffStore.getState().setVisiblePath(current);
      });
    };
    sc.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      sc.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(frame);
    };
  }, [files.length, expanded]);

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

  if (expanded) {
    return (
      <div className={`diff-view${wrap ? " wrap" : ""}`} ref={rootRef}>
        <FullFileView
          prId={prId}
          file={expanded}
          threadsByLine={threadsByFile.get(expanded.path) ?? new Map()}
          onChanged={loadComments}
          onClose={closeExpanded}
        />
      </div>
    );
  }

  return (
    <div className={`diff-view${wrap ? " wrap" : ""}`} ref={rootRef}>
      {stale && (
        <div className="since-banner">
          <span>
            This PR changed since your last look ({timeAgo(mark.at)} ago).
          </span>
          <span className="spacer" />
          <button className={`since-toggle${sinceMode ? " active" : ""}`} onClick={toggleSince}>
            {sinceMode ? "Show full diff" : "Show only new changes"}
          </button>
          <button className="since-toggle" onClick={caughtUp}>
            I'm caught up
          </button>
        </div>
      )}
      <div className="eyebrow diff-summary">
        <span>
          {sinceMode && sinceRaw !== null ? "since your last look · " : ""}
          {files.length} files ·{" "}
          <span className="add">+{files.reduce((n, f) => n + f.additions, 0)}</span>{" "}
          <span className="del">−{files.reduce((n, f) => n + f.deletions, 0)}</span>
          {" · "}
          <span className={viewedCount === files.length ? "add" : undefined}>
            {viewedCount}/{files.length} viewed
          </span>
          {" · hover a line and hit + to comment"}
        </span>
        <span className="spacer" />
        <button
          className={`chip${hideTrivial ? " on" : ""}`}
          data-tip="Collapse hunks that only touch imports/housekeeping — the whole-file view (⤢) always shows everything"
          onClick={toggleTrivial}
        >
          hide import-only hunks
        </button>
        <button
          className={`chip${wrap ? " on" : ""}`}
          data-tip="Soft-wrap long lines instead of scrolling them sideways"
          onClick={toggleWrap}
        >
          wrap lines
        </button>
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
          plan={planByPath.get(f.path)}
          focused={focusPath === f.path}
          compose={seededCompose?.path === f.path ? seededCompose : null}
          onExpand={() => openExpanded(f)}
          hideTrivial={hideTrivial}
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
                  focused={focusPath === f.path}
                  compose={seededCompose?.path === f.path ? seededCompose : null}
                  onExpand={() => openExpanded(f)}
          hideTrivial={hideTrivial}
                />
              </div>
            ))}
        </div>
      )}
    </div>
  );
}

interface FullLine {
  kind: "add" | "del" | "ctx";
  text: string;
  n: number | null;
}

/** Overlay the diff onto the full file at head: added lines highlighted in
 *  place, deleted lines interleaved where they used to sit (anchored to the
 *  new-side line each deletion precedes). */
function mergeFullFile(content: string, file: DiffFile): FullLine[] {
  const adds = new Set<number>();
  const delsBefore = new Map<number, string[]>();
  let pos = 1;
  for (const l of file.lines) {
    if (l.kind === "hunk") {
      const m = l.text.match(/\+(\d+)/);
      if (m) pos = Number(m[1]);
    } else if (l.kind === "del") {
      delsBefore.set(pos, [...(delsBefore.get(pos) ?? []), l.text]);
    } else if (l.newLine != null) {
      if (l.kind === "add") adds.add(l.newLine);
      pos = l.newLine + 1;
    }
  }
  const out: FullLine[] = [];
  const lines = content.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const n = i + 1;
    for (const d of delsBefore.get(n) ?? []) out.push({ kind: "del", text: d, n: null });
    out.push({ kind: adds.has(n) ? "add" : "ctx", text: lines[i], n });
  }
  for (const d of delsBefore.get(lines.length + 1) ?? []) {
    out.push({ kind: "del", text: d, n: null });
  }
  return out;
}

/** Whole-file takeover of the diff panel: full contents at the PR head with
 *  the diff overlaid, auto-scrolled to the first change. Every line with a
 *  new-side number takes review comments exactly as it does in the hunk
 *  view, and the file's open threads sit under their lines here too. Closing
 *  restores the file list where you left it. */
function FullFileView({
  prId,
  file,
  threadsByLine,
  onChanged,
  onClose,
}: {
  prId: string;
  file: DiffFile;
  threadsByLine: Map<number, ReviewThread[]>;
  onChanged: () => void;
  onClose: () => void;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const bodyRef = useRef<HTMLPreElement>(null);
  useDiffVisibleWidth(bodyRef, content !== null);

  useEffect(() => {
    setContent(null);
    setError(null);
    void ipc
      .getFileAtHead(prId, file.path)
      .then(setContent)
      .catch((e) => setError(String(e)));
  }, [prId, file.path]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const merged = useMemo(
    () => (content != null ? mergeFullFile(content, file) : []),
    [content, file],
  );
  // The whole file is here, so every new-side line has text for a suggestion.
  const lineTextByNew = useMemo(() => {
    const m = new Map<number, string>();
    for (const l of merged) if (l.n != null) m.set(l.n, l.text);
    return m;
  }, [merged]);
  const comments = useLineComments({ prId, path: file.path, lineTextByNew, onChanged });

  useEffect(() => {
    if (merged.length === 0) return;
    requestAnimationFrame(() => {
      bodyRef.current
        ?.querySelector(".diff-line.add, .diff-line.del")
        ?.scrollIntoView({ block: "center" });
    });
  }, [merged]);

  const dirIdx = file.path.lastIndexOf("/");
  return (
    <div className="diff-file full-file">
      <div className="diff-file-header">
        <span className="diff-path mono" data-tip={file.path}>
          {dirIdx >= 0 && (
            <span className="diff-path-dir">{file.path.slice(0, dirIdx + 1)}</span>
          )}
          <span className="diff-path-name">{file.path.slice(dirIdx + 1)}</span>
        </span>
        <span className="eyebrow">whole file</span>
        <span className="spacer" />
        <span className="diffstat mono">
          <span className="add">+{file.additions}</span>{" "}
          <span className="del">−{file.deletions}</span>
        </span>
        <CopyButton text={content} what={`the contents of ${file.path}`} />
        <button className="icon-btn" {...tip("Back to the diff (esc)")} onClick={onClose}>
          ✕
        </button>
      </div>
      {error && <div className="analysis-error">{error}</div>}
      {content === null && !error && <div className="canvas-loading">fetching file…</div>}
      {content !== null && (
        <pre className="diff-body" ref={bodyRef}>
          <div className="diff-scroll-inner">
            {merged.map((l, i) => (
              <CommentableLine
                key={i}
                kind={l.kind}
                n={l.n}
                text={l.text}
                threads={l.n != null ? threadsByLine.get(l.n) : undefined}
                comments={comments}
                onChanged={onChanged}
              />
            ))}
          </div>
        </pre>
      )}
    </div>
  );
}
