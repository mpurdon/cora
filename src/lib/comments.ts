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
