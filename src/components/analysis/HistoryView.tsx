import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { AuditEntry } from "../../bindings/AuditEntry";
import type { PrCommit } from "../../bindings/PrCommit";
import { describeAudit } from "../../lib/audit";
import { ipc } from "../../lib/ipc";
import { ciStatusTone, timeAgo } from "../../state/prStore";

/** The PR's commits and your own actions on it, one timeline, newest
 *  first — so "I asked for changes, then they pushed, then I approved"
 *  reads in order instead of across two views. */
type Item =
  | { kind: "commit"; at: string; commit: PrCommit }
  | { kind: "action"; at: string; entry: AuditEntry };

export function HistoryView({ prId, headSha }: { prId: string; headSha: string }) {
  const [commits, setCommits] = useState<PrCommit[] | null>(null);
  const [actions, setActions] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  const loadActions = () => void ipc.getPrAudit(prId).then(setActions).catch(() => {});

  useEffect(() => {
    setCommits(null);
    setActions([]);
    setError(null);
    void ipc
      .getPrCommits(prId)
      .then(setCommits)
      .catch((e) => setError(String(e)));
    loadActions();
    // Anything you do from another tab — a comment, a review — lands here
    // without a reopen.
    const unlisten = listen("reviews:changed", loadActions);
    return () => void unlisten.then((fn) => fn());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prId, headSha]);

  if (error) {
    return (
      <div className="placeholder">
        <p className="analysis-error">{error}</p>
      </div>
    );
  }
  if (commits == null) {
    return <div className="canvas-loading">fetching commits…</div>;
  }

  const items: Item[] = [
    ...commits.map((c): Item => ({ kind: "commit", at: c.at, commit: c })),
    ...actions.filter((a) => !a.undone).map((a): Item => ({ kind: "action", at: a.at, entry: a })),
  ].sort((a, b) => Date.parse(b.at) - Date.parse(a.at));

  if (items.length === 0) {
    return <div className="placeholder">No commits.</div>;
  }
  const live = actions.filter((a) => !a.undone).length;

  return (
    <div className="history-view">
      <span className="eyebrow">
        {commits.length} commit{commits.length === 1 ? "" : "s"}
        {live > 0 && ` · ${live} action${live === 1 ? "" : "s"} of yours`} — newest first
      </span>
      {items.map((item) => {
        if (item.kind === "action") {
          const a = item.entry;
          return (
            <div key={`a:${a.id}`} className="commit-row action-row">
              <span className="lamp you" role="img" aria-label="you" />
              <span className="commit-sha mono action-you">you</span>
              <span className="commit-msg">{describeAudit(a)}</span>
              <span />
              <span />
              <span className="commit-ago mono">{timeAgo(a.at)} ago</span>
            </div>
          );
        }
        const c = item.commit;
        const ciLabel = c.ciStatus ? `checks: ${c.ciStatus.toLowerCase()}` : "no checks";
        return (
          <div key={c.sha} className="commit-row" data-tip={c.message}>
            <span
              className={`lamp ${ciStatusTone(c.ciStatus)}`}
              role="img"
              aria-label={ciLabel}
              data-tip={ciLabel}
            />
            <button
              className="commit-sha mono"
              data-tip="Open commit on GitHub"
              onClick={() => void openUrl(c.url)}
            >
              {c.shortSha}
            </button>
            <span className="commit-msg">{c.message}</span>
            <span className="commit-author mono">@{c.author}</span>
            <span className="commit-stat mono">
              <span className="add">+{c.additions}</span> <span className="del">−{c.deletions}</span>
            </span>
            <span className="commit-ago mono">{timeAgo(c.at)} ago</span>
          </div>
        );
      })}
    </div>
  );
}
