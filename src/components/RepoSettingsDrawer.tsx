import { useEffect, useState } from "react";
import { tip } from "./Tooltip";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";
<<<<<<< HEAD
import { REPO_PRIORITY_LABEL, REPO_PRIORITY_ORDER } from "../lib/priority";
=======

const PRIORITIES: RepoPriority[] = [
  "ignored",
  "unimportant",
  "someday",
  "standard",
  "important",
  "critical",
];
>>>>>>> ghost/ghost-70bcc6

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n)}…` : s;
}

/** Per-repo overrides: priority, approve-message seed, and review
 *  instructions. Opened from the Repositories settings pane and from the
 *  repo group's context menu in the PR list — both share this one drawer. */
export function RepoSettingsDrawer({
  repo,
  open,
  onClose,
  settings,
  onSaveSettings,
  priority,
  onSetPriority,
}: {
  repo: string;
  open: boolean;
  onClose: () => void;
  settings: Settings | null;
  onSaveSettings: (patch: Partial<Settings>) => Promise<void>;
  priority: RepoPriority;
  onSetPriority: (p: RepoPriority) => void;
}) {
  const [approveDraft, setApproveDraft] = useState("");
  const [instructionsDraft, setInstructionsDraft] = useState("");

  // Depend on whether settings has loaded, not its identity: MainApp fetches
  // settings asynchronously after the drawer opens (null → loaded), so this
  // must still fire then — but it must NOT refire on every later settings
  // update (e.g. a priority change elsewhere), or it would clobber text the
  // reviewer is mid-typing here with the last-saved value.
  useEffect(() => {
    if (!open || !settings) return;
    setApproveDraft(settings.repoApproveMessages[repo] ?? "");
    setInstructionsDraft(settings.repoReviewInstructions[repo] ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, repo, !!settings]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const saveApprove = (value: string) => {
    if (!settings) return;
    const trimmed = value.trim();
    setApproveDraft(trimmed);
    const next = { ...settings.repoApproveMessages };
    if (trimmed) next[repo] = trimmed;
    else delete next[repo];
    void onSaveSettings({ repoApproveMessages: next });
  };

  const saveInstructions = (value: string) => {
    if (!settings) return;
    const trimmed = value.trim();
    setInstructionsDraft(trimmed);
    const next = { ...settings.repoReviewInstructions };
    if (trimmed) next[repo] = trimmed;
    else delete next[repo];
    void onSaveSettings({ repoReviewInstructions: next });
  };

  const globalApprove = settings?.defaultApproveMessage.trim() ?? "";
  const approvePlaceholder = globalApprove
    ? `Using global default: "${truncate(globalApprove, 80)}"`
    : "Using dynamic summary — a one-sentence recap of your review comments";

  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <aside className="activity-drawer open" aria-hidden={!open}>
        <header className="drawer-header">
          <span className="drawer-title mono">{repo}</span>
          <button className="icon-btn" {...tip("Close")} onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          {!settings ? (
            <div className="drawer-empty">loading…</div>
          ) : (
            <>
              <div className="field">
                <label>Priority</label>
                <select
                  value={priority}
                  onChange={(e) => onSetPriority(e.target.value as RepoPriority)}
                >
                  {REPO_PRIORITY_ORDER.map((p) => (
                    <option key={p} value={p}>
                      {REPO_PRIORITY_LABEL[p]}
                    </option>
                  ))}
                </select>
                <div className="field-hint">
                  Weights this repo everywhere it appears.
                </div>
              </div>

              <div className="field">
                <label>Approve message override</label>
                <textarea
                  className="globs-editor"
                  spellCheck={false}
                  placeholder={approvePlaceholder}
                  value={approveDraft}
                  onChange={(e) => setApproveDraft(e.target.value)}
                  onBlur={(e) => saveApprove(e.target.value)}
                />
                <div className="field-hint">
                  Seeds the approve composer for this repo's PRs. Leave empty to fall back to
                  the global default above.
                </div>
              </div>

              <div className="field">
                <label>Review instructions</label>
                <textarea
                  className="globs-editor"
                  spellCheck={false}
                  placeholder="Repo-specific knowledge no diff reveals…"
                  value={instructionsDraft}
                  onChange={(e) => setInstructionsDraft(e.target.value)}
                  onBlur={(e) => saveInstructions(e.target.value)}
                />
                <div className="field-hint">
                  Appended after your team review conventions for this repo's analysis and chat
                  prompts.
                </div>
              </div>
            </>
          )}
        </div>
      </aside>
    </>
  );
}
