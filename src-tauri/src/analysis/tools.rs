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
    /// GitHub code search allows ~10 requests/min; concurrent tool calls
    /// serialize through this gate with spacing instead of bursting into 429s.
    search_gate: tokio::sync::Mutex<Option<std::time::Instant>>,
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
            search_gate: tokio::sync::Mutex::new(None),
        })
    }

    async fn get(&self, path: &str, accept: &str) -> AppResult<reqwest::Response> {
        for attempt in 0..3u32 {
            let resp = self
                .http
                .get(format!("{}/{}", self.api_base, path))
                .bearer_auth(&self.token)
                .header("Accept", accept)
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .await?;
            let status = resp.status();
            // Rate-limited: honor Retry-After (capped) and try again rather
            // than handing the model a dead tool.
            let limited = status == 429
                || (status == 403
                    && resp
                        .headers()
                        .get("x-ratelimit-remaining")
                        .and_then(|v| v.to_str().ok())
                        == Some("0"));
            if limited && attempt < 2 {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(15)
                    .min(60);
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                continue;
            }
            if !status.is_success() {
                return Err(AppError::github_status(status.as_u16(), format!("GET {path}: HTTP {status}")));
            }
            return Ok(resp);
        }
        unreachable!("retry loop always returns")
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
            (
                "list_commits",
                "List this PR's commits in order (sha, author, date, first message line) — how the change was built up.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            (
                "get_commit_diff",
                "One commit's full message and per-file diff, by sha from list_commits. Use to review commit-by-commit or to see when something changed.",
                json!({"type": "object", "properties": {
                    "sha": {"type": "string", "description": "Commit sha (full or abbreviated)"}
                }, "required": ["sha"]}),
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
            "list_commits" => self.commits().await,
            "get_commit_diff" => {
                let sha = input
                    .get("sha")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::Other("get_commit_diff: missing sha".into()))?;
                self.commit_diff(sha).await
            }
            other => Err(AppError::Other(format!("unknown tool: {other}"))),
        }
    }

    /// Truncated-with-notice diff — the same view the get_pr_diff tool
    /// serves; also embedded in drill kickoffs.
    pub(crate) async fn pr_diff(&self) -> AppResult<String> {
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
            return Err(AppError::github_status(
                status.as_u16(),
                format!("POST {path}: HTTP {status} — {detail}"),
            ));
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
                match self
                    .get(
                        &format!("repos/{}/pulls/{}", self.repo, self.number),
                        "application/vnd.github.v3.diff",
                    )
                    .await
                {
                    Ok(resp) => Ok::<_, crate::error::AppError>(resp.text().await?),
                    // GitHub refuses the whole-PR diff media type past ~20k
                    // lines / 300 files (406). Reassemble from the paginated
                    // per-file endpoint, which has no such ceiling.
                    Err(e) if e.status() == Some(406) => self.diff_from_files().await,
                    Err(e) => Err(e),
                }
            })
            .await?;
        Ok(diff.clone())
    }

    /// Reconstruct a unified diff from `pulls/{n}/files` — the fallback for
    /// PRs whose combined diff GitHub won't serve. Per-file patches GitHub
    /// still omits (giant/binary) become a marker hunk so the UI can say so.
    async fn diff_from_files(&self) -> AppResult<String> {
        let mut out = String::new();
        for page in 1..=30 {
            let resp = self
                .get(
                    &format!(
                        "repos/{}/pulls/{}/files?per_page=100&page={page}",
                        self.repo, self.number
                    ),
                    "application/vnd.github+json",
                )
                .await?;
            let files: serde_json::Value = resp.json().await.map_err(crate::error::AppError::from)?;
            let Some(arr) = files.as_array() else { break };
            if arr.is_empty() {
                break;
            }
            for f in arr {
                let path = f.get("filename").and_then(serde_json::Value::as_str).unwrap_or("?");
                let prev = f
                    .get("previous_filename")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(path);
                let status = f.get("status").and_then(serde_json::Value::as_str).unwrap_or("");
                out.push_str(&format!("diff --git a/{prev} b/{path}\n"));
                if status == "removed" {
                    out.push_str("deleted file mode 100644\n");
                } else if status == "added" {
                    out.push_str("new file mode 100644\n");
                }
                let a = if status == "added" { "/dev/null".into() } else { format!("a/{prev}") };
                let b = if status == "removed" { "/dev/null".into() } else { format!("b/{path}") };
                out.push_str(&format!("--- {a}\n+++ {b}\n"));
                match f.get("patch").and_then(serde_json::Value::as_str) {
                    Some(p) => {
                        out.push_str(p);
                        out.push('\n');
                    }
                    None => out.push_str("@@ -0,0 +0,0 @@ patch omitted by GitHub (too large or binary)\n"),
                }
            }
            if arr.len() < 100 {
                break;
            }
        }
        if out.is_empty() {
            return Err(crate::error::AppError::Other(
                "GitHub returned no diff for this PR (too large)".into(),
            ));
        }
        Ok(out)
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
        // Serialize concurrent searches with spacing (~9/min) — the gate is
        // held across the request so a burst of tool calls forms a queue.
        let mut last = self.search_gate.lock().await;
        if let Some(prev) = *last {
            let min_gap = std::time::Duration::from_millis(6500);
            if prev.elapsed() < min_gap {
                tokio::time::sleep(min_gap - prev.elapsed()).await;
            }
        }
        let result = self.search_inner(query).await;
        *last = Some(std::time::Instant::now());
        result
    }

    async fn search_inner(&self, query: &str) -> AppResult<String> {
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

    async fn commits(&self) -> AppResult<String> {
        let mut lines: Vec<String> = Vec::new();
        for page in 1..=3 {
            let resp = self
                .get(
                    &format!(
                        "repos/{}/pulls/{}/commits?per_page=100&page={page}",
                        self.repo, self.number
                    ),
                    "application/vnd.github+json",
                )
                .await?;
            let body: Value = resp.json().await?;
            let Some(arr) = body.as_array() else { break };
            for c in arr {
                let sha = c.get("sha").and_then(Value::as_str).unwrap_or("");
                let author = c
                    .pointer("/author/login")
                    .and_then(Value::as_str)
                    .or_else(|| c.pointer("/commit/author/name").and_then(Value::as_str))
                    .unwrap_or("?");
                let date = c
                    .pointer("/commit/author/date")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let subject = c
                    .pointer("/commit/message")
                    .and_then(Value::as_str)
                    .and_then(|m| m.lines().next())
                    .unwrap_or("");
                lines.push(format!("{} {date} {author} — {subject}", &sha[..sha.len().min(10)]));
            }
            if arr.len() < 100 {
                break;
            }
        }
        Ok(if lines.is_empty() { "(no commits)".into() } else { lines.join("\n") })
    }

    async fn commit_diff(&self, sha: &str) -> AppResult<String> {
        // The JSON commit endpoint carries message and per-file patches in
        // one response — no second diff-media fetch needed.
        let resp = self
            .get(
                &format!("repos/{}/commits/{sha}", self.repo),
                "application/vnd.github+json",
            )
            .await?;
        let c: Value = resp.json().await?;
        let mut out = format!(
            "commit {}\nAuthor: {} · {}\n\n{}\n",
            c.get("sha").and_then(Value::as_str).unwrap_or(sha),
            c.pointer("/commit/author/name").and_then(Value::as_str).unwrap_or("?"),
            c.pointer("/commit/author/date").and_then(Value::as_str).unwrap_or(""),
            c.pointer("/commit/message").and_then(Value::as_str).unwrap_or(""),
        );
        for f in c.get("files").and_then(Value::as_array).into_iter().flatten() {
            let path = f.get("filename").and_then(Value::as_str).unwrap_or("?");
            let status = f.get("status").and_then(Value::as_str).unwrap_or("");
            let add = f.get("additions").and_then(Value::as_i64).unwrap_or(0);
            let del = f.get("deletions").and_then(Value::as_i64).unwrap_or(0);
            out.push_str(&format!("\n--- {path} ({status}, +{add} −{del})\n"));
            match f.get("patch").and_then(Value::as_str) {
                Some(p) => {
                    out.push_str(p);
                    out.push('\n');
                }
                None => out.push_str("(patch omitted by GitHub — too large or binary)\n"),
            }
        }
        if out.len() > MAX_DIFF_CHARS {
            out.truncate(MAX_DIFF_CHARS);
            out.push_str("\n\n[commit diff truncated]");
        }
        Ok(out)
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
