import { useEffect, useState } from "react";
import { tip } from "./Tooltip";
import type { AuditEntry } from "../bindings/AuditEntry";
import { auditPr, describeAudit } from "../lib/audit";
import { ipc } from "../lib/ipc";
import { timeAgo } from "../state/prStore";

/** Merges are permanent; close/reopen are reversed with the PR controls. */
const UNDOABLE = new Set(["muted", "unmuted", "untracked", "tracked", "pr-priority", "repo-priority"]);

/** Everything you did — to CORA's tracking state (undoable) and on GitHub
 *  through it. Each PR entry names its PR and opens it in the main window. */
export function HistoryDrawer({
  open,
  onClose,
  onOpenPr,
}: {
  open: boolean;
  onClose: () => void;
  /** Open a PR from an entry: by id when it's still tracked, else by
   *  repo and number so it can be fetched again. */
  onOpenPr: (pr: { id: string; repo: string; number: number }) => void;
}) {
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
          <button className="icon-btn" {...tip("Close")} onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          {error && <div className="settings-error">{error}</div>}
          {entries.length === 0 && <div className="drawer-empty">no actions recorded yet</div>}
          {entries.map((entry) => {
            const pr = auditPr(entry);
            return (
            <div key={entry.id} className={`audit-entry${entry.undone ? " undone" : ""}`}>
              <div className="audit-main">
                <span className="audit-action">
                  {describeAudit(entry)}
                  {pr && (
                    <button
                      className="audit-pr mono"
                      data-tip={pr.title ? `Open ${pr.title}` : "Open this PR"}
                      onClick={() => {
                        onOpenPr(pr);
                        onClose();
                      }}
                    >
                      {pr.ref}
                    </button>
                  )}
                </span>
                <span className="audit-subject mono">{pr ? pr.title : entry.subjectLabel}</span>
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
            );
          })}
        </div>
      </aside>
    </>
  );
}
