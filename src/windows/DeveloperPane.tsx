import { useEffect, useRef, useState } from "react";
import type { LogEntry } from "../bindings/LogEntry";
import type { LogLevel } from "../bindings/LogLevel";
import type { Settings } from "../bindings/Settings";
import { ipc, onDevLog } from "../lib/ipc";
import { UsageView } from "../components/UsageView";

const LEVELS: LogLevel[] = ["debug", "info", "warn", "error"];
const LEVEL_RANK: Record<LogLevel, number> = { debug: 0, info: 1, warn: 2, error: 3 };

type DevTab = "logs" | "usage" | "prompt" | "internals";

export function DeveloperPane({
  settings,
  save,
}: {
  settings: Settings;
  save: (p: Partial<Settings>) => Promise<void>;
}) {
  const [tab, setTab] = useState<DevTab>("logs");
  return (
    <section className="pane-section pane-wide dev-pane">
      <h2>Developer</h2>
      <div className="tabs">
        {(
          [
            ["logs", "Logs"],
            ["usage", "Usage"],
            ["prompt", "System prompt"],
            ["internals", "Internals"],
          ] as [DevTab, string][]
        ).map(([key, label]) => (
          <button
            key={key}
            className={`tab${tab === key ? " active" : ""}`}
            onClick={() => setTab(key)}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === "logs" && <LogsView />}
      {tab === "usage" && <UsageView />}
      {tab === "prompt" && <PromptEditor settings={settings} save={save} />}
      {tab === "internals" && <InternalsView settings={settings} />}
    </section>
  );
}

// ------------------------------------------------------------------- logs

function LogsView() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [minLevel, setMinLevel] = useState<LogLevel>("debug");
  const [sourceFilter, setSourceFilter] = useState("all");
  const [follow, setFollow] = useState(true);
  const logRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void ipc.getDevLogs().then(setEntries);
    void onDevLog((entry) => setEntries((prev) => [...prev.slice(-1999), entry])).then(
      (fn) => (unlisten = fn),
    );
    return () => unlisten?.();
  }, []);

  // Scroll the log box itself. scrollIntoView() walks up to the nearest
  // scrollable ancestor — here the settings pane — and yanks the whole page
  // down, hiding the heading and tabs.
  useEffect(() => {
    if (follow && logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [entries, follow]);

  const sources = ["all", ...new Set(entries.map((e) => e.source))];
  const visible = entries.filter(
    (e) =>
      LEVEL_RANK[e.level] >= LEVEL_RANK[minLevel] &&
      (sourceFilter === "all" || e.source === sourceFilter),
  );

  return (
    <div>
      <div className="dev-log-controls">
        <select value={minLevel} onChange={(e) => setMinLevel(e.target.value as LogLevel)}>
          {LEVELS.map((l) => (
            <option key={l} value={l}>
              {l}+
            </option>
          ))}
        </select>
        <select value={sourceFilter} onChange={(e) => setSourceFilter(e.target.value)}>
          {sources.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <label className="check-row">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          follow
        </label>
        <span className="spacer" />
        <button
          className="action-btn"
          onClick={() =>
            void navigator.clipboard.writeText(
              visible.map((e) => `${e.at} [${e.level}] ${e.source}: ${e.message}`).join("\n"),
            )
          }
        >
          Copy
        </button>
        <button
          className="action-btn"
          onClick={() => void ipc.clearDevLogs().then(() => setEntries([]))}
        >
          Clear
        </button>
      </div>
      <div className="dev-log" ref={logRef}>
        {visible.length === 0 && <div className="dev-log-empty">no entries yet</div>}
        {visible.map((e, i) => (
          <div key={i} className={`dev-log-line level-${e.level}`}>
            <span className="dev-log-time">{e.at.slice(11, 19)}</span>
            <span className={`dev-log-level level-${e.level}`}>{e.level}</span>
            <span className="dev-log-source">{e.source}</span>
            <span className="dev-log-msg">{e.message}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// ------------------------------------------------------------------- prompt

function PromptEditor({
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

// ------------------------------------------------------------------- internals

function InternalsView({ settings }: { settings: Settings }) {
  const [internals, setInternals] = useState<{
    dataDir: string;
    dbPath: string;
    version: string;
  } | null>(null);

  useEffect(() => {
    void ipc.getAppInternals().then(setInternals);
  }, []);

  return (
    <div className="internals">
      {internals && (
        <dl className="internals-facts">
          <dt>Version</dt>
          <dd className="mono">{internals.version}</dd>
          <dt>Data directory</dt>
          <dd className="mono">{internals.dataDir}</dd>
          <dt>Database</dt>
          <dd className="mono">{internals.dbPath}</dd>
        </dl>
      )}
      <span className="eyebrow">Effective settings</span>
      <pre className="auth-detail internals-json">{JSON.stringify(settings, null, 2)}</pre>
    </div>
  );
}
