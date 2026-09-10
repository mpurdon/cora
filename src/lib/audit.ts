import type { AuditEntry } from "../bindings/AuditEntry";

/** What you did, in a few words — shared by the History drawer and a PR's
 *  History tab so an action reads the same wherever it shows. */
export function describeAudit(entry: AuditEntry): string {
  const via = entry.newValue.includes("via assistant") ? " via the assistant" : "";
  switch (entry.action) {
    case "muted":
      return "Muted";
    case "unmuted":
      return "Unmuted";
    case "untracked":
      return "Untracked";
    case "tracked":
      return "Tracked";
    case "pr-priority":
      return `PR priority ${entry.oldValue} → ${entry.newValue}`;
    case "repo-priority":
      return `Repo priority ${entry.oldValue} → ${entry.newValue}`;
    case "author-priority":
      return `Author priority ${entry.oldValue} → ${entry.newValue}`;
    case "merged":
      return `Merged (${entry.newValue.replace(/^merged \(|\)$/g, "")})`;
    case "closed":
      return "Closed";
    case "reopened":
      return "Reopened";
    case "approved":
      return "Approved";
    case "changes-requested":
      return "Requested changes";
    case "review-commented":
      return "Reviewed with a comment";
    case "commented":
      return `Commented${via}`;
    case "diff-commented": {
      const where = entry.newValue.split(" · ")[0];
      return `Commented on ${where}${via}`;
    }
    case "replied":
      return `Replied to a thread${via}`;
    case "thread-resolved":
      return `Resolved a thread${via}`;
    case "thread-unresolved":
      return `Unresolved a thread${via}`;
    default:
      return entry.action;
  }
}

/** The PR an entry is about, when it's about one: "owner/repo#123" plus the
 *  title, cut from the label the backend wrote ("owner/repo#123 — title").
 *  Repo- and author-priority entries have no PR and return null. */
export function auditPr(
  entry: AuditEntry,
): { id: string; ref: string; repo: string; number: number; title: string } | null {
  const m = entry.subjectLabel.match(/^([^\s/]+\/[^\s#]+)#(\d+)(?: — (.*))?$/s);
  if (!m) return null;
  return {
    id: entry.subjectId,
    ref: `${m[1]}#${m[2]}`,
    repo: m[1],
    number: Number(m[2]),
    title: m[3] ?? "",
  };
}
