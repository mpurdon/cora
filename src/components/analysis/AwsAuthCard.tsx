import { useEffect, useState } from "react";
import { ipc } from "../../lib/ipc";

type Phase = "idle" | "signing-in" | "failed";

/**
 * Recovery card for AWS credential failures: one click runs
 * `aws sso login`, waits for the browser round-trip, then retries.
 */
export function AwsAuthCard({
  detail,
  onSignedIn,
}: {
  detail: string;
  onSignedIn: () => void;
}) {
  const [profile, setProfile] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [failure, setFailure] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [showDetail, setShowDetail] = useState(false);

  useEffect(() => {
    void ipc.getSettings().then((s) => setProfile(s.awsProfile));
  }, []);

  const command = `aws sso login --profile ${profile || "<profile>"}`;

  const signIn = async () => {
    setPhase("signing-in");
    setFailure(null);
    try {
      await ipc.awsSsoLogin(profile);
      onSignedIn();
    } catch (e) {
      setPhase("failed");
      setFailure(String(e));
    }
  };

  const copy = async () => {
    await navigator.clipboard.writeText(command);
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  };

  return (
    <div className="auth-card">
      <div className="auth-title">
        <span className="lamp bad" />
        AWS session needed
      </div>
      <p className="auth-body">
        CORA couldn't get AWS credentials for the <span className="mono">{profile}</span>{" "}
        profile — usually an expired SSO session. Sign in and the analysis will resume
        automatically.
      </p>

      {phase === "signing-in" ? (
        <div className="auth-waiting">
          <span className="sync-dot live" />
          Waiting for the browser sign-in to finish…
        </div>
      ) : (
        <div className="auth-actions">
          <button className="action-btn auth-primary" onClick={() => void signIn()}>
            Sign in with AWS SSO
          </button>
          <button className="action-btn" onClick={() => void copy()}>
            {copied ? "Copied" : "Copy command"}
          </button>
        </div>
      )}

      {phase === "failed" && failure && (
        <p className="auth-failure">
          Sign-in didn't complete: {failure}
          <br />
          You can also run <span className="mono auth-cmd">{command}</span> in a terminal, then
          retry.
        </p>
      )}

      <button className="auth-detail-toggle" onClick={() => setShowDetail((s) => !s)}>
        {showDetail ? "hide" : "show"} technical detail
      </button>
      {showDetail && <pre className="auth-detail">{detail}</pre>}
    </div>
  );
}
