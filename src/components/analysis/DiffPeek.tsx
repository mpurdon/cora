import { useEffect, useMemo, useState } from "react";
import type { C4Node } from "../../bindings/C4Node";
import { ipc } from "../../lib/ipc";
import { DiffJump, parseDiff, type DiffFile } from "./DiffView";

/** Lowercase with separators stripped, so naming conventions stop mattering:
 *  "Document Viewer Panel", "document-viewer-panel" and
 *  DocumentViewerPanel.tsx all reduce to the same token run. */
function squash(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Best-effort mapping from a C4 node to the diff files it concerns.
 *  Node ids follow "code:<path>#<symbol>" / "component:<path>" conventions,
 *  but models drift — so fall back to name matching against paths and
 *  added lines. */
export function matchFiles(files: DiffFile[], node: C4Node): DiffFile[] {
  const idPath = node.id
    .replace(/^(code|component|container|system|ext):/, "")
    .split("#")[0]
    .trim()
    .toLowerCase();
  const name = node.name.replace(/\(\)$/, "").trim();
  const idSquash = squash(idPath);
  const nameSquash = squash(name);

  const byPath = files.filter((f) => {
    const p = f.path.toLowerCase();
    if (idPath.length > 2 && (p.includes(idPath) || idPath.includes(p))) return true;
    return idSquash.length > 4 && squash(f.path).includes(idSquash);
  });
  if (byPath.length > 0) return byPath;

  if (nameSquash.length > 4) {
    const byName = files.filter((f) => squash(f.path).includes(nameSquash));
    if (byName.length > 0) return byName;
  }

  // No file carries the whole name — take the file(s) whose path hits the
  // most of its words ("Documents BFF Router" → documents/router.ts), but
  // only when a majority land; one shared word proves nothing.
  const tokens = [...new Set(name.toLowerCase().split(/[^a-z0-9]+/).filter((t) => t.length >= 3))];
  if (tokens.length >= 2) {
    const need = Math.max(2, Math.ceil(tokens.length / 2));
    const hits = files.map((f) => {
      const p = squash(f.path);
      return tokens.filter((t) => p.includes(t)).length;
    });
    const best = Math.max(0, ...hits);
    if (best >= need) return files.filter((_, i) => hits[i] === best);
  }

  if (name.length > 2) {
    // Last resort: the name (or its identifier form) in changed lines —
    // catches `import { DocumentViewerPanel }` style references.
    const ident = name.replace(/\s+/g, "");
    const byLine = files.filter((f) =>
      f.lines.some(
        (l) =>
          l.kind !== "ctx" &&
          (l.text.includes(name) || (ident.length > 4 && l.text.includes(ident))),
      ),
    );
    if (byLine.length > 0) return byLine;
  }
  return [];
}

/** Where in the diff does a finding belong? Three passes, most precise
 *  first: its graph nodes → a path named in the text → identifier-looking
 *  tokens (camelCase/snake_case) matched against changed lines. Null only
 *  when nothing anchors it — a genuinely PR-level remark. */
export function resolveFindingFile(
  seed: string,
  nodes: C4Node[],
  files: DiffFile[],
): string | null {
  const byNode = nodes.flatMap((n) => matchFiles(files, n))[0]?.path;
  if (byNode) return byNode;

  const lower = seed.toLowerCase();
  const byPath =
    files.find((f) => lower.includes(f.path.toLowerCase()))?.path ??
    files.find((f) => {
      const base = (f.path.split("/").pop() ?? "").toLowerCase();
      return base.length > 4 && lower.includes(base);
    })?.path;
  if (byPath) return byPath;

  // Identifier-ish tokens only (camelCase or snake_case) — plain English
  // words would anchor everything to everything.
  const tokens = [
    ...[...seed.matchAll(/`([^`\n]+)`/g)].map((m) => m[1]),
    ...(seed.match(/[A-Za-z_][A-Za-z0-9_]{4,}/g) ?? []),
  ].filter((tok) => /[a-z][A-Z]|_/.test(tok));
  const unique = [...new Set(tokens)];
  if (unique.length === 0) return null;
  let best: { path: string; hits: number } | null = null;
  for (const f of files) {
    const changed = f.lines.filter((l) => l.kind === "add" || l.kind === "del");
    const hits = unique.filter((tok) => changed.some((l) => l.text.includes(tok))).length;
    if (hits > 0 && (!best || hits > best.hits)) best = { path: f.path, hits };
  }
  return best?.path ?? null;
}

/** Pure diff rendering — no comments, no composers. */
function BareFileDiff({ file }: { file: DiffFile }) {
  const [open, setOpen] = useState(true);
  return (
    <div className="diff-file">
      <button className="diff-file-header" onClick={() => setOpen((o) => !o)}>
        <span className="chevron">{open ? "▾" : "▸"}</span>
        <span className="diff-path mono">{file.path}</span>
        <span className="spacer" />
        <span className="diffstat mono">
          <span className="add">+{file.additions}</span> <span className="del">−{file.deletions}</span>
        </span>
      </button>
      {open && (
        <pre className="diff-body">
          <div className="diff-scroll-inner">
            {file.lines.map((l, i) =>
              l.kind === "hunk" ? (
                <DiffJump key={i} text={l.text} />
              ) : (
                <div key={i} className={`diff-line ${l.kind}`}>
                  <span className="code-lineno">{l.newLine ?? ""}</span>
                  <span className="diff-gutter">
                    {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                  </span>
                  {l.text}
                </div>
              ),
            )}
          </div>
        </pre>
      )}
    </div>
  );
}

/** Slide-out showing just the diff behind a clicked canvas node. */
export function DiffPeek({
  prId,
  node,
  onClose,
}: {
  prId: string;
  node: C4Node;
  onClose: () => void;
}) {
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setRaw(null);
    void ipc
      .getPrDiff(prId)
      .then(setRaw)
      .catch((e) => setError(String(e)));
  }, [prId]);

  const files = useMemo(() => (raw ? parseDiff(raw) : []), [raw]);
  const matched = useMemo(() => matchFiles(files, node), [files, node]);

  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <aside className="activity-drawer code-drawer open">
        <header className="drawer-header">
          <span className="drawer-title mono code-drawer-path">{node.name} — diff</span>
          <button className="icon-btn" data-tip="Close" aria-label="Close" onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body code-drawer-body diff-peek-body">
          {error && <div className="analysis-error">{error}</div>}
          {raw === null && !error && <div className="drawer-empty">fetching diff…</div>}
          {raw !== null && matched.length === 0 && (
            <div className="drawer-empty">
              couldn't map "{node.name}" to a changed file — it may be unchanged in this PR
            </div>
          )}
          {matched.map((f) => (
            <BareFileDiff key={f.path} file={f} />
          ))}
        </div>
      </aside>
    </>
  );
}
