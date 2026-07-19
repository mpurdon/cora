use std::collections::HashMap;

use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

pub const PR_FRAGMENT: &str = "
fragment PrFields on PullRequest {
  id number title url isDraft state updatedAt
  repository { nameWithOwner }
  author { login }
  reviewDecision
  mergeable
  additions deletions changedFiles
  totalCommentsCount
  recentComments: comments(last: 5) { nodes { id body author { login __typename } } }
  labels(first: 10) { nodes { name color } }
  commits(last: 1) { nodes { commit { oid statusCheckRollup { state } } } }
}
";

/// The scopes we poll, in priority order. Each becomes an aliased `search`.
pub struct PollRequest {
    pub watched_repos: Vec<String>,
    pub tracked_ids: Vec<String>,
    /// YYYY-MM-DD; when set, every search scope gains `updated:>=` so PRs
    /// idle beyond the settings window never enter discovery.
    pub updated_since: Option<String>,
}

impl PollRequest {
    /// Build one batched GraphQL document + variables. Aliases for empty
    /// scopes are omitted entirely (GraphQL requires every variable be used).
    pub fn build(&self) -> (String, Value) {
        let base = match &self.updated_since {
            Some(d) => format!("is:pr is:open archived:false updated:>={d}"),
            None => "is:pr is:open archived:false".to_string(),
        };
        let mut var_defs = vec![
            "$qReview: String!",
            "$qAuthor: String!",
            "$qInvolves: String!",
        ];
        let mut body = String::from(
            "
  reviewRequested: search(query: $qReview, type: ISSUE, first: 30) { nodes { ...PrFields } }
  authored: search(query: $qAuthor, type: ISSUE, first: 30) { nodes { ...PrFields } }
  involved: search(query: $qInvolves, type: ISSUE, first: 30) { nodes { ...PrFields } }
",
        );
        let mut vars = json!({
            "qReview": format!("{base} review-requested:@me"),
            "qAuthor": format!("{base} author:@me"),
            "qInvolves": format!("{base} involves:@me"),
        });

        if !self.watched_repos.is_empty() {
            var_defs.push("$qWatched: String!");
            body.push_str(
                "  watched: search(query: $qWatched, type: ISSUE, first: 50) { nodes { ...PrFields } }\n",
            );
            let repo_quals: Vec<String> =
                self.watched_repos.iter().map(|r| format!("repo:{r}")).collect();
            vars["qWatched"] = json!(format!("{base} {}", repo_quals.join(" ")));
        }

        if !self.tracked_ids.is_empty() {
            var_defs.push("$trackedIds: [ID!]!");
            body.push_str("  tracked: nodes(ids: $trackedIds) { ... on PullRequest { ...PrFields } }\n");
            vars["trackedIds"] = json!(self.tracked_ids);
        }

        body.push_str("  rateLimit { cost remaining resetAt }\n");
        let doc = format!("query Poll({}) {{{}}}\n{}", var_defs.join(", "), body, PR_FRAGMENT);
        (doc, vars)
    }
}

/// PR nodes per alias, in the response.
pub const SEARCH_ALIASES: [&str; 4] = ["reviewRequested", "authored", "involved", "watched"];

pub struct GraphQlClient {
    http: reqwest::Client,
    url: String,
    token: String,
}

impl GraphQlClient {
    pub fn new(url: &str, token: &str) -> AppResult<Self> {
        // 60s: the batched poll computes viewer-scoped fields across ~140
        // PRs and GitHub can stream that slowly — a mid-body timeout
        // surfaces as a cryptic "error decoding response body".
        let http = reqwest::Client::builder()
            .user_agent("cora-pr-review")
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self { http, url: url.to_string(), token: token.to_string() })
    }

    /// GitHub's GraphQL edge throws transient 5xx / truncated bodies fairly
    /// often, especially on heavy batched queries — retry those a couple of
    /// times with backoff before surfacing a failure.
    pub async fn run(&self, query: &str, variables: &Value) -> AppResult<Value> {
        let mut attempt = 0u32;
        loop {
            match self.run_once(query, variables).await {
                Err(e) if attempt < 2 && is_transient(&e) => {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_secs(3 * attempt as u64)).await;
                }
                other => return other,
            }
        }
    }

    async fn run_once(&self, query: &str, variables: &Value) -> AppResult<Value> {
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(AppError::GitHub("authentication failed — check your PAT".into()));
        }
        if !status.is_success() {
            return Err(AppError::GitHub(format!("HTTP {status}")));
        }
        let body: Value = resp.json().await?;
        if let Some(errors) = body.get("errors").and_then(Value::as_array) {
            // Queries tolerate partial data (stale tracked ids surface as
            // NOT_FOUND node errors next to perfectly good results). Mutations
            // never do: a rejected mutation comes back as
            // {"data": {"theMutation": null}, "errors": [...]} — non-null data
            // — and swallowing that reports success for a write GitHub refused.
            let is_mutation = query.trim_start().starts_with("mutation");
            let data_null = body.pointer("/data").map(Value::is_null).unwrap_or(true);
            if is_mutation || data_null {
                let msgs: Vec<String> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str).map(String::from))
                    .collect();
                let joined = if msgs.is_empty() { "GraphQL error".into() } else { msgs.join("; ") };
                return Err(AppError::GitHub(joined));
            }
        }
        body.get("data")
            .cloned()
            .ok_or_else(|| AppError::GitHub("empty response".into()))
    }
}

