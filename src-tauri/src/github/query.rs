use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::Manager;

use crate::error::{AppError, AppResult};
use crate::health::{Admission, EndpointHealth};

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

/// The scopes we poll, in priority order. Each is issued as its own request
/// (see `run_poll`), not batched into one document.
pub struct PollRequest {
    /// Org login every scope is fenced to (`org:<login>` qualifier) — the
    /// isolation boundary: one poll request never sees another org's PRs.
    pub org: String,
    pub watched_repos: Vec<String>,
    pub tracked_ids: Vec<String>,
    /// YYYY-MM-DD; when set, every search scope gains `updated:>=` so PRs
    /// idle beyond the settings window never enter discovery.
    pub updated_since: Option<String>,
}

impl PollRequest {
    /// The shared search prefix: open PRs in this org, within the recency
    /// window. Every scope's query is this plus its own qualifier.
    fn base(&self) -> String {
        let mut base = match &self.updated_since {
            Some(d) => format!("is:pr is:open archived:false updated:>={d}"),
            None => "is:pr is:open archived:false".to_string(),
        };
        if !self.org.is_empty() {
            // `user:` matches repos owned by either a user or an organization
            // (orgs are users in GitHub's model) — `org:` would exclude
            // personal accounts like a solo login.
            base.push_str(&format!(" user:{}", self.org));
        }
        base
    }

    /// The search scopes to issue, as (alias, full query string, page size).
    /// Each becomes its own request — see `run_poll` for why they aren't
    /// batched.
    fn search_scopes(&self) -> Vec<(&'static str, String, u32)> {
        let base = self.base();
        let mut scopes = vec![
            ("reviewRequested", format!("{base} review-requested:@me"), 30),
            ("authored", format!("{base} author:@me"), 30),
            ("involved", format!("{base} involves:@me"), 30),
        ];
        if !self.watched_repos.is_empty() {
            let repo_quals: Vec<String> =
                self.watched_repos.iter().map(|r| format!("repo:{r}")).collect();
            scopes.push(("watched", format!("{base} {}", repo_quals.join(" ")), 50));
        }
        scopes
    }
}

/// Fold a response's `rateLimit.remaining` into the running minimum — a poll
/// now spans several requests, so report the tightest budget we saw.
fn merge_rate(rate: &mut Option<i64>, data: &Value) {
    if let Some(r) = data.pointer("/rateLimit/remaining").and_then(Value::as_i64) {
        *rate = Some(rate.map_or(r, |cur| cur.min(r)));
    }
}

/// Fetch every poll scope as its own small request and return alias → raw PR
/// nodes plus the tightest rate-limit remaining.
///
/// The scopes are deliberately NOT batched into one document. The full
/// `PrFields` fragment (mergeable, status rollup, recent comments, labels)
/// across four searches plus the tracked-node re-fetch pushes a single query
/// past GitHub's GraphQL gateway limit — it 502s (or nulls out nodes with a
/// wall of per-field timeout errors) the large majority of the time. The same
/// reason `fetch_viewer_reviews` is chunked out. Kept apart, each request
/// stays cheap and the 502-aware retry in `run` mops up the occasional edge
/// hiccup.
pub async fn run_poll(
    client: &GraphQlClient,
    request: &PollRequest,
) -> AppResult<(HashMap<&'static str, Vec<Value>>, Option<i64>)> {
    let mut aliased: HashMap<&'static str, Vec<Value>> = HashMap::new();
    let mut rate: Option<i64> = None;

    for (alias, query, first) in request.search_scopes() {
        let doc = format!(
            "query($q: String!) {{ search(query: $q, type: ISSUE, first: {first}) \
             {{ nodes {{ ...PrFields }} }} rateLimit {{ remaining }} }}\n{PR_FRAGMENT}"
        );
        let data = client.run(&doc, &json!({ "q": query })).await?;
        if let Some(nodes) = data.pointer("/search/nodes").and_then(Value::as_array) {
            aliased.insert(alias, nodes.clone());
        }
        merge_rate(&mut rate, &data);
    }

    // Tracked PRs are re-fetched in small chunks (same reasoning as the viewer
    // reviews): one `nodes(ids:)` request per chunk stays comfortably cheap.
    if !request.tracked_ids.is_empty() {
        let doc = format!(
            "query($ids: [ID!]!) {{ nodes(ids: $ids) \
             {{ ... on PullRequest {{ ...PrFields }} }} rateLimit {{ remaining }} }}\n{PR_FRAGMENT}"
        );
        let mut tracked: Vec<Value> = Vec::new();
        for chunk in request.tracked_ids.chunks(30) {
            let data = client.run(&doc, &json!({ "ids": chunk })).await?;
            if let Some(nodes) = data.pointer("/nodes").and_then(Value::as_array) {
                // nodes(ids:) yields nulls for unresolvable ids — drop them.
                tracked.extend(nodes.iter().filter(|n| !n.is_null()).cloned());
            }
            merge_rate(&mut rate, &data);
        }
        aliased.insert("tracked", tracked);
    }

    Ok((aliased, rate))
}

