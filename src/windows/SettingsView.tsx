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
        {Object.keys(settings.repoPriorities).length > 0 && (
          <div className="field">
            <label>Repository priorities</label>
            {Object.entries(settings.repoPriorities)
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([repo, prio]) => (
                <div key={repo} className="row prio-row">
                  <span className="mono prio-repo">{repo}</span>
                  <select
                    value={prio}
                    onChange={(e) => {
                      const next = { ...settings.repoPriorities };
                      if (e.target.value === "normal") delete next[repo];
                      else next[repo] = e.target.value as typeof prio;
                      void saveSettings({ repoPriorities: next });
                    }}
                  >
                    <option value="high">high</option>
                    <option value="normal">normal</option>
                    <option value="low">low</option>
                    <option value="ignored">ignored</option>
                  </select>
                </div>
              ))}
            <div className="field-hint">
              Flag repos from the PR list with ⚑ (group by repo). Ignored repos are never
              tracked; setting one back to normal removes it from this list.
            </div>
          </div>
        )}
      </section>

      <section>
        <span className="eyebrow">AWS</span>
        <div className="field">
          <label htmlFor="profile">AWS profile</label>
          <input
            id="profile"
            value={settings.awsProfile}
            onChange={(e) => setSettings({ ...settings, awsProfile: e.target.value })}
            onBlur={() => void saveSettings({ awsProfile: settings.awsProfile })}
          />
          <div className="field-hint">
            A named profile from ~/.aws/config. If analysis fails with an auth error, refresh
            with: aws sso login --profile {settings.awsProfile || "…"}
          </div>
        </div>
        <div className="field">
          <label htmlFor="region">Region</label>
          <input
            id="region"
            placeholder="us-east-2"
            value={settings.awsRegion}
            onChange={(e) => setSettings({ ...settings, awsRegion: e.target.value })}
            onBlur={() => void saveSettings({ awsRegion: settings.awsRegion })}
          />
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
          <label htmlFor="model">Bedrock model id or inference-profile ARN</label>
          <input
            id="model"
            value={settings.bedrockModelId}
            onChange={(e) => setSettings({ ...settings, bedrockModelId: e.target.value })}
            onBlur={() => void saveSettings({ bedrockModelId: settings.bedrockModelId })}
          />
          <div className="field-hint">
            Accepts a model id or an application-inference-profile ARN.
          </div>
        </div>
      </section>

      <section>
        <span className="eyebrow">CORA</span>
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
          <div className="field-hint">How often CORA checks GitHub for changes.</div>
        </div>
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

