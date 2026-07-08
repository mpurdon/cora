use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, InferenceConfiguration, Message, StopReason,
    SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema, ToolResultBlock,
    ToolResultContentBlock, ToolSpecification,
};
use aws_smithy_types::Document;
use chrono::Utc;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::analysis::tools::RepoTools;
use crate::devlog;
use crate::analysis::types::{
    events, AnalysisLevel, AnalysisProgress, AnalysisResult, AnalysisUsage, Assessment, C4Graph,
    TraceStep,
};
use crate::error::{AppError, AppResult};
use crate::models::{Settings, TrackedPr};

const MAX_TURNS: usize = 30;
const MAX_OUTPUT_TOKENS: i32 = 8192;

pub const SYSTEM_PROMPT: &str = r#"You are a principal engineer reviewing a pull request. Your job is NOT line-level nitpicking (style, ternaries, null coalescing) — tools like Copilot handle that. Your job is to understand the change in relation to the whole system and explain it to a reviewer who lacks full context.

Priorities, strictly in this order:
1. EXTERNAL boundaries: effects on external systems — third-party APIs, other teams' services, queues/topics, webhooks, contracts, data leaving the system. These are always the most important findings.
2. SERVICE boundaries: effects that cross containers/services within the system — API shape changes, new dependencies between services, shared datastore access.
3. INTERNAL structure: module responsibilities, dependency direction, pattern consistency with the rest of the repo. Mention only when material.

Writing style: the summary is a TLDR — two short sentences maximum, no mechanism walkthrough. Everything else goes in the detail field. A reviewer should absorb the summary in three seconds.

Also evaluate the change against the AWS Well-Architected pillars (operational excellence, security, reliability, performance efficiency, cost optimization, sustainability). Report only MATERIAL findings — a missing retry on a new external call matters; a variable name does not.

Method: explore the repository first (README/docs, tree, targeted file reads and searches) until you understand the architecture well enough to place this change in it. Be economical — fetch what you need, not everything.

C4 graph rules:
- Build the graph at the requested C4 level, scoped to AFFECTED elements plus their immediate neighbors. Do not map the whole system.
- Node ids must be deterministic and derived from stable names: "person:<role>", "ext:<system>", "system:<name>", "container:<name>", "component:<path>", "code:<path>#<class>". Lowercase, kebab-case.
- Use `boundary` to nest: containers inside their system, components inside their container.
- Mark every node/edge with its change status. Edges that cross a boundary MUST set crossesBoundary=true.

When you are done exploring, you MUST call submit_analysis exactly once with the complete result. Include EVERY field in the schema — when a list has nothing to report, pass an empty array, never omit the field. Do not produce a final text answer."#;

fn level_instructions(level: AnalysisLevel, focus: Option<&str>) -> String {
    match level {
        AnalysisLevel::Context => "Requested C4 level: CONTEXT + CONTAINER. Show the system in its environment (people, external systems) and the affected containers. This is the default view — keep it at architecture altitude.".into(),
        AnalysisLevel::Component => format!(
            "Requested C4 level: COMPONENT. The user drilled into node '{}'. Anchor everything in the PR diff: deep-dive the components the diff touches, their responsibility shifts, dependency-direction violations, and pattern consistency. Include untouched components only as thin context (mark them unchanged), never as the subject.",
            focus.unwrap_or("(unspecified)")
        ),
        AnalysisLevel::Code => format!(
            "Requested C4 level: CODE. The user drilled into component '{}'. Show the classes/modules the diff changes and what those changes do to coupling, interfaces, and cohesion. Untouched classes appear only when needed to explain an affected relationship. Still architectural framing; no style nits.",
            focus.unwrap_or("(unspecified)")
        ),
    }
}

