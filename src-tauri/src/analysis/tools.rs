use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

/// GitHub REST client backing the agent's context-fetching tools.
/// No clones — everything is fetched on demand.
pub struct RepoTools {
    http: reqwest::Client,
    api_base: String,
    repo: String,
    head_ref: String,
    number: i64,
    token: String,
    /// The full diff is wanted by both the metrics pass and the model's
    /// get_pr_diff tool in the same run — fetch it once per instance.
    diff_cache: tokio::sync::OnceCell<String>,
}

const MAX_FILE_CHARS: usize = 40_000;
const MAX_DIFF_CHARS: usize = 60_000;

impl RepoTools {
    pub fn new(graphql_url: &str, repo: &str, number: i64, head_ref: &str, token: &str) -> AppResult<Self> {
        // Derive the REST base from the GraphQL endpoint (GHE support).
        let api_base = graphql_url
            .trim_end_matches("/graphql")
            .trim_end_matches('/')
            .to_string();
        let http = reqwest::Client::builder()
            .user_agent("cora-pr-review")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            api_base,
            repo: repo.to_string(),
            head_ref: head_ref.to_string(),
            number,
            token: token.to_string(),
            diff_cache: tokio::sync::OnceCell::new(),
        })
    }

    async fn get(&self, path: &str, accept: &str) -> AppResult<reqwest::Response> {
        let resp = self
            .http
            .get(format!("{}/{}", self.api_base, path))
            .bearer_auth(&self.token)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AppError::GitHub(format!(
                "GET {path}: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp)
    }

    /// The tool specs the model sees. Names must match `execute`.
    pub fn specs() -> Vec<(&'static str, &'static str, Value)> {
        vec![
            (
                "get_pr_diff",
                "Get the full unified diff of the pull request under review.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            (
                "get_file",
                "Read a file from the repository at the PR head (or an explicit ref).",
                json!({"type": "object", "properties": {
                    "path": {"type": "string", "description": "File path from repo root"},
                    "ref": {"type": "string", "description": "Optional git ref; defaults to the PR head"}
                }, "required": ["path"]}),
            ),
            (
                "list_tree",
                "List a directory in the repository (names, types, sizes). Use to explore structure.",
                json!({"type": "object", "properties": {
                    "path": {"type": "string", "description": "Directory path; empty for repo root"}
                }, "required": []}),
            ),
            (
                "search_code",
                "Search code in this repository. Returns matching file paths with fragments.",
                json!({"type": "object", "properties": {
                    "query": {"type": "string", "description": "Search terms (GitHub code search syntax)"}
                }, "required": ["query"]}),
            ),
            (
                "get_readme_and_docs",
                "Get the repository README and a listing of docs/ if present. Good first call for architecture context.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            (
                "list_recent_prs",
                "List recently merged PRs in this repo (titles), to see how this area has been changing.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
        ]
    }

    pub async fn execute(&self, name: &str, input: &Value) -> AppResult<String> {
        match name {
            "get_pr_diff" => self.pr_diff().await,
            "get_file" => {
                let path = input
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::Other("get_file: missing path".into()))?;
                let r#ref = input.get("ref").and_then(Value::as_str);
                self.file(path, r#ref).await
            }
            "list_tree" => {
                let path = input.get("path").and_then(Value::as_str).unwrap_or("");
                self.tree(path).await
            }
            "search_code" => {
                let query = input
                    .get("query")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::Other("search_code: missing query".into()))?;
                self.search(query).await
            }
            "get_readme_and_docs" => self.readme_and_docs().await,
            "list_recent_prs" => self.recent_prs().await,
            other => Err(AppError::Other(format!("unknown tool: {other}"))),
        }
    }

    async fn pr_diff(&self) -> AppResult<String> {
        let mut text = self.pr_diff_full().await?;
        if text.len() > MAX_DIFF_CHARS {
            text.truncate(MAX_DIFF_CHARS);
            text.push_str("\n\n[diff truncated — use get_file for specific files]");
        }
        Ok(text)
    }

    /// Authenticated REST POST (used by mutations like line comments).
    pub async fn post(&self, path: &str, body: &Value) -> AppResult<Value> {
        let resp = self
            .http
            .post(format!("{}/{}", self.api_base, path))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            return Err(AppError::GitHub(format!("POST {path}: HTTP {status} — {detail}")));
        }
        Ok(resp.json().await?)
    }

    pub fn pr_number(&self) -> i64 {
        self.number
    }

    pub fn head_ref(&self) -> &str {
        &self.head_ref
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Diff of a commit range (base…head) — "changes since my last look".
    pub async fn compare_diff(&self, base: &str) -> AppResult<String> {
        let resp = self
            .get(
                &format!("repos/{}/compare/{}...{}", self.repo, base, self.head_ref),
                "application/vnd.github.v3.diff",
            )
            .await?;
        Ok(resp.text().await?)
    }

    /// Untruncated diff, fetched once per instance (head-pinned, can't go stale).
    pub async fn pr_diff_full(&self) -> AppResult<String> {
        let diff = self
            .diff_cache
            .get_or_try_init(|| async {
                let resp = self
                    .get(
                        &format!("repos/{}/pulls/{}", self.repo, self.number),
                        "application/vnd.github.v3.diff",
                    )
                    .await?;
                Ok::<_, crate::error::AppError>(resp.text().await?)
            })
            .await?;
        Ok(diff.clone())
    }

    async fn file(&self, path: &str, r#ref: Option<&str>) -> AppResult<String> {
        let r = r#ref.unwrap_or(&self.head_ref);
        let resp = self
            .get(
                &format!("repos/{}/contents/{}?ref={}", self.repo, path, r),
                "application/vnd.github.raw+json",
            )
            .await?;
        let mut text = resp.text().await?;
        if text.len() > MAX_FILE_CHARS {
            text.truncate(MAX_FILE_CHARS);
            text.push_str("\n\n[file truncated]");
        }
        Ok(text)
    }

    async fn tree(&self, path: &str) -> AppResult<String> {
        let resp = self
            .get(
                &format!(
                    "repos/{}/contents/{}?ref={}",
                    self.repo,
                    path.trim_matches('/'),
                    self.head_ref
                ),
                "application/vnd.github+json",
            )
            .await?;
        let entries: Value = resp.json().await?;
        let Some(list) = entries.as_array() else {
            return Ok("(not a directory — use get_file)".into());
        };
        let lines: Vec<String> = list
            .iter()
            .filter_map(|e| {
                let name = e.get("name")?.as_str()?;
                let kind = e.get("type")?.as_str()?;
                let size = e.get("size").and_then(Value::as_i64).unwrap_or(0);
                Some(if kind == "dir" {
                    format!("{name}/")
                } else {
                    format!("{name} ({size} bytes)")
                })
            })
            .collect();
        Ok(lines.join("\n"))
    }

    async fn search(&self, query: &str) -> AppResult<String> {
        let q = format!("{query} repo:{}", self.repo);
        let resp = self
            .get(
                &format!("search/code?q={}&per_page=10", urlencode(&q)),
                "application/vnd.github.text-match+json",
            )
            .await?;
        let body: Value = resp.json().await?;
        let items = body
            .pointer("/items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if items.is_empty() {
            return Ok("(no matches)".into());
        }
        let lines: Vec<String> = items
            .iter()
            .filter_map(|i| {
                let path = i.get("path")?.as_str()?.to_string();
                let fragment = i
                    .pointer("/text_matches/0/fragment")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Some(format!("## {path}\n{fragment}"))
            })
            .collect();
        Ok(lines.join("\n\n"))
    }

    async fn readme_and_docs(&self) -> AppResult<String> {
        let readme = match self
            .get(
                &format!("repos/{}/readme", self.repo),
                "application/vnd.github.raw+json",
            )
            .await
        {
            Ok(resp) => {
                let mut t = resp.text().await?;
                if t.len() > MAX_FILE_CHARS {
                    t.truncate(MAX_FILE_CHARS);
                    t.push_str("\n[truncated]");
                }
                t
            }
            Err(_) => "(no README)".into(),
        };
        let docs = self.tree("docs").await.unwrap_or_else(|_| "(no docs/ directory)".into());
        Ok(format!("# README\n{readme}\n\n# docs/ listing\n{docs}"))
    }

    async fn recent_prs(&self) -> AppResult<String> {
        let resp = self
            .get(
                &format!(
                    "repos/{}/pulls?state=closed&sort=updated&direction=desc&per_page=10",
                    self.repo
                ),
                "application/vnd.github+json",
            )
            .await?;
        let body: Value = resp.json().await?;
        let lines: Vec<String> = body
            .as_array()
            .map(|prs| {
                prs.iter()
                    .filter(|p| !p.get("merged_at").map(Value::is_null).unwrap_or(true))
                    .filter_map(|p| {
                        Some(format!(
                            "#{} {}",
                            p.get("number")?.as_i64()?,
                            p.get("title")?.as_str()?
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(if lines.is_empty() {
            "(no recently merged PRs)".into()
        } else {
            lines.join("\n")
        })
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
