//! The scout pre-read: a cheap model reads a diff too large to show the
//! architecture pass whole, and hands back a map — which files belong to
//! which feature slice, where the entry points are, which files touch a
//! boundary worth a principal engineer's attention. The main model then
//! spends its exploration budget on the files that matter instead of
//! discovering them one read at a time.

use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::analysis::engine::{converse_once, document_to_value};
use crate::analysis::tools::split_diff;
use crate::devlog;
use crate::error::{AppError, AppResult};
use crate::models::{Settings, TrackedPr};

/// Characters of diff per scout call. Haiku's window is 200k tokens; this
/// keeps each chunk near a quarter of it, with the index and prompt on top.
const CHUNK_CHARS: usize = 200_000;
/// Scout calls per PR. Past this a diff is a generated-code dump, and the
/// index alone tells the story; the remaining files are named as unscouted.
const MAX_CHUNKS: usize = 8;
const MAX_OUTPUT_TOKENS: i32 = 4096;
/// The report's ceiling in the kickoff — a map, not a second diff.
const MAX_REPORT_CHARS: usize = 12_000;

const SCOUT_PROMPT: &str = r#"You are pre-reading one part of a large pull request diff on behalf of a principal engineer who will only have time to read a few of its files. Produce a map, not a review.

1. Cluster the changed files into feature slices: the files that together implement one thing (a new endpoint and its schema, handler, client hook and test are one slice). Name each slice in 2-4 words and say in one sentence what it does.
2. For each slice, name the entry points — the one or two files a reader should open first to understand it.
3. Flag boundary files: anything that touches an external system or third-party API, another service, a queue or topic, a database schema or migration, auth or permissions, money, or a contract other teams depend on. One short note each.
4. Note anything a reviewer would want to know that the file list does not show: generated code, copied files, a pattern repeated many times, a test that was weakened.

Be terse and concrete. Use paths exactly as they appear. Do not evaluate quality. Call submit_scout exactly once with the complete result; do not write a final text answer."#;

fn scout_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "slices": {"type": "array", "items": {"type": "object", "properties": {
                "name": {"type": "string"},
                "purpose": {"type": "string"},
                "files": {"type": "array", "items": {"type": "string"}},
                "entryPoints": {"type": "array", "items": {"type": "string"}}
            }, "required": ["name", "purpose", "files", "entryPoints"]}},
            "boundaries": {"type": "array", "items": {"type": "object", "properties": {
                "file": {"type": "string"},
                "kind": {"type": "string", "enum": ["external", "service", "queue", "data", "auth", "money", "contract"]},
                "note": {"type": "string"}
            }, "required": ["file", "kind", "note"]}},
            "notes": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["slices", "boundaries", "notes"]
    })
}

fn specs() -> Vec<(&'static str, &'static str, Value)> {
    vec![(
        "submit_scout",
        "Submit the map of this chunk. Call exactly once.",
        scout_schema(),
    )]
}

/// One chunk's parsed answer, tolerant of the drift the main engine sees
/// too: a stringified payload, missing lists, unknown kinds.
#[derive(Default)]
struct Chunk {
    slices: Vec<(String, String, Vec<String>, Vec<String>)>,
    boundaries: Vec<(String, String, String)>,
    notes: Vec<String>,
}

fn parse(payload: &Value) -> Chunk {
    let payload = match payload.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        Some(v) => v,
        None => payload.clone(),
    };
    let strs = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::trim).map(String::from)).collect())
            .unwrap_or_default()
    };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let mut out = Chunk::default();
    for s in payload.get("slices").and_then(Value::as_array).into_iter().flatten() {
        let name = text(s, "name");
        if name.is_empty() {
            continue;
        }
        let entry = strs(s.get("entryPoints").or_else(|| s.get("entry_points")));
        out.slices.push((name, text(s, "purpose"), strs(s.get("files")), entry));
    }
    for b in payload.get("boundaries").and_then(Value::as_array).into_iter().flatten() {
        let file = text(b, "file");
        if file.is_empty() {
            continue;
        }
        out.boundaries.push((file, text(b, "kind"), text(b, "note")));
    }
    out.notes = strs(payload.get("notes"));
    out
}

