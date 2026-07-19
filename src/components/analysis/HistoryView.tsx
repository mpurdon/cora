import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { PrCommit } from "../../bindings/PrCommit";
import { ipc } from "../../lib/ipc";
import { ciStatusTone, timeAgo } from "../../state/prStore";

/** Commit history for the PR, oldest first — like GitHub's Commits tab. */
export function HistoryView({ prId, headSha }: { prId: string; headSha: string }) {
  const [commits, setCommits] = useState<PrCommit[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setCommits(null);
    setError(null);
    void ipc
      .getPrCommits(prId)
      .then(setCommits)
      .catch((e) => setError(String(e)));
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
  if (commits.length === 0) {
    return <div className="placeholder">No commits.</div>;
  }

  return (
    <div className="history-view">
      <span className="eyebrow">
        {commits.length} commit{commits.length === 1 ? "" : "s"} — oldest first
      </span>
      {commits.map((c) => {
        const ciLabel = c.ciStatus ? `checks: ${c.ciStatus.toLowerCase()}` : "no checks";
        return (
        <div key={c.sha} className="commit-row" title={c.message}>
          <span
            className={`lamp ${ciStatusTone(c.ciStatus)}`}
            role="img"
            aria-label={ciLabel}
            title={ciLabel}
          />
          <button
            className="commit-sha mono"
            title="Open commit on GitHub"
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
