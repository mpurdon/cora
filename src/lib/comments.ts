import type { CodeFinding } from "../bindings/CodeFinding";
import type { PrConversation } from "../bindings/PrConversation";

/** First sentence of a possibly-paragraph-length text. Findings from older
 *  analyses can be essays; the PR author gets the headline while the full
 *  reasoning stays visible inside the app. */
function firstSentence(text: string): string {
  const m = text.match(/^.*?(?<!\be\.g)(?<!\bi\.e)\.(?=\s|$)/s);
  return (m ? m[0] : text).trim();
}

/** Draft comment seeded from a code finding — deliberately short and in a
 *  plain human voice: one sentence of what, one sentence of fix, no
 *  machine-looking `kind · severity` label or arrow (reviewers strip those).
 *  The composer lets the reviewer expand before posting. */
export function findingSeed(f: CodeFinding): string {
  return `${firstSentence(f.finding)}\n\n${firstSentence(f.suggestion)}`;
}

/** Where the viewer has already commented, from the live PR conversation:
 *  - `lines`: exact "path:line" keys, for line-anchored findings.
 *  - `files`: paths commented on at all, for file-level (null-line) findings —
 *    the composer anchors those to the file's first changed line, a real line
 *    we can't reconstruct, so any comment from the viewer on the file counts.
 *  Matched by location, not comment body: the reviewer edits the seeded draft
 *  (e.g. strips the label) before posting, so text is not a reliable key. */
export function viewerComments(
  conversation: PrConversation | null,
  viewer: string,
): { lines: Set<string>; files: Set<string> } {
  const lines = new Set<string>();
  const files = new Set<string>();
  if (!viewer) return { lines, files };
  for (const t of conversation?.threads ?? []) {
    if (!t.path) continue;
    if (!t.comments.some((c) => c.author === viewer)) continue;
    files.add(t.path);
    const a = t.startLine ?? t.line;
    const b = t.line ?? t.startLine;
    if (a == null || b == null) continue;
    for (let ln = Math.min(a, b); ln <= Math.max(a, b); ln++) {
      lines.add(`${t.path}:${ln}`);
    }
  }
  return { lines, files };
}

/** Whether a finding has been addressed by a viewer comment at its location. */
export function isFindingCommented(
  f: CodeFinding,
  mine: { lines: Set<string>; files: Set<string> },
): boolean {
  return f.line != null ? mine.lines.has(`${f.path}:${f.line}`) : mine.files.has(f.path);
}

/** Join file basenames into a natural clause, capped so it stays one sentence:
 *  "A" · "A and B" · "A, B and C" · "A, B and 2 others". */
function joinFiles(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  if (names.length === 3) return `${names[0]}, ${names[1]} and ${names[2]}`;
  return `${names[0]}, ${names[1]} and ${names.length - 2} others`;
}

/** A one-sentence request-changes summary built from the viewer's inline
 *  comments — a pointer to where the changes are, not their substance.
 *  Empty when the viewer hasn't left any file-anchored comments (nothing to
 *  point at — let them write their own). */
export function requestChangesSeed(conversation: PrConversation | null, viewer: string): string {
  if (!viewer) return "";
  const byFile = new Map<string, number>();
  for (const t of conversation?.threads ?? []) {
    if (!t.path) continue;
    const mine = t.comments.filter((c) => c.author === viewer).length;
    if (mine === 0) continue;
    byFile.set(t.path, (byFile.get(t.path) ?? 0) + mine);
  }
  const total = [...byFile.values()].reduce((a, b) => a + b, 0);
  if (total === 0) return "";
  const files = joinFiles([...byFile.keys()].map((p) => p.slice(p.lastIndexOf("/") + 1)));
  return `Requesting changes — see my ${total} comment${total === 1 ? "" : "s"} on ${files}.`;
}

/** Mirrors the Rust-side gate (models::is_non_blocking_comment): threads
 *  opened as praise/note/fyi or marked (non-blocking) don't hold up approval,
 *  so neither the approve gate nor the approval summary treats them as work. */
export function isNonBlockingComment(body: string): boolean {
  const first = (body.trimStart().split("\n")[0] ?? "")
    .replace(/[*_`~]/g, "")
    .trimStart()
    .toLowerCase();
  return (
    first.startsWith("praise:") ||
    first.startsWith("note:") ||
    first.startsWith("fyi:") ||
    first.includes("(non-blocking)") ||
    first.includes("non-blocking:")
  );
}

/** A one-sentence approval summary, the counterpart to `requestChangesSeed`:
 *  what your review actually did — the comments of yours that got addressed,
 *  and any non-blocking notes you're leaving behind. Only counts threads you
 *  started, so it never claims credit for someone else's review, and falls
 *  back to a plain sign-off when you left nothing to point at (an approval
 *  always says something — silence reads as a rubber stamp). */
export function approveSeed(conversation: PrConversation | null, viewer: string): string {
  const addressed = new Map<string, number>();
  const notes = new Map<string, number>();
  for (const t of conversation?.threads ?? []) {
    const root = t.comments[0];
    if (!t.path || !viewer || root?.author !== viewer) continue;
    // Resolved (or outdated — the code moved under it) means the author acted
    // on it. Anything of mine still open got past the approve gate, so it's a
    // note by construction; a blocking one would have disabled Approve.
    const settled = t.resolved || t.outdated;
    if (!settled && !isNonBlockingComment(root.body)) continue;
    const bucket = settled ? addressed : notes;
    const name = t.path.slice(t.path.lastIndexOf("/") + 1);
    // One per thread, not per comment: a point you raised counts once however
    // much back-and-forth it took to settle.
    bucket.set(name, (bucket.get(name) ?? 0) + 1);
  }
  const count = (m: Map<string, number>) => [...m.values()].reduce((a, b) => a + b, 0);
  const nDone = count(addressed);
  const nNotes = count(notes);

  const clauses: string[] = [];
  if (nDone > 0) {
    clauses.push(
      `my ${nDone} comment${nDone === 1 ? " on" : "s on"} ${joinFiles([...addressed.keys()])} ${
        nDone === 1 ? "is" : "are"
      } addressed`,
    );
  }
  if (nNotes > 0) {
    clauses.push(
      `${nNotes} non-blocking note${nNotes === 1 ? "" : "s"} on ${joinFiles([...notes.keys()])} — ${
        nNotes === 1 ? "take it or leave it" : "take them or leave them"
      }`,
    );
  }
  if (clauses.length === 0) return "Approving — nothing blocking from me.";
  return `Approving — ${clauses.join("; ")}.`;
}
