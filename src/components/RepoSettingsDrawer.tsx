import { useEffect, useState } from "react";
import { tip } from "./Tooltip";
import type { RepoPriority } from "../bindings/RepoPriority";
import type { Settings } from "../bindings/Settings";
import { resolveApproveMessage } from "../lib/comments";
import { ipc } from "../lib/ipc";
import { useRepoSettingsDrawerStore } from "../state/repoSettingsDrawerStore";
import { Field } from "../windows/SettingsView";

/** Per-repo overrides — priority, approve message, review instructions —
 *  opened from a repo row's gear action instead of living inline in the
 *  Repositories table. */
export function RepoSettingsDrawer() {
  const isOpen = useRepoSettingsDrawerStore((s) => s.isOpen);
  const repoFullName = useRepoSettingsDrawerStore((s) => s.repoFullName);
  const close = useRepoSettingsDrawerStore((s) => s.close);
  const [settings, setSettingsState] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => void ipc.getSettings().then(setSettingsState).catch((e) => setError(String(e)));

  useEffect(() => {
    if (isOpen) load();
  }, [isOpen, repoFullName]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close]);

  // Always re-reads the freshest settings before merging a patch, so a
  // field save here never clobbers a concurrent write — most importantly the
  // priority select's own audited ipc.setRepoPriority call just below.
  const patch = async (build: (fresh: Settings) => Settings) => {
    try {
      const fresh = await ipc.getSettings();
      const next = build(fresh);
      await ipc.setSettings(next);
      setSettingsState(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const setPriority = async (priority: RepoPriority) => {
    if (!repoFullName) return;
    try {
      await ipc.setRepoPriority(repoFullName, priority); // audited server-side
      const fresh = await ipc.getSettings();
      setSettingsState(fresh);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const setApproveMessage = (repo: string, value: string) =>
    void patch((fresh) => {
      const next = { ...fresh.repoApproveMessages };
      if (value.trim()) next[repo] = value;
      else delete next[repo];
      return { ...fresh, repoApproveMessages: next };
    });

  const setReviewInstructions = (repo: string, value: string) =>
    void patch((fresh) => {
      const next = { ...fresh.repoReviewInstructions };
      if (value.trim()) next[repo] = value;
      else delete next[repo];
      return { ...fresh, repoReviewInstructions: next };
    });

  const open = isOpen && !!repoFullName;

  return (
    <>
      {open && <div className="drawer-backdrop" onClick={close} />}
      <aside className={`activity-drawer${open ? " open" : ""}`} aria-hidden={!open}>
        <header className="drawer-header">
          <span className="drawer-title mono">{repoFullName ?? ""}</span>
          <button className="icon-btn" {...tip("Close")} onClick={close}>
            ✕
          </button>
        </header>
        <div className="drawer-body">
          {error && <div className="settings-error">{error}</div>}
          {settings && repoFullName && (
            <div key={repoFullName}>
              <Field
                label="Priority"
                hint="Weights this repo anywhere it appears — high floats to the top, low sinks, ignored is never tracked."
              >
                <select
                  value={settings.repoPriorities[repoFullName] ?? "normal"}
                  onChange={(e) => void setPriority(e.target.value as RepoPriority)}
                >
                  <option value="high">high</option>
                  <option value="normal">normal</option>
                  <option value="low">low</option>
                  <option value="ignored">ignored</option>
                </select>
              </Field>

              <Field
                label="Approve message"
                hint="Used to seed an empty approval on this repo. Falls back to the global default, then a plain sign-off."
              >
                <input
                  type="text"
                  defaultValue={settings.repoApproveMessages[repoFullName] ?? ""}
                  placeholder={resolveApproveMessage(settings, repoFullName)}
                  onBlur={(e) => setApproveMessage(repoFullName, e.target.value)}
                />
              </Field>

              <Field
                label="Review instructions"
                hint="Repo-specific guidance appended to the analysis and assistant prompts for this repo, alongside your team-wide review conventions."
              >
                <textarea
                  className="globs-editor"
                  spellCheck={false}
                  placeholder="e.g. This repo's public API is the /pkg directory — flag any breaking change there."
                  defaultValue={settings.repoReviewInstructions[repoFullName] ?? ""}
                  onBlur={(e) => setReviewInstructions(repoFullName, e.target.value)}
                />
              </Field>
            </div>
          )}
        </div>
      </aside>
    </>
  );
}
