import { useEffect, useState, type ReactNode } from "react";
import { tip } from "./Tooltip";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";

/** Switch-style on/off control — mirrors the one in SettingsView. */
function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="toggle-row">
      <input
        type="checkbox"
        disabled={disabled}
        className="toggle-input"
        role="switch"
        aria-checked={checked}
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="toggle-track">
        <span className="toggle-knob" />
      </span>
      {label}
    </label>
  );
}

export interface RepoSettingsDrawerProps {
  repoKey: string | null;
  settings: Settings;
  onSave: (updated: Settings) => void;
  onClose: () => void;
}

export function RepoSettingsDrawer({
  repoKey,
  settings,
  onSave,
  onClose,
}: RepoSettingsDrawerProps) {
  const [watched, setWatched] = useState(false);
  const [priority, setPriority] = useState<RepoPriority>("normal");
  const [approveMessage, setApproveMessage] = useState("");
  const [reviewInstructions, setReviewInstructions] = useState("");

  // Re-initialise local state whenever the selected repo changes.
  useEffect(() => {
    if (repoKey === null) return;
    setWatched(settings.watchedRepos.includes(repoKey));
    setPriority(settings.repoPriorities[repoKey] ?? "normal");
    setApproveMessage(settings.repoApproveMessages?.[repoKey] ?? "");
    setReviewInstructions(settings.repoReviewInstructions?.[repoKey] ?? "");
  }, [repoKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const handleSave = () => {
    if (repoKey === null) return;

    // watchedRepos
    const watchedRepos = watched
      ? [...new Set([...settings.watchedRepos, repoKey])].sort()
      : settings.watchedRepos.filter((r) => r !== repoKey);

    // repoPriorities — omit 'normal' (absent = normal)
    const repoPriorities = { ...settings.repoPriorities };
    if (priority === "normal") {
      delete repoPriorities[repoKey];
    } else {
      repoPriorities[repoKey] = priority;
    }

    // repoApproveMessages
    const repoApproveMessages = { ...(settings.repoApproveMessages ?? {}) };
    const trimmedMessage = approveMessage.trim();
    if (trimmedMessage) {
      repoApproveMessages[repoKey] = trimmedMessage;
    } else {
      delete repoApproveMessages[repoKey];
    }

    // repoReviewInstructions
    const repoReviewInstructions = { ...(settings.repoReviewInstructions ?? {}) };
    const trimmedInstructions = reviewInstructions.trim();
    if (trimmedInstructions) {
      repoReviewInstructions[repoKey] = trimmedInstructions;
    } else {
      delete repoReviewInstructions[repoKey];
    }

    onSave({
      ...settings,
      watchedRepos,
      repoPriorities,
      repoApproveMessages,
      repoReviewInstructions,
    });
    onClose();
  };

  return (
    <>
      {repoKey !== null && (
        <div className="drawer-backdrop" onClick={onClose} />
      )}
      <aside
        className={"activity-drawer" + (repoKey ? " open" : "")}
        aria-hidden={repoKey === null}
      >
        <header className="drawer-header">
          <span className="drawer-title mono">{repoKey ?? ""}</span>
          <button className="icon-btn" {...tip("Close")} onClick={onClose}>
            ✕
          </button>
        </header>

        <div className="drawer-body">
          <div className="field">
            <label>Watch all PRs</label>
            <Toggle
              checked={watched}
              onChange={setWatched}
              label="Track every open PR in this repository"
            />
            <div className="field-hint">
              When enabled, CORA tracks all open PRs — not just those involving you.
            </div>
          </div>

          <div className="field">
            <label>Priority</label>
            <select
              value={priority}
              onChange={(e) => setPriority(e.target.value as RepoPriority)}
            >
              <option value="high">high</option>
              <option value="normal">normal</option>
              <option value="low">low</option>
              <option value="ignored">ignored</option>
            </select>
            <div className="field-hint">
              High floats this repo to the top everywhere; ignored means it is never tracked.
            </div>
          </div>

          <div className="field">
            <label>Approve message override</label>
            <input
              placeholder="Leave blank to use the default approve message"
              value={approveMessage}
              onChange={(e) => setApproveMessage(e.target.value)}
            />
            <div className="field-hint">
              Overrides the global approve message for this repository only.
            </div>
          </div>

          <div className="field">
            <label>Review instructions</label>
            <textarea
              className="globs-editor"
              spellCheck={false}
              placeholder="Repo-specific context for AI reviews…"
              value={reviewInstructions}
              onChange={(e) => setReviewInstructions(e.target.value)}
            />
            <div className="field-hint">
              Appended to AI review prompts for this repo only.
            </div>
          </div>

          <div className="row">
            <button className="action-btn" onClick={handleSave}>
              Save
            </button>
            <button className="action-btn" onClick={onClose}>
              Cancel
            </button>
          </div>
        </div>
      </aside>
    </>
  );
}