/// Schema for the terminal submit_analysis tool — mirrors AnalysisResult's
/// graph + assessment (camelCase, as serde serializes them).
fn submit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "graph": {
                "type": "object",
                "properties": {
                    "nodes": {"type": "array", "items": {"type": "object", "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "kind": {"type": "string", "enum": ["person", "external-system", "system", "container", "component", "code", "data-store", "queue"]},
                        "technology": {"type": "string"},
                        "description": {"type": "string"},
                        "boundary": {"type": "string"},
                        "change": {"type": "string", "enum": ["added", "modified", "removed", "affected", "unchanged"]}
                    }, "required": ["id", "name", "kind", "change"]}},
                    "edges": {"type": "array", "items": {"type": "object", "properties": {
                        "id": {"type": "string"},
                        "source": {"type": "string"},
                        "target": {"type": "string"},
                        "label": {"type": "string"},
                        "protocol": {"type": "string"},
                        "crossesBoundary": {"type": "boolean"},
                        "change": {"type": "string", "enum": ["added", "modified", "removed", "affected", "unchanged"]}
                    }, "required": ["id", "source", "target", "label", "crossesBoundary", "change"]}}
                },
                "required": ["nodes", "edges"]
            },
            "assessment": {
                "type": "object",
                "properties": {
                    "summary": {"type": "string", "description": "TLDR: at most TWO short sentences (under 40 words total). What the change is and the single most important thing about it. No mechanism detail here."},
                    "detail": {"type": "string", "description": "The fuller explanation: mechanism, how it flows through the system, why it's safe or risky. 1-2 paragraphs."},
                    "fit": {"type": "string", "enum": ["fits", "tension", "misfit"]},
                    "fitRationale": {"type": "string"},
                    "boundaryImpacts": {"type": "array", "items": {"type": "object", "properties": {
                        "kind": {"type": "string", "enum": ["external", "service", "internal"]},
                        "description": {"type": "string"},
                        "nodeIds": {"type": "array", "items": {"type": "string"}}
                    }, "required": ["kind", "description"]}},
                    "wellArchitected": {"type": "array", "items": {"type": "object", "properties": {
                        "pillar": {"type": "string", "enum": ["operational-excellence", "security", "reliability", "performance-efficiency", "cost-optimization", "sustainability"]},
                        "severity": {"type": "string", "enum": ["info", "low", "medium", "high"]},
                        "finding": {"type": "string"},
                        "recommendation": {"type": "string"},
                        "nodeIds": {"type": "array", "items": {"type": "string"}}
                    }, "required": ["pillar", "severity", "finding", "recommendation"]}},
                    "contextNotes": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["summary", "detail", "fit", "fitRationale", "boundaryImpacts", "wellArchitected", "contextNotes"]
            }
        },
        "required": ["graph", "assessment"]
    })
}

// -- serde_json::Value <-> aws_smithy_types::Document -------------------------

fn value_to_document(v: &Value) -> Document {
    match v {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Document::Number(aws_smithy_types::Number::NegInt(i))
            } else {
                Document::Number(aws_smithy_types::Number::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(a) => Document::Array(a.iter().map(value_to_document).collect()),
        Value::Object(o) => Document::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), value_to_document(v)))
                .collect(),
        ),
    }
}

fn document_to_value(d: &Document) -> Value {
    match d {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(n) => match n {
            aws_smithy_types::Number::PosInt(i) => json!(i),
            aws_smithy_types::Number::NegInt(i) => json!(i),
            aws_smithy_types::Number::Float(f) => json!(f),
        },
        Document::String(s) => Value::String(s.clone()),
        Document::Array(a) => Value::Array(a.iter().map(document_to_value).collect()),
        Document::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), document_to_value(v)))
                .collect(),
        ),
    }
}

// -----------------------------------------------------------------------------

