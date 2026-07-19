import type { CodeFinding } from "../bindings/CodeFinding";

/** First sentence of a possibly-paragraph-length text. Findings from older
 *  analyses can be essays; the PR author gets the headline while the full
 *  reasoning stays visible inside the app. */
function firstSentence(text: string): string {
  const m = text.match(/^.*?(?<!\be\.g)(?<!\bi\.e)\.(?=\s|$)/s);
  return (m ? m[0] : text).trim();
}

/** Draft comment seeded from a code finding — deliberately short: one
 *  sentence of what, one imperative sentence of fix. The composer lets the
 *  reviewer expand before posting. */
export function findingSeed(f: CodeFinding): string {
  return `**${f.kind} · ${f.severity}**: ${firstSentence(f.finding)}\n\n→ ${firstSentence(f.suggestion)}`;
}
