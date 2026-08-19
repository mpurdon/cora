import { useCallback, useEffect, useRef } from "react";
import { tip } from "./Tooltip";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";
import { HARDCODED_APPROVE_FALLBACK } from "../lib/comments";
import { useSettingsStore } from "../state/settingsStore";

const PRIORITY_OPTIONS: RepoPriority[] = ["high", "normal", "low", "ignored"];

/** Per-repo priority, approve-message override, and review instructions —
 *  the settings that don't fit the flag-cycle fast path in the repo rail.
 *  Priority still goes through the audited `setRepoPriority` the flag icon
 *  and context menu already use, so undo/audit history is unaffected;
 *  everything else goes through the shared settings store. */
export function RepoSettingsDrawer({
  open,
  repo,
  priority,
  onSetPriority,
  onClose,
}: {
  open: boolean;
  repo: string | null;
  priority: RepoPriority;
  onSetPriority: (repo: string, priority: RepoPriority) => void;
  onClose: () => void;
}) {
  const settings = useSettingsStore((s) => s.settings);
  const save = useSettingsStore((s) => s.save);

  const approveRef = useRef<HTMLTextAreaElement>(null);
  const instructionsRef = useRef<HTMLTextAreaElement>(null);

  // A single combined save, not one call per field — two concurrent
  // setSettings round-trips would race, and the slower one would overwrite
  // whichever field the faster one just wrote.
  const flush = useCallback(() => {
    if (!repo || !settings) return;
    const patch: Partial<Settings> = {};
    const approveVal = approveRef.current?.value ?? "";
    if (approveVal !== (settings.repoApproveMessages[repo] ?? "")) {
      patch.repoApproveMessages = { ...settings.repoApproveMessages, [repo]: approveVal };
    }
    const instructionsVal = instructionsRef.current?.value ?? "";
    if (instructionsVal !== (settings.repoReviewInstructions[repo] ?? "")) {
      patch.repoReviewInstructions = { ...settings.repoReviewInstructions, [repo]: instructionsVal };
    }
    if (Object.keys(patch).length > 0) void save(patch);
  }, [repo, settings, save]);

  const close = useCallback(() => {
    flush();
    onClose();
  }, [flush, onClose]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, close]);

  if (!repo) return null;

  const resetApproveMessage = () => {
    if (!settings) return;
    const next = { ...settings.repoApproveMessages };
    delete next[repo];
    void save({ repoApproveMessages: next });
    if (approveRef.current) approveRef.current.value = "";
  };

  const globalDefault = settings?.defaultApproveMessage?.trim();
  const inheritedApprove = globalDefault || HARDCODED_APPROVE_FALLBACK;

  return (
    <>
      {open && <div className="drawer-backdrop" onClick={close} />}
      <aside className={`activity-drawer repo-settings-drawer${open ? " open" : ""}`} aria-hidden={!open}>
        <header className="drawer-header">
          <span className="drawer-title mono">{repo}</span>
          <button className="icon-btn" {...tip("Close")} onClick={close}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          <div className="field">
            <label>Priority</label>
            <select
              value={priority}
              onChange={(e) => onSetPriority(repo, e.target.value as RepoPriority)}
            >
              {PRIORITY_OPTIONS.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
            <div className="field-hint">
              Weights this repo anywhere it appears — high floats to the top, low sinks, ignored
              is never tracked.
            </div>
          </div>

          <div className="field">
            <label>Approve message override</label>
            <textarea
              key={`approve-${repo}`}
              ref={approveRef}
              className="globs-editor"
              spellCheck={false}
              placeholder={inheritedApprove}
              defaultValue={settings?.repoApproveMessages[repo] ?? ""}
              onBlur={flush}
            />
            <div className="field-hint">
              Seeds this repo's approve box instead of{" "}
              {globalDefault ? "the global default" : "the built-in default"} above. Empty uses
              that inherited text (shown as the placeholder).
            </div>
            <div className="row repo-settings-reset">
              <button className="thread-reply-btn" onClick={resetApproveMessage}>
                Reset to default
              </button>
            </div>
          </div>

          <div className="field">
            <label>Review instructions</label>
            <textarea
              key={`instructions-${repo}`}
              ref={instructionsRef}
              className="globs-editor"
              spellCheck={false}
              placeholder="Extra guidance for this repo only — appended to the analysis and assistant prompts…"
              defaultValue={settings?.repoReviewInstructions[repo] ?? ""}
              onBlur={flush}
            />
          </div>
        </div>
      </aside>
    </>
  );
}