fn tool_config() -> AppResult<ToolConfiguration> {
    let mut builder = ToolConfiguration::builder();
    for (name, description, schema) in RepoTools::specs() {
        builder = builder.tools(Tool::ToolSpec(
            ToolSpecification::builder()
                .name(name)
                .description(description)
                .input_schema(ToolInputSchema::Json(value_to_document(&schema)))
                .build()
                .map_err(|e| AppError::Other(e.to_string()))?,
        ));
    }
    builder = builder.tools(Tool::ToolSpec(
        ToolSpecification::builder()
            .name("submit_analysis")
            .description("Submit the final architecture analysis. Call exactly once, when your exploration is complete.")
            .input_schema(ToolInputSchema::Json(value_to_document(&submit_schema())))
            .build()
            .map_err(|e| AppError::Other(e.to_string()))?,
    ));
    builder.build().map_err(|e| AppError::Other(e.to_string()))
}

fn progress(app: &AppHandle, pr_id: &str, level: AnalysisLevel, message: impl Into<String>) {
    let _ = app.emit(
        events::ANALYSIS_PROGRESS,
        AnalysisProgress { pr_id: pr_id.to_string(), level, message: message.into() },
    );
}

/// Emit progress to the UI and record it in the persistent trace.
fn note(
    app: &AppHandle,
    trace: &mut Vec<TraceStep>,
    pr_id: &str,
    level: AnalysisLevel,
    kind: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    trace.push(TraceStep {
        at: Utc::now().to_rfc3339(),
        kind: kind.to_string(),
        message: message.clone(),
    });
    progress(app, pr_id, level, message);
}

fn describe_tool_call(name: &str, input: &Value) -> String {
    match name {
        "get_pr_diff" => "reading the PR diff".into(),
        "get_file" => format!(
            "reading {}",
            input.get("path").and_then(Value::as_str).unwrap_or("a file")
        ),
        "list_tree" => format!(
            "exploring {}/",
            input.get("path").and_then(Value::as_str).unwrap_or("")
        ),
        "search_code" => format!(
            "searching \"{}\"",
            input.get("query").and_then(Value::as_str).unwrap_or("")
        ),
        "get_readme_and_docs" => "reading README and docs".into(),
        "list_recent_prs" => "checking recent PRs".into(),
        "submit_analysis" => "assembling the assessment".into(),
        other => other.to_string(),
    }
}

