import { useEffect, useState } from "react";
import type { Settings } from "../bindings/Settings";
import { ipc } from "../lib/ipc";

export function SettingsView({ onClose }: { onClose: () => void }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [patPresent, setPatPresent] = useState(false);
  const [patDraft, setPatDraft] = useState("");
  const [reposDraft, setReposDraft] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      const s = await ipc.getSettings();
      setSettings(s);
      setReposDraft(s.watchedRepos.join("\n"));
      setPatPresent(await ipc.githubPatPresent());
    })();
  }, []);

  if (!settings) return null;

  const flash = (msg: string) => {
    setSaved(msg);
    setError(null);
    setTimeout(() => setSaved(null), 2500);
  };

  const savePat = async () => {
    try {
      await ipc.setGithubPat(patDraft);
      setPatDraft("");
      setPatPresent(true);
      flash("token stored in Keychain");
    } catch (e) {
      setError(String(e));
    }
  };

  const saveSettings = async (patch: Partial<Settings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    try {
      await ipc.setSettings(next);
      flash("saved");
    } catch (e) {
      setError(String(e));
    }
  };

  const saveRepos = () =>
    saveSettings({
      watchedRepos: reposDraft
        .split(/[\n,\s]+/)
        .map((r) => r.trim())
        .filter((r) => /^[\w.-]+\/[\w.-]+$/.test(r)),
    });

  return (
    <div className="settings">
      <h1>Settings</h1>

      <section>
        <span className="eyebrow">GitHub</span>
        <div className="field">
          <label htmlFor="pat">Personal access token</label>
          <div className="row">
            <input
              id="pat"
              type="password"
              placeholder={patPresent ? "••••••••  (stored in Keychain)" : "ghp_… or github_pat_…"}
              value={patDraft}
              onChange={(e) => setPatDraft(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && patDraft && void savePat()}
            />
            <button className="action-btn" disabled={!patDraft} onClick={() => void savePat()}>
              Save
            </button>
            {patPresent && <span className="pat-ok">✓ connected</span>}
          </div>
          <div className="field-hint">
            Needs repo read scope. The token is stored in the macOS Keychain and never leaves the
            Rust core.
          </div>
        </div>
        <div className="field">
          <label htmlFor="gql">GraphQL endpoint</label>
          <input
            id="gql"
            value={settings.githubGraphqlUrl}
            onChange={(e) => setSettings({ ...settings, githubGraphqlUrl: e.target.value })}
            onBlur={() => void saveSettings({ githubGraphqlUrl: settings.githubGraphqlUrl })}
          />
          <div className="field-hint">
            For GitHub Enterprise, use https://&lt;host&gt;/api/graphql.
          </div>
        </div>
        <div className="field">
          <label htmlFor="repos">Watched repositories (owner/name, one per line)</label>
          <input
            id="repos"
            placeholder="acme/api  acme/web"
            value={reposDraft}
            onChange={(e) => setReposDraft(e.target.value)}
            onBlur={() => void saveRepos()}
          />
        </div>
        <div className="field">
          <label htmlFor="interval">Poll interval (seconds)</label>
          <input
            id="interval"
            type="number"
            min={15}
            value={settings.pollIntervalSecs}
            onChange={(e) =>
              setSettings({ ...settings, pollIntervalSecs: Number(e.target.value) || 45 })
            }
            onBlur={() => void saveSettings({ pollIntervalSecs: settings.pollIntervalSecs })}
          />
        </div>
      </section>

      <section>
        <span className="eyebrow">AWS Bedrock (analysis)</span>
        <div className="field">
          <label htmlFor="profile">AWS profile</label>
          <input
            id="profile"
            value={settings.awsProfile}
            onChange={(e) => setSettings({ ...settings, awsProfile: e.target.value })}
            onBlur={() => void saveSettings({ awsProfile: settings.awsProfile })}
          />
          <div className="field-hint">A named profile from ~/.aws/config.</div>
        </div>
        <div className="field">
          <label htmlFor="endpoint">Custom endpoint URL (optional)</label>
          <input
            id="endpoint"
            placeholder="https://bedrock-runtime.…"
            value={settings.awsEndpointUrl}
            onChange={(e) => setSettings({ ...settings, awsEndpointUrl: e.target.value })}
            onBlur={() => void saveSettings({ awsEndpointUrl: settings.awsEndpointUrl })}
          />
        </div>
        <div className="field">
          <label htmlFor="model">Bedrock model id</label>
          <input
            id="model"
            value={settings.bedrockModelId}
            onChange={(e) => setSettings({ ...settings, bedrockModelId: e.target.value })}
            onBlur={() => void saveSettings({ bedrockModelId: settings.bedrockModelId })}
          />
        </div>
      </section>

      <section>
        <span className="eyebrow">Track a PR manually</span>
        <TrackByUrl onTracked={() => flash("PR tracked")} onError={setError} />
      </section>

      <div className="row">
        <button className="action-btn" onClick={onClose}>
          Done
        </button>
        {saved && <span className="save-note">{saved}</span>}
        {error && <span style={{ color: "var(--bad)", fontSize: 11 }}>{error}</span>}
      </div>
    </div>
  );
}

function TrackByUrl({
  onTracked,
  onError,
}: {
  onTracked: () => void;
  onError: (e: string) => void;
}) {
  const [url, setUrl] = useState("");
  const track = async () => {
    try {
      await ipc.trackPrUrl(url);
      setUrl("");
      onTracked();
    } catch (e) {
      onError(String(e));
    }
  };
  return (
    <div className="field">
      <div className="row">
        <input
          placeholder="https://github.com/owner/repo/pull/123"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && url && void track()}
        />
        <button className="action-btn" disabled={!url} onClick={() => void track()}>
          Track
        </button>
      </div>
    </div>
  );
}
