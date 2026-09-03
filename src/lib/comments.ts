import { fileName } from "./fileTree";
import type { BoundaryImpact } from "../bindings/BoundaryImpact";
import type { CodeFinding } from "../bindings/CodeFinding";
import type { ImpactKind } from "../bindings/ImpactKind";
import type { Pillar } from "../bindings/Pillar";
import type { PrConversation } from "../bindings/PrConversation";
import type { Settings } from "../bindings/Settings";
import type { WaFinding } from "../bindings/WaFinding";

/** Human names for the assessment's enum tags, shared by the rows that show
 *  them and the text (comment seeds, explain prompts) built from them. */
export const IMPACT_LABEL: Record<ImpactKind, string> = {
  external: "external system",
  service: "service boundary",
  internal: "internal",
};

export const PILLAR_LABEL: Record<Pillar, string> = {
  "operational-excellence": "Operational excellence",
  security: "Security",
  reliability: "Reliability",
  "performance-efficiency": "Performance efficiency",
  "cost-optimization": "Cost optimization",
  sustainability: "Sustainability",
};

/** Anything the "explain" button can hand to the assistant: a line-anchored
 *  code finding, a Well-Architected finding, or a boundary impact. The
 *  generated bindings carry no tag, so the three are told apart by shape —
 *  each has a key the other two lack. */
export type Explainable = CodeFinding | WaFinding | BoundaryImpact;

export function isCodeFinding(f: Explainable): f is CodeFinding {
  return "path" in f;
}

export function isWaFinding(f: Explainable): f is WaFinding {
  return "pillar" in f;
}

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

/** The C4 node ids a finding points at, as a prompt line — or nothing when
 *  the model gave none. Named so the assistant can look the nodes up in the
 *  graph rather than guess which code the finding is about. */
function nodeLine(nodeIds: string[]): string[] {
  if (nodeIds.length === 0) return [];
  return [`Architecture nodes (C4 graph ids): ${nodeIds.map((id) => `\`${id}\``).join(", ")}`];
}

/** The message the "explain" button sends to the assistant. Asks for a
 *  plain-language read that's actionable in a single turn — but terse: it lands
 *  in a narrow side panel, so the prompt enforces short answers under fixed
 *  headings, no preamble or restating the code, so the reviewer can act (edit,
 *  comment, or dismiss) without wading through prose or a clarifying round-trip.
 *
 *  The three finding shapes differ only in how they're grounded: a code finding
 *  names a path and line to read; a Well-Architected finding or boundary
 *  impact has no location, so the prompt names the C4 nodes it touches and asks
 *  the assistant to find the code behind them with its tools. */
export function eli5Prompt(f: Explainable): string {
  let ask: string;
  let context: string[];
  let ground: string;
  if (isCodeFinding(f)) {
    const where = f.line != null ? `${f.path}:${f.line}` : f.path;
    ask = `Explain this ${f.kind} finding in plain language and tell me what to do.`;
    context = [`Finding (${f.severity}) at ${where}:`, `> ${f.finding}`, `Suggested fix: ${f.suggestion}`];
    ground = `Read the code at ${where} to ground it`;
  } else if (isWaFinding(f)) {
    const from = f.nodeIds.length > 0 ? " (start from the nodes above)" : "";
    ask = `Explain this Well-Architected finding in plain language and tell me what to do.`;
    context = [
      `Finding (${f.severity}, ${PILLAR_LABEL[f.pillar]} pillar):`,
      `> ${f.finding}`,
      `Recommendation: ${f.recommendation}`,
      ...nodeLine(f.nodeIds),
    ];
    ground = `Use your tools to find and read the code this is about${from} to ground it`;
  } else {
    const from = f.nodeIds.length > 0 ? " (start from the nodes above)" : "";
    ask = `Explain this ${IMPACT_LABEL[f.kind]} boundary impact in plain language and tell me what to do.`;
    context = [`Impact (${IMPACT_LABEL[f.kind]}):`, `> ${f.description}`, ...nodeLine(f.nodeIds)];
    ground = `Use your tools to find and read the code on both sides of this boundary${from} to ground it`;
  }
  return [
    `${ask} Be terse — this lands in a narrow side panel.`,
    ``,
    ...context,
    ``,
    `${ground}, then answer under these exact headings — one or two sentences each, no preamble, no sign-off, don't restate the code back to me:`,
    `**What it is** — the problem in everyday terms.`,
    `**Why it matters** — the concrete consequence, or say plainly if it's low-stakes or fine to leave.`,
    `**Do on this PR?** — yes or no, and if yes the exact change in a sentence or two (a short snippet only if that's the fastest way to say it).`,
  ].join("\n");
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
  const mine = myCommentsSummary(conversation, viewer);
  return mine ? `Requesting changes — see my ${mine}.` : "";
}

/** The comment-review seed: the same pointer to your line comments, with no
 *  verdict in front of it. Empty when you haven't commented, so the box asks
 *  you to say something — GitHub won't take a comment review with no body. */
export function commentReviewSeed(conversation: PrConversation | null, viewer: string): string {
  const mine = myCommentsSummary(conversation, viewer);
  return mine ? `See my ${mine}.` : "";
}

/** "3 comments on a.ts and b.ts" — your own line comments, counted per file. */
function myCommentsSummary(conversation: PrConversation | null, viewer: string): string {
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
  const files = joinFiles([...byFile.keys()].map(fileName));
  return `${total} comment${total === 1 ? "" : "s"} on ${files}`;
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
    const name = fileName(t.path);
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

/** Approve-composer seed, in precedence order: this repo's override, else
 *  the global default, else the dynamic per-PR summary. The one place that
 *  chain lives — callers should use this instead of `approveSeed` directly. */
export function resolveApproveMessage(
  repo: string,
  settings: Settings,
  conversation: PrConversation | null,
  viewer: string,
): string {
  const repoOverride = settings.repoApproveMessages[repo]?.trim();
  if (repoOverride) return repoOverride;
  const globalDefault = settings.defaultApproveMessage.trim();
  if (globalDefault) return globalDefault;
  return approveSeed(conversation, viewer);
}
