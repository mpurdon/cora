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
}

impl PollRequest {
    /// Build one batched GraphQL document + variables. Aliases for empty
    /// scopes are omitted entirely (GraphQL requires every variable be used).
    pub fn build(&self) -> (String, Value) {
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
            "qReview": "is:pr is:open archived:false review-requested:@me",
            "qAuthor": "is:pr is:open archived:false author:@me",
            "qInvolves": "is:pr is:open archived:false involves:@me",
        });

        if !self.watched_repos.is_empty() {
            var_defs.push("$qWatched: String!");
            body.push_str(
                "  watched: search(query: $qWatched, type: ISSUE, first: 50) { nodes { ...PrFields } }\n",
            );
            let repo_quals: Vec<String> =
                self.watched_repos.iter().map(|r| format!("repo:{r}")).collect();
            vars["qWatched"] =
                json!(format!("is:pr is:open archived:false {}", repo_quals.join(" ")));
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
        let http = reqwest::Client::builder()
            .user_agent("cora-pr-review")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { http, url: url.to_string(), token: token.to_string() })
    }

    pub async fn run(&self, query: &str, variables: &Value) -> AppResult<Value> {
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
            // Partial data with NOT_FOUND node errors is fine (stale tracked
            // ids); anything without data is a hard failure.
            if body.pointer("/data").map(Value::is_null).unwrap_or(true) {
                let msgs: Vec<String> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(Value::as_str).map(String::from))
                    .collect();
                return Err(AppError::GitHub(msgs.join("; ")));
            }
        }
        body.get("data")
            .cloned()
            .ok_or_else(|| AppError::GitHub("empty response".into()))
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
        let (doc, vars) = PollRequest { watched_repos: vec![], tracked_ids: vec![] }.build();
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
        }
        .build();
        assert!(doc.contains("watched: search"));
        assert!(doc.contains("tracked: nodes"));
        assert_eq!(
            vars["qWatched"].as_str().unwrap(),
            "is:pr is:open archived:false repo:acme/api repo:acme/web"
        );
    }
}
