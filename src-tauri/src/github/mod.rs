pub mod poller;
pub mod query;

use serde_json::Value;

use crate::models::{Label, PrInfo};

/// Map one GraphQL `PullRequest` node to our DTO. Returns None for non-PR
/// nodes (the `nodes(ids:)` lookup can surface other types) or shape drift.
pub fn parse_pr(v: &Value) -> Option<PrInfo> {
    let id = v.get("id")?.as_str()?.to_string();
    let repo = v.pointer("/repository/nameWithOwner")?.as_str()?.to_string();
    let last_commit = v.pointer("/commits/nodes/0/commit");
    Some(PrInfo {
        id,
        number: v.get("number")?.as_i64()?,
        title: v.get("title")?.as_str()?.to_string(),
        url: v.get("url")?.as_str()?.to_string(),
        repo,
        author: v
            .pointer("/author/login")
            .and_then(Value::as_str)
            .unwrap_or("ghost")
            .to_string(),
        is_draft: v.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        state: v.get("state")?.as_str()?.to_string(),
        review_decision: v
            .get("reviewDecision")
            .and_then(Value::as_str)
            .map(String::from),
        ci_status: last_commit
            .and_then(|c| c.pointer("/statusCheckRollup/state"))
            .and_then(Value::as_str)
            .map(String::from),
        mergeable: v
            .get("mergeable")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_string(),
        additions: v.get("additions").and_then(Value::as_i64).unwrap_or(0),
        deletions: v.get("deletions").and_then(Value::as_i64).unwrap_or(0),
        changed_files: v.get("changedFiles").and_then(Value::as_i64).unwrap_or(0),
        total_comments: v.get("totalCommentsCount").and_then(Value::as_i64).unwrap_or(0),
        recent_comment_authors: v
            .pointer("/recentComments/nodes")
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|c| {
                        let login = c.pointer("/author/login").and_then(Value::as_str)?;
                        let is_bot = c.pointer("/author/__typename").and_then(Value::as_str)
                            == Some("Bot");
                        Some(if is_bot && !login.ends_with("[bot]") {
                            format!("{login}[bot]")
                        } else {
                            login.to_string()
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        head_sha: last_commit
            .and_then(|c| c.get("oid"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        updated_at: v
            .get("updatedAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        labels: v
            .pointer("/labels/nodes")
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|l| {
                        Some(Label {
                            name: l.get("name")?.as_str()?.to_string(),
                            color: l.get("color")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parse a GitHub PR URL into (owner/repo, number).
pub fn parse_pr_url(url: &str) -> Option<(String, i64)> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let mut parts = rest.split('/');
    let _host = parts.next()?;
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let number: i64 = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((format!("{owner}/{repo}"), number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pr_urls() {
        assert_eq!(
            parse_pr_url("https://github.com/acme/api/pull/123"),
            Some(("acme/api".into(), 123))
        );
        assert_eq!(
            parse_pr_url("https://github.com/acme/api/pull/123/files#diff-abc"),
            Some(("acme/api".into(), 123))
        );
        assert_eq!(parse_pr_url("https://github.com/acme/api/issues/5"), None);
        assert_eq!(parse_pr_url("not a url"), None);
    }
}