/// Per-PR review context fetched in cheap chunks: the viewer's own latest
/// review, plus the newest opinionated review (approve / request changes)
/// by anyone — the actor behind a review-state change.
#[derive(Debug, Clone, Default)]
pub struct ViewerReviewInfo {
    pub my_state: Option<String>,
    pub my_at: Option<String>,
    pub rerequested: bool,
    /// (author, state) of the most recent APPROVED / CHANGES_REQUESTED review.
    pub last_opinion: Option<(String, String)>,
}

pub type ViewerReviews = HashMap<String, ViewerReviewInfo>;

/// viewer-scoped review fields are computed per-viewer per-PR on GitHub's
/// side — batching them into the main ~140-node poll pushes the query past
/// GitHub's gateway timeout (persistent 502s). Fetch them separately, in
/// small chunks that each stay comfortably cheap.
pub async fn fetch_viewer_reviews(
    client: &GraphQlClient,
    ids: &[String],
) -> AppResult<ViewerReviews> {
    const DOC: &str = "query($ids: [ID!]!) { nodes(ids: $ids) { ... on PullRequest {
        id
        viewerLatestReview { state submittedAt }
        viewerLatestReviewRequest { id }
        latestOpinionatedReviews(last: 1) { nodes { author { login } state } }
    } } }";
    let mut out = HashMap::new();
    for chunk in ids.chunks(40) {
        let data = client.run(DOC, &json!({ "ids": chunk })).await?;
        let Some(nodes) = data.pointer("/nodes").and_then(Value::as_array) else { continue };
        for n in nodes {
            let Some(id) = n.get("id").and_then(Value::as_str) else { continue };
            let last_opinion = n
                .pointer("/latestOpinionatedReviews/nodes/0")
                .and_then(|r| {
                    Some((
                        r.pointer("/author/login")?.as_str()?.to_string(),
                        r.get("state")?.as_str()?.to_string(),
                    ))
                });
            out.insert(
                id.to_string(),
                ViewerReviewInfo {
                    my_state: n
                        .pointer("/viewerLatestReview/state")
                        .and_then(Value::as_str)
                        .map(String::from),
                    my_at: n
                        .pointer("/viewerLatestReview/submittedAt")
                        .and_then(Value::as_str)
                        .map(String::from),
                    rerequested: n
                        .pointer("/viewerLatestReviewRequest/id")
                        .and_then(Value::as_str)
                        .is_some(),
                    last_opinion,
                },
            );
        }
    }
    Ok(out)
}

/// Worth an automatic retry: gateway hiccups (502/503/504), timeouts, and
/// bodies that got cut off mid-stream. Auth and query errors are not.
fn is_transient(e: &AppError) -> bool {
    match e {
        AppError::Http(e) => e.is_timeout() || e.is_connect() || e.is_decode(),
        AppError::GitHub(msg) => {
            msg.contains("502") || msg.contains("503") || msg.contains("504")
        }
        _ => false,
    }
}

/// Extract PR nodes per alias from a poll response.
/// Returns alias → list of raw PR values.
pub fn split_by_alias(data: &Value) -> HashMap<&'static str, Vec<Value>> {
    let mut out = HashMap::new();
    for alias in SEARCH_ALIASES {
        if let Some(nodes) = data.pointer(&format!("/{alias}/nodes")).and_then(Value::as_array) {
            out.insert(alias, nodes.clone());
        }
    }
    if let Some(nodes) = data.pointer("/tracked").and_then(Value::as_array) {
        // nodes(ids:) yields nulls for unresolvable ids — drop them.
        out.insert("tracked", nodes.iter().filter(|n| !n.is_null()).cloned().collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omits_empty_scopes() {
        let (doc, vars) = PollRequest {
            watched_repos: vec![],
            tracked_ids: vec![],
            updated_since: None,
        }
        .build();
        assert!(!doc.contains("watched:"));
        assert!(!doc.contains("tracked:"));
        assert!(doc.contains("reviewRequested:"));
        assert!(vars.get("qWatched").is_none());
    }

    #[test]
    fn includes_watched_and_tracked() {
        let (doc, vars) = PollRequest {
            watched_repos: vec!["acme/api".into(), "acme/web".into()],
            tracked_ids: vec!["PR_x".into()],
            updated_since: None,
        }
        .build();
        assert!(doc.contains("watched: search"));
        assert!(doc.contains("tracked: nodes"));
        assert_eq!(
            vars["qWatched"].as_str().unwrap(),
            "is:pr is:open archived:false repo:acme/api repo:acme/web"
        );
    }

    #[test]
    fn updated_since_qualifies_every_search_scope() {
        let (_, vars) = PollRequest {
            watched_repos: vec!["acme/api".into()],
            tracked_ids: vec![],
            updated_since: Some("2026-06-16".into()),
        }
        .build();
        for key in ["qReview", "qAuthor", "qInvolves", "qWatched"] {
            assert!(
                vars[key].as_str().unwrap().contains("updated:>=2026-06-16"),
                "{key} missing recency qualifier"
            );
        }
    }
}
