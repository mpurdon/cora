import { useEffect, useMemo, useState } from "react";
import type { C4Node } from "../../bindings/C4Node";
import { ipc } from "../../lib/ipc";
import { parseDiff, type DiffFile } from "./DiffView";

/** Best-effort mapping from a C4 node to the diff files it concerns.
 *  Node ids follow "code:<path>#<symbol>" / "component:<path>" conventions,
 *  but models drift — so fall back to name matching against paths and
 *  added lines. */
function matchFiles(files: DiffFile[], node: C4Node): DiffFile[] {
  const idPath = node.id
    .replace(/^(code|component|container|system|ext):/, "")
    .split("#")[0]
    .trim()
    .toLowerCase();
  const name = node.name.replace(/\(\)$/, "").trim();

  const byPath = files.filter((f) => {
    const p = f.path.toLowerCase();
    return idPath.length > 2 && (p.includes(idPath) || idPath.includes(p));
  });
  if (byPath.length > 0) return byPath;

  if (name.length > 2) {
    const byName = files.filter(
      (f) =>
        f.path.toLowerCase().includes(name.toLowerCase()) ||
        f.lines.some((l) => l.kind !== "ctx" && l.text.includes(name)),
    );
    if (byName.length > 0) return byName;
  }
  return [];
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
          {file.lines.map((l, i) => (
            <div key={i} className={`diff-line ${l.kind}`}>
              <span className="diff-gutter">
                {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
              </span>
              {l.text}
            </div>
          ))}
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
          <button className="icon-btn" title="Close" onClick={onClose}>
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