/// Split the diff into per-file slices and pack them into chunks by size.
/// Returns (index lines, hunks) per chunk, plus the files past the cap.
fn chunks(full: &str) -> (Vec<(Vec<String>, String)>, Vec<String>) {
    let files = split_diff(full);
    let mut out: Vec<(Vec<String>, String)> = Vec::new();
    let mut overflow: Vec<String> = Vec::new();
    for (path, text) in files {
        let fits_last = out
            .last()
            .map(|(_, hunks)| hunks.len() + text.len() <= CHUNK_CHARS)
            .unwrap_or(false);
        if !fits_last {
            if out.len() == MAX_CHUNKS {
                overflow.push(path);
                continue;
            }
            out.push((Vec::new(), String::new()));
        }
        let (index, hunks) = out.last_mut().expect("chunk just pushed");
        index.push(path);
        // A single file over the chunk size is cut, not skipped — its head
        // still says what it is.
        let take = text.len().min(CHUNK_CHARS);
        let cut = text.floor_char_boundary(take);
        hunks.push_str(&text[..cut]);
        if cut < text.len() {
            hunks.push_str("\n[file's diff cut here — it exceeds the scout chunk size]\n");
        }
        hunks.push('\n');
    }
    (out, overflow)
}

async fn scout_chunk(
    app: &AppHandle,
    settings: &Settings,
    client: &aws_sdk_bedrockruntime::Client,
    pr: &TrackedPr,
    n: usize,
    total: usize,
    index: &[String],
    hunks: &str,
) -> AppResult<Chunk> {
    let kickoff = format!(
        "Pull request {} #{}: {}\nChunk {} of {}. Files in this chunk:\n{}\n\n## Diff\n{}",
        pr.info.repo,
        pr.info.number,
        pr.info.title,
        n + 1,
        total,
        index.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"),
        hunks,
    );
    let messages = vec![Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(kickoff))
        .build()
        .map_err(|e| AppError::Other(e.to_string()))?];
    // One shot — no cache to warm, and a chunk is never sent twice.
    let mut use_cache = false;
    let resp = converse_once(
        app,
        "scout",
        client,
        &settings.bedrock_scout_model_id,
        SCOUT_PROMPT,
        &messages,
        &specs(),
        MAX_OUTPUT_TOKENS,
        &mut use_cache,
        &settings.aws_profile,
    )
    .await?;
    if let Some(usage) = resp.usage() {
        crate::usage::record(app, pr, "scout", &settings.bedrock_scout_model_id, usage);
    }
    let message = resp
        .output()
        .and_then(|o| o.as_message().ok().cloned())
        .ok_or_else(|| AppError::Other("scout returned no message".into()))?;
    for block in message.content() {
        if let ContentBlock::ToolUse(tu) = block {
            if tu.name() == "submit_scout" {
                return Ok(parse(&document_to_value(tu.input())));
            }
        }
    }
    // No tool call: whatever it wrote is still a note worth carrying.
    let text: String = message
        .content()
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.trim()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return Err(AppError::Other("scout submitted nothing".into()));
    }
    Ok(Chunk { notes: vec![text.chars().take(2000).collect()], ..Chunk::default() })
}

