import { useEffect, useState } from "react";
import { tip } from "./Tooltip";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";
import { ipc } from "../lib/ipc";

type ApproveMap = "repoApproveMessages" | "repoReviewInstructions";

/** Per-repo overrides: priority (audited, applies immediately), the approve
 *  message seed, and review instructions folded into the analysis/chat
 *  prompts. Mirrors HistoryDrawer/ContextDrawer's slide-out chrome. */
export function RepoSettingsDrawer({
  repo,
  open,
  settings,
  priority,
  onClose,
  onSettingsSaved,
  onPriorityChanged,
}: {
  repo: string | null;
  open: boolean;
  settings: Settings | null;
  priority: RepoPriority;
  onClose: () => void;
  onSettingsSaved: (patch: Partial<Settings>) => void;
  onPriorityChanged: (p: RepoPriority) => void;
}) {
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setError(null), [repo]);

  const setPriority = async (p: RepoPriority) => {
    if (!repo) return;
    try {
      await ipc.setRepoPriority(repo, p); // audited server-side
      onPriorityChanged(p);
    } catch (e) {
      setError(String(e));
    }
  };

  // Never persists an empty-string value — an empty field means "no
  // override", so its key is removed from the map instead.
  const saveRepoMapField = async (map: ApproveMap, value: string) => {
    if (!repo || !settings) return;
    const next = { ...settings[map] };
    const trimmed = value.trim();
    if (trimmed) next[repo] = trimmed;
    else delete next[repo];
    try {
      await ipc.setSettings({ ...settings, [map]: next });
      setError(null);
      onSettingsSaved({ [map]: next });
    } catch (e) {
      setError(String(e));
    }
  };

  const approvePlaceholder =
    settings?.defaultApproveMessage?.trim() || "Approving — nothing blocking from me.";

  return (
    <>
      {open && <div className="drawer-backdrop" onClick={onClose} />}
      <aside className={`activity-drawer${open ? " open" : ""}`} aria-hidden={!open}>
        <header className="drawer-header">
          <span className="drawer-title mono">{repo ?? "Repo settings"}</span>
          <button className="icon-btn" {...tip("Close")} onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          {error && <div className="settings-error">{error}</div>}
          {!repo ? (
            <div className="drawer-empty">no repo selected</div>
          ) : (
            <>
              <div className="field">
                <label>Priority</label>
                <select
                  value={priority}
                  onChange={(e) => void setPriority(e.target.value as RepoPriority)}
                >
                  <option value="high">high</option>
                  <option value="normal">normal</option>
                  <option value="low">low</option>
                  <option value="ignored">ignored</option>
                </select>
                <div className="field-hint">
                  Weights this repo everywhere it appears — high floats to the top, low sinks,
                  ignored is never tracked at all.
                </div>
              </div>

              <div className="field">
                <label>Approve message override</label>
                <textarea
                  key={`approve-${repo}`}
                  className="globs-editor"
                  spellCheck={false}
                  placeholder={approvePlaceholder}
                  defaultValue={settings?.repoApproveMessages[repo] ?? ""}
                  onBlur={(e) => void saveRepoMapField("repoApproveMessages", e.target.value)}
                />
                <div className="field-hint">
                  Seeds the approve-review composer for this repo. Empty falls back to the
                  global default approve message, shown above as the placeholder.
                </div>
              </div>

              <div className="field">
                <label>Repo review instructions</label>
                <textarea
                  key={`instructions-${repo}`}
                  className="globs-editor"
                  spellCheck={false}
                  placeholder="Team knowledge no diff reveals, specific to this repo…"
                  defaultValue={settings?.repoReviewInstructions[repo] ?? ""}
                  onBlur={(e) => void saveRepoMapField("repoReviewInstructions", e.target.value)}
                />
                <div className="field-hint">
                  Injected into the analysis, code pass, and assistant prompts for this repo,
                  alongside the global team review conventions.
                </div>
              </div>
            </>
          )}
        </div>
      </aside>
    </>
  );
}
