import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import type { TrackedPr } from "../bindings/TrackedPr";
import {
  buildTree,
  countFiles,
  fileName,
  sortedDirs,
  sortedFiles,
  treeFileOrder,
  type TreeDir,
} from "../lib/fileTree";
import { matchesAny } from "../lib/globs";
import { ipc } from "../lib/ipc";
import { analysisKey, useAnalysisStore } from "../state/analysisStore";
import { useDiffStore } from "../state/diffStore";
import { parseTitle } from "../state/prStore";
import { fileDigest, parseDiffCached, planTooltip, type DiffFile } from "./analysis/DiffView";

/** Focused sidebar for the selected PR: back to the list, then the changed
 *  files as a folder tree — like an IDE's project view of the review. */
export function PrFilesRail({
  pr,
  onBack,
  onOpenFile,
}: {
  pr: TrackedPr;
  onBack: () => void;
  onOpenFile: (path: string) => void;
}) {
  const entry = useDiffStore((s) => s.entries[pr.id]);
  const ensure = useDiffStore((s) => s.ensure);
  const setViewed = useDiffStore((s) => s.setViewed);
  useEffect(() => {
    void ensure(pr.id, pr.headSha);
  }, [pr.id, pr.headSha, ensure]);

  const [ignoreGlobs, setIgnoreGlobs] = useState<string[]>([]);
  useEffect(() => {
    void ipc.getSettings().then((s) => setIgnoreGlobs(s.reviewIgnoreGlobs));
  }, []);

  // Per-file significance from the L1 review plan, when one exists for this head.
  const l1Result = useAnalysisStore((s) => s.runs[analysisKey(pr.id, "context")]?.result);
  const planByPath = useMemo(
    () =>
      new Map(
        (l1Result?.headSha === pr.headSha ? l1Result.assessment.reviewPlan : []).map((p) => [
          p.path,
          p,
        ]),
      ),
    [l1Result, pr.headSha],
  );

  // Keyed on the raw string, not the entry object — a viewed-state toggle
  // replaces the entry but must not re-parse the whole diff.
  const raw = entry?.status === "done" ? entry.raw : null;
  const files = useMemo(() => (raw ? parseDiffCached(raw) : []), [raw]);
  const tree = useMemo(() => buildTree(files), [files]);
  const digests = useMemo(() => new Map(files.map((f) => [f.path, fileDigest(f)])), [files]);

  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  useEffect(() => {
    setCollapsed(new Set());
  }, [pr.id]);

  // The highlighted file follows the Diff tab's scroll position (and clicks).
  const visiblePath = useDiffStore((s) => s.visiblePath);
  const treeRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    treeRef.current
      ?.querySelector(".tree-row.active")
      ?.scrollIntoView({ block: "nearest" });
  }, [visiblePath]);

  const toggleDir = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const isViewed = (f: DiffFile) => entry?.viewed[f.path] === digests.get(f.path);
  // Ignore-glob files (lockfiles, generated) don't count toward review progress.
  const significant = files.filter((f) => !matchesAny(f.path, ignoreGlobs));
  const viewedCount = significant.filter(isViewed).length;

  const renderDir = (dir: TreeDir, depth: number): ReactElement[] => {
    const rows: ReactElement[] = [];
    const dirs = sortedDirs(dir);
    const dirFiles = sortedFiles(dir);
    for (const d of dirs) {
      const isCollapsed = collapsed.has(d.path);
      rows.push(
        <button
          key={`d:${d.path}`}
          className="tree-row tree-dir"
          style={{ paddingLeft: 8 + depth * 12 }}
          onClick={() => toggleDir(d.path)}
          aria-expanded={!isCollapsed}
        >
          <span className="chevron">{isCollapsed ? "▸" : "▾"}</span>
          <span className="tree-name">{d.name}</span>
          {isCollapsed && <span className="group-count">{countFiles(d)}</span>}
        </button>,
      );
      if (!isCollapsed) rows.push(...renderDir(d, depth + 1));
    }
    for (const f of dirFiles) {
      const plan = planByPath.get(f.path);
      const viewed = isViewed(f);
      const skipped = matchesAny(f.path, ignoreGlobs);
      rows.push(
        <button
          key={`f:${f.path}`}
          className={`tree-row tree-file${f.path === visiblePath ? " active" : ""}${viewed ? " viewed" : ""}${skipped ? " skipped" : ""}${f.deleted ? " deleted" : ""}`}
          style={{ paddingLeft: 8 + depth * 12 }}
          title={`${f.oldPath ? `${f.oldPath} → ` : ""}${f.path}${f.deleted ? " · deleted" : f.added ? " · new" : ""}${
            plan ? `\n\n${plan.significance.toUpperCase()} — ${planTooltip(plan) ?? ""}` : ""
          }`}
          onClick={() => onOpenFile(f.path)}
        >
          <span
            className={`tree-check${viewed ? " on" : ""}`}
            role="checkbox"
            aria-checked={viewed}
            title={viewed ? "Viewed — click to unmark" : "Mark as viewed"}
            onClick={(e) => {
              e.stopPropagation();
              const digest = digests.get(f.path);
              if (!digest) return;
              setViewed(pr.id, f.path, digest, !viewed);
              if (viewed) return;
              // Just marked viewed: point the diff at the next unviewed
              // file (tree order, wrapping), skipping ignore-glob files.
              const order = treeFileOrder(files).filter((p) => !matchesAny(p, ignoreGlobs));
              const i = order.indexOf(f.path);
              const next = [...order.slice(i + 1), ...order.slice(0, Math.max(i, 0))].find(
                (p) => p !== f.path && entry?.viewed[p] !== digests.get(p),
              );
              if (next) useDiffStore.getState().requestFocusFile(next);
            }}
          >
            ✓
          </span>
          {plan && <span className={`sig-dot ${plan.significance}`} />}
          <span className="tree-name">{fileName(f.path)}</span>
          <span className="spacer" />
          <span className="diffstat mono">
            <span className="add">+{f.additions}</span> <span className="del">−{f.deletions}</span>
          </span>
        </button>,
      );
    }
    return rows;
  };

  return (
    <>
      <div className="files-rail-header">
        <button className="files-back" onClick={onBack} title="Back to the pull-request list">
          ‹ All pull requests
        </button>
        <span className="eyebrow">
          {pr.repo} · #{pr.number}
        </span>
        <div className="files-pr-title">{parseTitle(pr.title).clean}</div>
        {files.length > 0 && (
          <div className="eyebrow files-progress">
            {files.length} files ·{" "}
            <span className="add">+{files.reduce((n, f) => n + f.additions, 0)}</span>{" "}
            <span className="del">−{files.reduce((n, f) => n + f.deletions, 0)}</span> ·{" "}
            <span className={viewedCount === significant.length ? "add" : undefined}>
              {viewedCount}/{significant.length} viewed
            </span>
          </div>
        )}
      </div>
      <div className="file-tree" ref={treeRef}>
        {(!entry || entry.status === "loading") && (
          <div className="eyebrow tree-note">fetching files…</div>
        )}
        {entry?.status === "error" && <div className="tree-note tree-error">{entry.error}</div>}
        {entry?.status === "done" && files.length === 0 && (
          <div className="eyebrow tree-note">empty diff</div>
        )}
        {renderDir(tree, 0)}
      </div>
    </>
  );
}
