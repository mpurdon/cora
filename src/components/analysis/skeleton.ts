import type { AnalysisResult } from "../../bindings/AnalysisResult";
import type { C4Graph } from "../../bindings/C4Graph";
import type { C4Node } from "../../bindings/C4Node";
import type { ChangeStatus } from "../../bindings/ChangeStatus";
import { matchFiles } from "./DiffPeek";
import type { DiffFile } from "./DiffView";

/** Focus the context graph on one system: the system, everything nested
 *  inside it, and whatever connects to any of that. Pure navigation over the
 *  result we already have — drilling a system never costs a model run. */
export function filterGraph(graph: C4Graph, systemId: string): C4Graph {
  const keep = new Set<string>([systemId]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const n of graph.nodes) {
      if (!keep.has(n.id) && n.boundary && keep.has(n.boundary)) {
        keep.add(n.id);
        grew = true;
      }
    }
  }
  const visible = new Set(keep);
  for (const e of graph.edges) {
    if (keep.has(e.source)) visible.add(e.target);
    if (keep.has(e.target)) visible.add(e.source);
  }
  return {
    nodes: graph.nodes.filter((n) => visible.has(n.id)),
    edges: graph.edges.filter((e) => visible.has(e.source) && visible.has(e.target)),
  };
}

/** The diff files a drilled node concerns; when the matcher finds nothing
 *  (name/path drift), fall back to the whole diff — a too-wide sketch beats
 *  an empty canvas. */
function scopeFiles(files: DiffFile[], node: C4Node): DiffFile[] {
  const matched = matchFiles(files, node);
  return matched.length > 0 ? matched : files;
}

/** Directory a file's component-level grouping hangs off: the last two
 *  meaningful path segments ("components/DocumentViewerPanel"). */
function moduleKey(path: string): string {
  const dir = path.slice(0, Math.max(0, path.lastIndexOf("/")));
  if (!dir) return "(root)";
  const segments = dir.split("/").filter((s) => !s.startsWith("[") && s !== "src" && s !== "app");
  return segments.slice(-2).join("/") || dir.split("/").slice(-2).join("/");
}

function groupChange(group: DiffFile[]): ChangeStatus {
  if (group.every((f) => f.added)) return "added";
  if (group.every((f) => f.deleted)) return "removed";
  return "modified";
}

const SIG_RANK: Record<string, number> = { critical: 0, important: 1, mechanical: 2 };

/** Instant sketch of a container's components: its changed files grouped
 *  into modules by directory, ranked by the review plan. The scoped agentic
 *  run replaces this with real relationships when it lands — the sketch
 *  exists so drilling never blocks on the model. */
export function componentSkeleton(
  container: C4Node,
  context: AnalysisResult,
  files: DiffFile[],
): C4Graph {
  const scoped = scopeFiles(files, container);
  const plan = new Map(context.assessment.reviewPlan.map((p) => [p.path, p]));
  const groups = new Map<string, DiffFile[]>();
  for (const f of scoped) {
    const key = moduleKey(f.path);
    groups.set(key, [...(groups.get(key) ?? []), f]);
  }
  const nodes: C4Node[] = [{ ...container, boundary: null }];
  for (const [key, group] of groups) {
    const adds = group.reduce((n, f) => n + f.additions, 0);
    const dels = group.reduce((n, f) => n + f.deletions, 0);
    const worst = group
      .map((f) => plan.get(f.path)?.significance)
      .filter((s): s is NonNullable<typeof s> => s != null)
      .sort((a, b) => (SIG_RANK[a] ?? 3) - (SIG_RANK[b] ?? 3))[0];
    nodes.push({
      id: `component:${key}`,
      name: key,
      kind: "component",
      technology: null,
      description: `${group.length} file${group.length === 1 ? "" : "s"} · +${adds} −${dels}${
        worst ? ` · ${worst}` : ""
      }`,
      boundary: container.id,
      change: groupChange(group),
    });
  }
  return { nodes, edges: [] };
}

/** Instant sketch of a component's code level: its diff files as code
 *  nodes, annotated with code-finding counts from the context pass. */
export function codeSkeleton(
  component: C4Node,
  context: AnalysisResult,
  files: DiffFile[],
): C4Graph {
  const scoped = scopeFiles(files, component);
  const findings = new Map<string, number>();
  for (const f of context.codeFindings) {
    findings.set(f.path, (findings.get(f.path) ?? 0) + 1);
  }
  const nodes: C4Node[] = [{ ...component, boundary: null }];
  for (const f of scoped) {
    const n = findings.get(f.path) ?? 0;
    nodes.push({
      id: `code:${f.path}`,
      name: f.path.split("/").pop() ?? f.path,
      kind: "code",
      technology: null,
      description: `+${f.additions} −${f.deletions}${
        n > 0 ? ` · ${n} finding${n === 1 ? "" : "s"}` : ""
      }`,
      boundary: component.id,
      change: f.added ? "added" : f.deleted ? "removed" : "modified",
    });
  }
  return { nodes, edges: [] };
}
