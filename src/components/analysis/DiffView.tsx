import { useEffect, useMemo, useState } from "react";
import { ipc } from "../../lib/ipc";

interface DiffFile {
  path: string;
  oldPath?: string;
  additions: number;
  deletions: number;
  lines: { kind: "add" | "del" | "ctx" | "hunk"; text: string }[];
}

function parseDiff(raw: string): DiffFile[] {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  for (const line of raw.split("\n")) {
    if (line.startsWith("diff --git ")) {
      if (current) files.push(current);
      // "diff --git a/x b/y" — y is the post-change path.
      const m = line.match(/^diff --git a\/(.*) b\/(.*)$/);
      current = {
        path: m?.[2] ?? line.slice(11),
        oldPath: m && m[1] !== m[2] ? m[1] : undefined,
        additions: 0,
        deletions: 0,
        lines: [],
      };
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
      current.lines.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      current.additions += 1;
      current.lines.push({ kind: "add", text: line.slice(1) });
    } else if (line.startsWith("-")) {
      current.deletions += 1;
      current.lines.push({ kind: "del", text: line.slice(1) });
    } else {
      current.lines.push({ kind: "ctx", text: line.startsWith(" ") ? line.slice(1) : line });
    }
  }
  if (current) files.push(current);
  return files;
}

function FileDiff({ file }: { file: DiffFile }) {
  // Large files start collapsed so huge PRs stay scrollable.
  const [open, setOpen] = useState(file.lines.length <= 400);
  return (
    <div className="diff-file">
      <button className="diff-file-header" onClick={() => setOpen((o) => !o)}>
        <span className="chevron">{open ? "▾" : "▸"}</span>
        <span className="diff-path mono">
          {file.oldPath ? `${file.oldPath} → ` : ""}
          {file.path}
        </span>
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

export function DiffView({ prId, headSha }: { prId: string; headSha: string }) {
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setRaw(null);
    setError(null);
    void ipc
      .getPrDiff(prId)
      .then(setRaw)
      .catch((e) => setError(String(e)));
  }, [prId, headSha]);

  const files = useMemo(() => (raw ? parseDiff(raw) : []), [raw]);

  if (error) {
    return <div className="placeholder analysis-error">{error}</div>;
  }
  if (raw === null) {
    return <div className="canvas-loading">fetching diff…</div>;
  }
  if (files.length === 0) {
    return <div className="placeholder">Empty diff.</div>;
  }
  return (
    <div className="diff-view">
      <div className="eyebrow diff-summary">
        {files.length} files · <span className="add">+{files.reduce((n, f) => n + f.additions, 0)}</span>{" "}
        <span className="del">−{files.reduce((n, f) => n + f.deletions, 0)}</span>
      </div>
      {files.map((f) => (
        <FileDiff key={f.path} file={f} />
      ))}
    </div>
  );
}