pub async fn run(
    app: &AppHandle,
    settings: &Settings,
    token: &str,
    pr: &TrackedPr,
    level: AnalysisLevel,
    focus_node_id: Option<String>,
    parent_context: Option<String>,
) -> AppResult<AnalysisResult> {
    let pr_id = pr.info.id.clone();
    progress(app, &pr_id, level, "connecting to Bedrock");
    devlog::info(
        app,
        "analysis",
        format!(
            "starting {} analysis for {}#{} (head {})",
            level.as_str(),
            pr.info.repo,
            pr.info.number,
            &pr.info.head_sha[..8.min(pr.info.head_sha.len())]
        ),
    );

    let system_prompt = if settings.custom_system_prompt.trim().is_empty() {
        SYSTEM_PROMPT.to_string()
    } else {
        devlog::warn(app, "analysis", "using CUSTOM system prompt from developer settings");
        settings.custom_system_prompt.clone()
    };

    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if !settings.aws_profile.is_empty() {
        loader = loader.profile_name(&settings.aws_profile);
    }
    if !settings.aws_region.is_empty() {
        loader = loader.region(aws_config::Region::new(settings.aws_region.clone()));
    }
    let sdk_config = loader.load().await;
    let mut conf = aws_sdk_bedrockruntime::config::Builder::from(&sdk_config);
    // Only apply a real URL — a pasted ARN here breaks dispatch cryptically.
    if settings.aws_endpoint_url.starts_with("http") {
        conf = conf.endpoint_url(&settings.aws_endpoint_url);
    }
    conf = conf.timeout_config(
        aws_sdk_bedrockruntime::config::timeout::TimeoutConfig::builder()
            .operation_timeout(std::time::Duration::from_secs(300))
            .build(),
    );
    let client = aws_sdk_bedrockruntime::Client::from_conf(conf.build());

    let tools = RepoTools::new(
        &settings.github_graphql_url,
        &pr.info.repo,
        pr.info.number,
        &pr.info.head_sha,
        token,
    )?;

    // Drilled runs inherit the higher-level result so the model doesn't
    // re-derive (and re-fetch) the system map it already built.
    let parent_section = match &parent_context {
        Some(json) => format!(
            "\n\nA higher-level analysis of this PR was already completed. Its result is below — trust it as your system map, do NOT re-explore what it already covers, and keep your node ids consistent with it. Focus your exploration budget on the drill target only.\n<previous_analysis>\n{json}\n</previous_analysis>"
        ),
        None => String::new(),
    };

    let kickoff = format!(
        "Analyze this pull request.\n\nRepository: {}\nPR #{}: {}\nAuthor: {}\nBranch head: {}\nStats: +{} −{} across {} files\nURL: {}\n\n{}{}\n\nStart by getting the diff and whatever repository context you need.",
        pr.info.repo,
        pr.info.number,
        pr.info.title,
        pr.info.author,
        pr.info.head_sha,
        pr.info.additions,
        pr.info.deletions,
        pr.info.changed_files,
        pr.info.url,
        level_instructions(level, focus_node_id.as_deref()),
        parent_section,
    );

    let mut messages = vec![Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(kickoff))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?];

    let config = tool_config()?;
    let mut nudged = false;
    let (mut total_in, mut total_out) = (0i32, 0i32);
    let mut trace: Vec<TraceStep> = Vec::new();
    note(app, &mut trace, &pr_id, level, "status", "starting exploration");

    for turn in 0..MAX_TURNS {
        let started = std::time::Instant::now();
        let resp = client
            .converse()
            .model_id(&settings.bedrock_model_id)
            .system(SystemContentBlock::Text(system_prompt.clone()))
            .set_messages(Some(messages.clone()))
            .tool_config(config.clone())
            .inference_config(
                InferenceConfiguration::builder()
                    .max_tokens(MAX_OUTPUT_TOKENS)
                    .build(),
            )
            .send()
            .await
            .map_err(|e| {
                let detail =
                    format!("{}", aws_smithy_types::error::display::DisplayErrorContext(&e));
                let lower = detail.to_lowercase();
                let hint = if lower.contains("token")
                    || lower.contains("expired")
                    || lower.contains("credential")
                    || lower.contains("sso")
                {
                    format!(
                        " — your SSO session may have expired; run: aws sso login --profile {}",
                        settings.aws_profile
                    )
                } else {
                    String::new()
                };
                AppError::Other(format!("Bedrock: {detail}{hint}"))
            })?;

        if let Some(usage) = resp.usage() {
            total_in += usage.input_tokens();
            total_out += usage.output_tokens();
            devlog::debug(
                app,
                "bedrock",
                format!(
                    "turn {turn}: {}ms, {} in / {} out tokens (total {} / {})",
                    started.elapsed().as_millis(),
                    usage.input_tokens(),
                    usage.output_tokens(),
                    total_in,
                    total_out,
                ),
            );
        }

        let Some(message) = resp.output().and_then(|o| o.as_message().ok().cloned()) else {
            return Err(AppError::Other("Bedrock returned no message".into()));
        };

        let mut tool_results: Vec<ContentBlock> = Vec::new();
        let mut submitted: Option<Value> = None;

        for block in message.content() {
            match block {
                ContentBlock::Text(t) => {
                    let snippet: String = t.chars().take(400).collect();
                    if !snippet.trim().is_empty() {
                        note(app, &mut trace, &pr_id, level, "thought", snippet);
                    }
                }
                ContentBlock::ToolUse(tu) => {
                    let input = document_to_value(tu.input());
                    note(
                        app,
                        &mut trace,
                        &pr_id,
                        level,
                        "tool",
                        describe_tool_call(tu.name(), &input),
                    );
                    if tu.name() == "submit_analysis" {
                        devlog::info(app, "analysis", "model submitted the analysis");
                        submitted = Some(input);
                        continue;
                    }
                    devlog::debug(
                        app,
                        "analysis",
                        format!("tool {}({})", tu.name(), serde_json::to_string(&input).unwrap_or_default()),
                    );
                    let result = tools.execute(tu.name(), &input).await;
                    let (content, is_error) = match result {
                        Ok(text) => (text, false),
                        Err(e) => {
                            devlog::warn(app, "analysis", format!("tool {} failed: {e}", tu.name()));
                            (format!("error: {e}"), true)
                        }
                    };
                    devlog::debug(
                        app,
                        "analysis",
                        format!("tool {} → {} chars", tu.name(), content.len()),
                    );
                    let mut trb = ToolResultBlock::builder()
                        .tool_use_id(tu.tool_use_id())
                        .content(ToolResultContentBlock::Text(content));
                    if is_error {
                        trb = trb.status(aws_sdk_bedrockruntime::types::ToolResultStatus::Error);
                    }
                    tool_results.push(ContentBlock::ToolResult(
                        trb.build().map_err(|e| AppError::Other(e.to_string()))?,
                    ));
                }
                _ => {}
            }
        }

        if let Some(payload) = submitted {
            devlog::info(
                app,
                "analysis",
                format!("complete after {} turns — {total_in} in / {total_out} out tokens", turn + 1),
            );
            note(app, &mut trace, &pr_id, level, "status", "assessment assembled");
            let usage = AnalysisUsage {
                input_tokens: total_in as i64,
                output_tokens: total_out as i64,
                turns: (turn + 1) as i64,
            };
            return parse_submission(pr, level, focus_node_id, payload, trace, usage);
        }

        messages.push(message);

        if !tool_results.is_empty() {
            messages.push(
                Message::builder()
                    .role(ConversationRole::User)
                    .set_content(Some(tool_results))
                    .build()
                    .map_err(|e| AppError::Other(e.to_string()))?,
            );
            continue;
        }

        // Model stopped without calling submit_analysis: nudge once.
        if matches!(resp.stop_reason(), StopReason::EndTurn) {
            if nudged {
                return Err(AppError::Other(
                    "analysis ended without submit_analysis".into(),
                ));
            }
            nudged = true;
            messages.push(
                Message::builder()
                    .role(ConversationRole::User)
                    .content(ContentBlock::Text(
                        "Call submit_analysis now with your complete result.".into(),
                    ))
                    .build()
                    .map_err(|e| AppError::Other(e.to_string()))?,
            );
        }
    }

    Err(AppError::Other(format!(
        "analysis did not complete within {MAX_TURNS} turns"
    )))
}