/// The scout report as a kickoff section, or an error when nothing usable
/// came back. Chunks run concurrently; one failed chunk is dropped with a
/// note rather than failing the whole pre-read.
pub async fn report(
    app: &AppHandle,
    settings: &Settings,
    client: &aws_sdk_bedrockruntime::Client,
    pr: &TrackedPr,
    full_diff: &str,
) -> AppResult<String> {
    let (parts, overflow) = chunks(full_diff);
    if parts.is_empty() {
        return Err(AppError::Other("nothing to scout".into()));
    }
    let total = parts.len();
    devlog::info(
        app,
        "scout",
        format!(
            "pre-reading {} chars of diff in {total} chunk(s) on {}",
            full_diff.len(),
            settings.bedrock_scout_model_id
        ),
    );
    let started = std::time::Instant::now();
    let results = futures::future::join_all(parts.iter().enumerate().map(|(n, (index, hunks))| {
        scout_chunk(app, settings, client, pr, n, total, index, hunks)
    }))
    .await;

    let mut merged = Chunk::default();
    let mut failed = 0usize;
    for r in results {
        match r {
            Ok(c) => {
                merged.slices.extend(c.slices);
                merged.boundaries.extend(c.boundaries);
                merged.notes.extend(c.notes);
            }
            Err(e) => {
                failed += 1;
                devlog::warn(app, "scout", format!("chunk failed: {e}"));
            }
        }
    }
    if failed == total {
        return Err(AppError::Other("every scout chunk failed".into()));
    }
    devlog::info(
        app,
        "scout",
        format!(
            "done in {}ms — {} slices, {} boundary flags, {} notes{}",
            started.elapsed().as_millis(),
            merged.slices.len(),
            merged.boundaries.len(),
            merged.notes.len(),
            if failed > 0 { format!(", {failed} chunk(s) failed") } else { String::new() }
        ),
    );

    let mut out = String::from(
        "\n\n## Scout report\nA cheaper model pre-read the whole diff and mapped it. Use this to choose what to read — verify anything you rely on; it is a map, not a review.\n",
    );
    if !merged.slices.is_empty() {
        out.push_str("\n### Feature slices\n");
        for (name, purpose, files, entry) in &merged.slices {
            out.push_str(&format!("- **{name}** — {purpose}\n"));
            if !entry.is_empty() {
                out.push_str(&format!("  entry points: {}\n", entry.join(", ")));
            }
            if !files.is_empty() {
                out.push_str(&format!("  files: {}\n", files.join(", ")));
            }
        }
    }
    if !merged.boundaries.is_empty() {
        out.push_str("\n### Boundary flags\n");
        for (file, kind, note) in &merged.boundaries {
            out.push_str(&format!("- {file} [{kind}] {note}\n"));
        }
    }
    if !merged.notes.is_empty() {
        out.push_str("\n### Notes\n");
        for n in &merged.notes {
            out.push_str(&format!("- {n}\n"));
        }
    }
    if !overflow.is_empty() {
        out.push_str(&format!(
            "\n{} files past the scouted range were not pre-read (they are still in the diff index): {}\n",
            overflow.len(),
            overflow.iter().take(30).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if failed > 0 {
        out.push_str(&format!("\n{failed} of {total} chunks could not be scouted; their files appear only in the index.\n"));
    }
    if out.len() > MAX_REPORT_CHARS {
        let cut = out.floor_char_boundary(MAX_REPORT_CHARS);
        out.truncate(cut);
        out.push_str("\n[scout report truncated]\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_pack_files_by_size_and_cap_the_count() {
        let file = |i: usize, size: usize| {
            format!("diff --git a/f{i}.ts b/f{i}.ts\n--- a/f{i}.ts\n+++ b/f{i}.ts\n{}\n", "+x".repeat(size / 2))
        };
        let mut diff = String::new();
        // Three of these fit a chunk with their headers; a fourth does not.
        for i in 0..40 {
            diff.push_str(&file(i, CHUNK_CHARS / 4));
        }
        let (parts, overflow) = chunks(&diff);
        assert_eq!(parts.len(), MAX_CHUNKS, "packs to the cap");
        assert!(parts.iter().all(|(idx, _)| idx.len() == 3), "three files fit a chunk: {:?}", parts.iter().map(|(i, _)| i.len()).collect::<Vec<_>>());
        assert_eq!(overflow.len(), 40 - MAX_CHUNKS * 3, "the rest are named, not dropped");
        assert!(parts[0].1.contains("diff --git a/f0.ts"));
    }

    #[test]
    fn parse_tolerates_stringified_payloads_and_snake_case() {
        let payload = json!({
            "slices": [{"name": "Bookings API", "purpose": "adds the endpoint", "files": ["a.ts"], "entry_points": ["a.ts"]}],
            "boundaries": [{"file": "a.ts", "kind": "external", "note": "calls the partner"}],
            "notes": ["generated client"]
        });
        let c = parse(&Value::String(payload.to_string()));
        assert_eq!(c.slices.len(), 1);
        assert_eq!(c.slices[0].3, vec!["a.ts".to_string()], "entry_points read as entryPoints");
        assert_eq!(c.boundaries[0].1, "external");
        assert_eq!(c.notes, vec!["generated client".to_string()]);
    }
}
