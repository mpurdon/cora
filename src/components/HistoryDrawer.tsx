import { useEffect, useState } from "react";
import type { AuditEntry } from "../bindings/AuditEntry";
import { ipc } from "../lib/ipc";
import { timeAgo } from "../state/prStore";

function describe(entry: AuditEntry): string {
  switch (entry.action) {
    case "muted":
      return "Muted";
    case "unmuted":
      return "Unmuted";
    case "untracked":
      return "Untracked";
    case "tracked":
      return "Tracked";
    case "pr-priority":
      return `PR priority ${entry.oldValue} → ${entry.newValue}`;
    case "repo-priority":
      return `Repo priority ${entry.oldValue} → ${entry.newValue}`;
    case "merged":
      return `Merged (${entry.newValue.replace(/^merged \(|\)$/g, "")})`;
    case "closed":
      return "Closed";
    case "reopened":
      return "Reopened";
    default:
      return entry.action;
  }
}

/** Merges are permanent; close/reopen are reversed with the PR controls. */
const UNDOABLE = new Set(["muted", "unmuted", "untracked", "tracked", "pr-priority", "repo-priority"]);

/** Everything you did to CORA's tracking state, undoable. */
export function HistoryDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = () => void ipc.getAuditLog().then(setEntries).catch((e) => setError(String(e)));

  useEffect(() => {
    if (open) load();
  }, [open]);

  const undo = async (id: number) => {
    try {
      await ipc.undoAudit(id);
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <>
      {open && <div className="drawer-backdrop" onClick={onClose} />}
      <aside className={`activity-drawer${open ? " open" : ""}`} aria-hidden={!open}>
        <header className="drawer-header">
          <span className="drawer-title">History</span>
          <button className="icon-btn" title="Close" onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          {error && <div className="settings-error">{error}</div>}
          {entries.length === 0 && <div className="drawer-empty">no actions recorded yet</div>}
          {entries.map((entry) => (
            <div key={entry.id} className={`audit-entry${entry.undone ? " undone" : ""}`}>
              <div className="audit-main">
                <span className="audit-action">{describe(entry)}</span>
                <span className="audit-subject mono">{entry.subjectLabel}</span>
                <span className="audit-when mono">{timeAgo(entry.at)} ago</span>
              </div>
              {entry.undone ? (
                <span className="thread-tag">undone</span>
              ) : UNDOABLE.has(entry.action) ? (
                <button className="thread-reply-btn" onClick={() => void undo(entry.id)}>
                  Undo
                </button>
              ) : null}
            </div>
          ))}
        </div>
      </aside>
    </>
  );
}