fn parse_submission(
    pr: &TrackedPr,
    level: AnalysisLevel,
    focus_node_id: Option<String>,
    payload: Value,
    trace: Vec<TraceStep>,
    usage: AnalysisUsage,
) -> AppResult<AnalysisResult> {
    let graph: C4Graph = serde_json::from_value(
        payload
            .get("graph")
            .cloned()
            .ok_or_else(|| AppError::Other("submit_analysis: missing graph".into()))?,
    )
    .map_err(|e| AppError::Other(format!("submit_analysis graph: {e}")))?;
    let mut assessment: Assessment = serde_json::from_value(
        payload
            .get("assessment")
            .cloned()
            .ok_or_else(|| AppError::Other("submit_analysis: missing assessment".into()))?,
    )
    .map_err(|e| AppError::Other(format!("submit_analysis assessment: {e}")))?;

    // Enforce the priority ordering regardless of what the model emitted.
    assessment
        .boundary_impacts
        .sort_by_key(|impact| match impact.kind {
            crate::analysis::types::ImpactKind::External => 0,
            crate::analysis::types::ImpactKind::Service => 1,
            crate::analysis::types::ImpactKind::Internal => 2,
        });

    Ok(AnalysisResult {
        pr_id: pr.info.id.clone(),
        head_sha: pr.info.head_sha.clone(),
        level,
        focus_node_id,
        graph,
        assessment,
        created_at: Utc::now().to_rfc3339(),
        trace,
        usage,
    })
}
