import { useEffect, useRef, useState } from "react";
import { ipc } from "../../lib/ipc";

type Phase = "idle" | "signing-in" | "failed";

/** Coming back to the window is the signal that you went and fixed the
 *  session; retry then, but not on every alt-tab while it is still broken. */
const FOCUS_RETRY_GAP_MS = 20_000;

/**
 * Recovery card for AWS credential failures. One click runs `aws sso login`
 * and retries after the browser round-trip — but a profile whose credentials
 * come from somewhere else (a credential process, an SSO helper that writes
 * keys) is refreshed outside this window, so the card also retries on its
 * own when the window regains focus, and offers a plain Retry.
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

  // Refocus after a sign-in elsewhere → retry, at most once per gap. The
  // card unmounts while the retry runs and comes back fresh if it fails,
  // so a still-broken session costs one cheap call per return, not a loop.
  const lastAuto = useRef(0);
  useEffect(() => {
    const onFocus = () => {
      if (phase === "signing-in") return;
      const now = Date.now();
      if (now - lastAuto.current < FOCUS_RETRY_GAP_MS) return;
      lastAuto.current = now;
      onSignedIn();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [phase, onSignedIn]);

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
        profile — usually an expired session. Sign in here, or refresh the profile's credentials
        the way you normally do, and the analysis resumes when you come back to this window.
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
          <button className="action-btn" onClick={onSignedIn} data-tip="Try the analysis again now">
            Retry
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
