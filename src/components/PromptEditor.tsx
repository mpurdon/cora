import { useEffect, useState } from "react";
import type { Settings } from "../bindings/Settings";
import { ipc } from "../lib/ipc";

/** The analysis system prompt: the built-in text, or a custom one saved
 *  over it. Lives in the AI pane, next to the other levers an experiment
 *  varies. */
export function PromptEditor({
  settings,
  save,
}: {
  settings: Settings;
  save: (p: Partial<Settings>) => Promise<void>;
}) {
  const [defaultPrompt, setDefaultPrompt] = useState("");
  const [draft, setDraft] = useState<string | null>(null);

  useEffect(() => {
    void ipc.getDefaultSystemPrompt().then(setDefaultPrompt);
  }, []);

  const usingCustom = settings.customSystemPrompt.trim().length > 0;
  const value = draft ?? (usingCustom ? settings.customSystemPrompt : defaultPrompt);
  const dirty = draft !== null && draft !== (usingCustom ? settings.customSystemPrompt : "");

  return (
    <div>
      <p className="pane-intro">
        The system prompt sent to Bedrock for every analysis. Edits apply to the next run —
        cached analyses aren't re-run. {usingCustom ? (
          <strong>Currently using a custom prompt.</strong>
        ) : (
          "Currently using the built-in prompt."
        )}
      </p>
      <textarea
        className="prompt-editor"
        spellCheck={false}
        value={value}
        onChange={(e) => setDraft(e.target.value)}
      />
      <div className="row" style={{ marginTop: 10 }}>
        <button
          className="action-btn"
          disabled={!dirty}
          onClick={() => {
            void save({ customSystemPrompt: draft ?? "" });
            setDraft(null);
          }}
        >
          Save as custom prompt
        </button>
        <button
          className="action-btn"
          disabled={!usingCustom && draft === null}
          onClick={() => {
            void save({ customSystemPrompt: "" });
            setDraft(null);
          }}
        >
          Reset to built-in
        </button>
      </div>
    </div>
  );
}