pub struct GraphQlClient {
    http: reqwest::Client,
    url: String,
    token: String,
    /// Shared with every other client for this endpoint. `None` in tests and
    /// in the few call sites with no `AppHandle` to hand — those simply do not
    /// participate in the breaker rather than getting a private one, which
    /// would be worse than none at all (a breaker nobody else can see).
    health: Option<Arc<EndpointHealth>>,
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
        Ok(Self { http, url: url.to_string(), token: token.to_string(), health: None })
    }

    /// Enrol this client in the process-wide GitHub breaker. Separate from
    /// `new` so the existing construction sites keep working unchanged and opt
    /// in by threading the `AppHandle` they already hold. Takes the shared
    /// handle rather than the `AppHandle` so tests can inject one.
    pub fn with_health(mut self, health: Arc<EndpointHealth>) -> Self {
        self.health = Some(health);
        self
    }

    /// The app-state form of [`with_health`], for the ordinary call sites.
    pub fn shared_health(app: &tauri::AppHandle) -> Arc<EndpointHealth> {
        app.state::<crate::health::GitHubHealth>().0.clone()
    }

    /// GitHub's GraphQL edge throws transient 5xx / truncated bodies fairly
    /// often, especially on heavy batched queries — retry those a couple of
    /// times with backoff before surfacing a failure.
    pub async fn run(&self, query: &str, variables: &Value) -> AppResult<Value> {
        // One poll cycle is dozens of chunked queries and the UI fires more
        // alongside it. When GitHub is down, the first one to find out speaks
        // for all of them.
        if let Some(health) = &self.health {
            if let Admission::Blocked(wait) = health.admit() {
                return Err(AppError::github(format!(
                    "GitHub is unreachable — Cora is holding requests for {}s, then trying one automatically.",
                    wait.as_secs()
                )));
            }
        }
        let mut attempt = 0u32;
        let outcome = loop {
            match self.run_once(query, variables).await {
                Err(e) if attempt < 2 && is_transient(&e) => {
                    // A sibling query already proved the endpoint down; stop.
                    if self.health.as_ref().is_some_and(|h| h.is_open()) {
                        break Err(AppError::github(
                            "GitHub is unreachable — Cora is holding requests and will retry one automatically.",
                        ));
                    }
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_secs(3 * attempt as u64)).await;
                }
                other => break other,
            }
        };
        if let Some(health) = &self.health {
            match &outcome {
                // Only reachability moves the breaker. A 401 or a rejected
                // mutation is GitHub answering us — the endpoint is fine, and
                // holding every other query because of one bad token or a
                // stale node id would be a self-inflicted outage.
                Err(e) if is_transient(e) => {
                    health.record_outage();
                }
                _ => health.record_success(),
            }
        }
        outcome
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
            return Err(AppError::github_status(status.as_u16(), "authentication failed — check your PAT"));
        }
        if !status.is_success() {
            return Err(AppError::github_status(status.as_u16(), format!("HTTP {status}")));
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
                return Err(AppError::github(joined));
            }
        }
        body.get("data")
            .cloned()
            .ok_or_else(|| AppError::github("empty response"))
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
        AppError::GitHub { message: msg, .. } => {
            msg.contains("502") || msg.contains("503") || msg.contains("504")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_open_breaker_short_circuits_before_the_network() {
        // The URL is unroutable, so if the breaker is consulted the call
        // returns instantly; if it is not, this blocks on connect timeouts.
        let health = Arc::new(EndpointHealth::default());
        health.record_outage();
        let client = GraphQlClient::new("http://127.0.0.1:1/graphql", "t")
            .unwrap()
            .with_health(health.clone());

        let started = std::time::Instant::now();
        let err = client.run("query { viewer { login } }", &json!({})).await.unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "should not have dialled");
        assert!(err.to_string().contains("holding requests"), "got: {err}");
    }

    #[test]
    fn only_reachability_failures_trip_the_breaker() {
        // `is_transient` is what decides whether a failure opens the shared
        // breaker, so a misclassification here stops every GitHub query in the
        // app, not just this one.
        for status in [502, 503, 504] {
            assert!(
                is_transient(&AppError::github_status(status, format!("HTTP {status}"))),
                "gateway {status} should trip"
            );
        }
        // GitHub answering us is not GitHub being unreachable.
        assert!(!is_transient(&AppError::github_status(401, "authentication failed — check your PAT")));
        assert!(!is_transient(&AppError::github_status(404, "HTTP 404")));
        assert!(!is_transient(&AppError::github("Could not resolve to a node with the global id")));
        // The breaker's own refusal must not read as an outage and re-arm it.
        assert!(!is_transient(&AppError::github("GitHub is unreachable — Cora is holding requests")));
    }

    fn scope_map(req: &PollRequest) -> HashMap<&'static str, String> {
        req.search_scopes().into_iter().map(|(a, q, _)| (a, q)).collect()
    }

    #[test]
    fn omits_watched_scope_when_no_repos() {
        let scopes = scope_map(&PollRequest {
            org: String::new(),
            watched_repos: vec![],
            tracked_ids: vec![],
            updated_since: None,
        });
        assert!(!scopes.contains_key("watched"));
        assert!(scopes.contains_key("reviewRequested"));
    }

    #[test]
    fn includes_watched_scope() {
        let scopes = scope_map(&PollRequest {
            org: String::new(),
            watched_repos: vec!["acme/api".into(), "acme/web".into()],
            tracked_ids: vec!["PR_x".into()],
            updated_since: None,
        });
        assert_eq!(
            scopes["watched"],
            "is:pr is:open archived:false repo:acme/api repo:acme/web"
        );
    }

    #[test]
    fn org_fences_every_search_scope() {
        let scopes = scope_map(&PollRequest {
            org: "team-and-tech".into(),
            watched_repos: vec!["team-and-tech/api".into()],
            tracked_ids: vec![],
            updated_since: None,
        });
        assert_eq!(scopes.len(), 4);
        for (alias, query) in &scopes {
            assert!(
                query.contains("user:team-and-tech"),
                "{alias} missing org fence"
            );
        }
    }

    #[test]
    fn updated_since_qualifies_every_search_scope() {
        let scopes = scope_map(&PollRequest {
            org: String::new(),
            watched_repos: vec!["acme/api".into()],
            tracked_ids: vec![],
            updated_since: Some("2026-06-16".into()),
        });
        for (alias, query) in &scopes {
            assert!(
                query.contains("updated:>=2026-06-16"),
                "{alias} missing recency qualifier"
            );
        }
    }
}
