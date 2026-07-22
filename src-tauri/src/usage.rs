//! What the AI has cost: every Bedrock request's tokens, recorded per PR,
//! and the rollups the Developer → Usage tab reads.
//!
//! Tokens are measured — Bedrock reports them. Money is not: this app is
//! usually pointed at an application-inference-profile ARN, which names no
//! model, so nothing here can infer a rate. Prices come from the settings
//! table (seeded from public per-model rates, editable), and anything we
//! can't price is reported as unpriced rather than quietly counted as free.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::ModelPrice;

/// One recorded request.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub at: String,
    pub pr_id: String,
    pub repo: String,
    #[ts(type = "number")]
    pub number: i64,
    pub pr_title: String,
    /// analysis | code-pass | chat
    pub kind: String,
    pub model: String,
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub cache_read: i64,
    #[ts(type = "number")]
    pub cache_write: i64,
}

/// Record one Bedrock request against a PR. Best-effort: a usage row is
/// bookkeeping, never a reason to fail the work that produced it.
pub fn record(
    app: &tauri::AppHandle,
    pr: &crate::models::TrackedPr,
    kind: &str,
    model: &str,
    usage: &aws_sdk_bedrockruntime::types::TokenUsage,
) {
    use tauri::Manager;
    let row = UsageRow {
        at: chrono::Utc::now().to_rfc3339(),
        pr_id: pr.info.id.clone(),
        repo: pr.info.repo.clone(),
        number: pr.info.number,
        pr_title: pr.info.title.clone(),
        kind: kind.to_string(),
        model: model.to_string(),
        input_tokens: usage.input_tokens() as i64,
        output_tokens: usage.output_tokens() as i64,
        cache_read: usage.cache_read_input_tokens().unwrap_or(0) as i64,
        cache_write: usage.cache_write_input_tokens().unwrap_or(0) as i64,
    };
    if let Err(e) = app.state::<crate::orgs::Orgs>().active().add_usage(&row) {
        crate::devlog::warn(app, "usage", format!("could not record token usage: {e}"));
    }
}

/// Cache reads bill at a tenth of the input rate; writing a 5-minute cache
/// entry costs a quarter more than plain input. Both are ratios to the input
/// price, so they hold whatever the per-model rate turns out to be.
const CACHE_READ_RATIO: f64 = 0.1;
const CACHE_WRITE_RATIO: f64 = 1.25;

/// Published per-million-token rates for models we can recognise by id,
/// matched on a substring so Bedrock's `anthropic.` and regional prefixes
/// (`us.anthropic.claude-opus-4-8`) resolve to the same entry. Only a
/// fallback: a settings entry for the exact model id always wins.
const KNOWN_RATES: &[(&str, f64, f64)] = &[
    ("claude-fable-5", 10.0, 50.0),
    ("claude-mythos-5", 10.0, 50.0),
    ("claude-opus-4-8", 5.0, 25.0),
    ("claude-opus-4-7", 5.0, 25.0),
    ("claude-opus-4-6", 5.0, 25.0),
    ("claude-opus-4-5", 5.0, 25.0),
    ("claude-sonnet-5", 3.0, 15.0),
    ("claude-sonnet-4-6", 3.0, 15.0),
    ("claude-sonnet-4-5", 3.0, 15.0),
    ("claude-haiku-4-5", 1.0, 5.0),
];

/// A model id we could only name by looking it up somewhere else — a Bedrock
/// inference-profile ARN mapped back to its family by the Claude Code
/// settings that configured it.
#[derive(Debug, Clone)]
pub struct ModelAlias {
    pub model: String,
    /// Human name for the family: "Claude Opus", "Claude Sonnet"…
    pub family: String,
    /// Where we learned it, for the UI to cite.
    pub source: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Does this value name a model? A Bedrock ARN, or an id carrying the model
/// family in it. Anything else in a settings `env` is not ours to read.
fn is_model_ref(value: &str) -> bool {
    let v = value.to_lowercase();
    (v.starts_with("arn:") && v.contains("bedrock")) || v.contains("claude") || v.contains("anthropic.")
}

/// Published rates per family. Opus 4.6/4.7/4.8 all price the same, as do
/// the current Sonnets, so family is a safe granularity even when the env
/// var doesn't pin a version.
fn family_rate(key: &str) -> Option<(&'static str, f64, f64)> {
    let k = key.to_uppercase();
    if k.contains("OPUS") {
        Some(("Claude Opus", 5.0, 25.0))
    } else if k.contains("SONNET") {
        Some(("Claude Sonnet", 3.0, 15.0))
    } else if k.contains("HAIKU") {
        Some(("Claude Haiku", 1.0, 5.0))
    } else {
        None
    }
}

/// Claude Code's settings files point env vars like ANTHROPIC_DEFAULT_OPUS_MODEL
/// at inference-profile ARNs. That mapping is the only thing that can tell us
/// what an ARN actually is, so read it back: every *.json directly under
/// ~/.claude, any `env` object, any key naming a model.
pub fn claude_settings_aliases() -> Vec<ModelAlias> {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return Vec::new();
    };
    let dir = home.join(".claude");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<ModelAlias> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // Settings files are small; don't slurp something enormous by accident.
        if entry.metadata().map(|m| m.len() > 1_000_000).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let Some(env) = json.get("env").and_then(|e| e.as_object()) else { continue };
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        for (key, value) in env {
            if !key.to_uppercase().contains("MODEL") {
                continue;
            }
            // Neighbouring keys like ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES
            // also say MODEL and OPUS; their values are capability lists, not
            // models. Take only values that look like a model reference.
            let Some(model) = value.as_str().map(str::trim).filter(|v| is_model_ref(v)) else {
                continue;
            };
            let Some((family, input, output)) = family_rate(key) else { continue };
            if out.iter().any(|a| a.model == model) {
                continue;
            }
            out.push(ModelAlias {
                model: model.to_string(),
                family: family.to_string(),
                source: format!("~/.claude/{file}"),
                input_per_mtok: input,
                output_per_mtok: output,
            });
        }
    }
    out
}

/// Where a rate came from. Ordered by authority: what you set wins over what
/// we inferred, which wins over a name we recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum RateSource {
    /// You typed it in the rate editor.
    Yours,
    /// An ARN matched a model env var in your Claude Code settings.
    ClaudeSettings,
    /// The model id itself names a model we have a published rate for.
    Published,
    /// Nothing could price it.
    Unknown,
}

/// Input/output dollars per million tokens for a model id, with where the
/// number came from. None of these is a guess at a model we can't identify —
/// an unrecognised ARN stays Unknown rather than being priced as Opus.
fn rate_for(
    model: &str,
    prices: &[ModelPrice],
    aliases: &[ModelAlias],
) -> (Option<(f64, f64)>, RateSource) {
    if let Some(p) = prices.iter().find(|p| p.model == model) {
        return (Some((p.input_per_mtok, p.output_per_mtok)), RateSource::Yours);
    }
    if let Some(a) = aliases.iter().find(|a| a.model == model) {
        return (
            Some((a.input_per_mtok, a.output_per_mtok)),
            RateSource::ClaudeSettings,
        );
    }
    let id = model.to_lowercase();
    match KNOWN_RATES.iter().find(|(needle, _, _)| id.contains(needle)) {
        Some((_, input, output)) => (Some((*input, *output)), RateSource::Published),
        None => (None, RateSource::Unknown),
    }
}

/// Dollars for one row, and whether we could price it at all.
fn cost_of(row: &UsageRow, prices: &[ModelPrice], aliases: &[ModelAlias]) -> Option<f64> {
    let (input, output) = rate_for(&row.model, prices, aliases).0?;
    let per_token = |tokens: i64, rate: f64| tokens as f64 * rate / 1_000_000.0;
    Some(
        per_token(row.input_tokens, input)
            + per_token(row.output_tokens, output)
            + per_token(row.cache_read, input * CACHE_READ_RATIO)
            + per_token(row.cache_write, input * CACHE_WRITE_RATIO),
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    #[ts(type = "number")]
    pub requests: i64,
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub cache_read: i64,
    #[ts(type = "number")]
    pub cache_write: i64,
    pub cost: f64,
    /// Requests whose model has no rate — their tokens count, their cost doesn't.
    #[ts(type = "number")]
    pub unpriced_requests: i64,
}

impl UsageTotals {
    fn add(&mut self, row: &UsageRow, cost: Option<f64>) {
        self.requests += 1;
        self.input_tokens += row.input_tokens;
        self.output_tokens += row.output_tokens;
        self.cache_read += row.cache_read;
        self.cache_write += row.cache_write;
        match cost {
            Some(c) => self.cost += c,
            None => self.unpriced_requests += 1,
        }
    }
}

/// A rollup with a name: one PR, one day, one model, one kind of work.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub key: String,
    /// Human label — a PR title, a date, a model id.
    pub label: String,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    pub all_time: UsageTotals,
    pub last_30_days: UsageTotals,
    /// Oldest recorded request, empty when nothing has been recorded.
    pub since: String,
    /// PRs by spend, most expensive first.
    pub by_pr: Vec<UsageGroup>,
    pub by_model: Vec<UsageGroup>,
    pub by_kind: Vec<UsageGroup>,
    /// One entry per day with activity, oldest first.
    pub by_day: Vec<UsageGroup>,
    /// All-time spend divided by the number of PRs it touched.
    pub cost_per_pr: f64,
    #[ts(type = "number")]
    pub tokens_per_pr: i64,
    #[ts(type = "number")]
    pub prs: i64,
    /// What the cached prefix would have cost at full input price, minus what
    /// it did cost — the prompt cache's running discount.
    pub cache_savings: f64,
    /// Model ids seen in the data, with the rate in force for each (0 when
    /// unpriced). Drives the rate editor.
    pub rates: Vec<ModelRate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ModelRate {
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    /// False when nothing could price this model.
    pub known: bool,
    pub source: RateSource,
    /// What we identified it as and from where, when we had to look it up:
    /// "Claude Opus · ~/.claude/trajector-settings.json".
    pub note: String,
}

/// Roll rows up into everything the dashboard shows. One pass per grouping,
/// all in memory — see `Store::usage_rows` for why that's the right call.
pub fn summarize(
    rows: &[UsageRow],
    prices: &[ModelPrice],
    aliases: &[ModelAlias],
    now: &str,
) -> UsageStats {
    let mut stats = UsageStats::default();
    if rows.is_empty() {
        return stats;
    }
    // Rows are stored oldest-first, so the first one is the start of history.
    stats.since = rows[0].at.clone();
    let cutoff = thirty_days_before(now);

    let mut by_pr: Vec<UsageGroup> = Vec::new();
    let mut by_model: Vec<UsageGroup> = Vec::new();
    let mut by_kind: Vec<UsageGroup> = Vec::new();
    let mut by_day: Vec<UsageGroup> = Vec::new();

    for row in rows {
        let cost = cost_of(row, prices, aliases);
        stats.all_time.add(row, cost);
        if row.at.as_str() >= cutoff.as_str() {
            stats.last_30_days.add(row, cost);
        }
        if let (Some((input_rate, _)), _) = rate_for(&row.model, prices, aliases) {
            // Cache reads at a tenth of input price — the other nine tenths
            // is the saving.
            stats.cache_savings +=
                row.cache_read as f64 * input_rate * (1.0 - CACHE_READ_RATIO) / 1_000_000.0;
        }

        let pr_label = if row.repo.is_empty() {
            row.pr_title.clone()
        } else {
            format!("{} #{}", row.repo, row.number)
        };
        push(&mut by_pr, &row.pr_id, &pr_label, row, cost);
        push(&mut by_model, &row.model, &row.model.clone(), row, cost);
        push(&mut by_kind, &row.kind, &row.kind.clone(), row, cost);
        let day = row.at.get(..10).unwrap_or_default().to_string();
        push(&mut by_day, &day, &day, row, cost);
    }

    stats.prs = by_pr.len() as i64;
    if stats.prs > 0 {
        stats.cost_per_pr = stats.all_time.cost / stats.prs as f64;
        stats.tokens_per_pr =
            (stats.all_time.input_tokens + stats.all_time.output_tokens) / stats.prs;
    }
    by_pr.sort_by(|a, b| b.totals.cost.total_cmp(&a.totals.cost));
    by_model.sort_by(|a, b| b.totals.cost.total_cmp(&a.totals.cost));
    by_kind.sort_by(|a, b| b.totals.cost.total_cmp(&a.totals.cost));

    stats.rates = by_model
        .iter()
        .map(|g| {
            let (resolved, source) = rate_for(&g.key, prices, aliases);
            let note = match source {
                RateSource::ClaudeSettings => aliases
                    .iter()
                    .find(|a| a.model == g.key)
                    .map(|a| format!("{} · {}", a.family, a.source))
                    .unwrap_or_default(),
                RateSource::Published => "published rate for this model".to_string(),
                RateSource::Yours | RateSource::Unknown => String::new(),
            };
            ModelRate {
                model: g.key.clone(),
                input_per_mtok: resolved.map(|r| r.0).unwrap_or(0.0),
                output_per_mtok: resolved.map(|r| r.1).unwrap_or(0.0),
                known: resolved.is_some(),
                source,
                note,
            }
        })
        .collect();

    stats.by_pr = by_pr;
    stats.by_model = by_model;
    stats.by_kind = by_kind;
    stats.by_day = by_day;
    stats
}

fn push(groups: &mut Vec<UsageGroup>, key: &str, label: &str, row: &UsageRow, cost: Option<f64>) {
    match groups.iter_mut().find(|g| g.key == key) {
        Some(group) => group.totals.add(row, cost),
        None => {
            let mut totals = UsageTotals::default();
            totals.add(row, cost);
            groups.push(UsageGroup { key: key.to_string(), label: label.to_string(), totals });
        }
    }
}

/// RFC3339 timestamps sort lexicographically, so a string cutoff is enough.
fn thirty_days_before(now: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(now)
        .map(|t| (t - chrono::Duration::days(30)).to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(model: &str, pr: &str, at: &str, input: i64, output: i64) -> UsageRow {
        UsageRow {
            at: at.into(),
            pr_id: pr.into(),
            repo: "org/repo".into(),
            number: 1,
            pr_title: "t".into(),
            kind: "analysis".into(),
            model: model.into(),
            input_tokens: input,
            output_tokens: output,
            cache_read: 0,
            cache_write: 0,
        }
    }

    #[test]
    fn prices_known_models_through_bedrock_prefixes() {
        let r = row("us.anthropic.claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 1_000_000, 0);
        assert_eq!(cost_of(&r, &[], &[]), Some(5.0));
    }

    /// An inference-profile ARN names no model — pricing it from a guess
    /// would be worse than admitting we don't know.
    #[test]
    fn leaves_unrecognized_models_unpriced() {
        let r = row("arn:aws:bedrock:us-east-2:1:application-inference-profile/x", "p1", "2026-07-01T00:00:00Z", 1_000_000, 0);
        assert_eq!(cost_of(&r, &[], &[]), None);
        let stats = summarize(&[r], &[], &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.all_time.unpriced_requests, 1);
        assert_eq!(stats.all_time.cost, 0.0);
        assert!(!stats.rates[0].known);
    }

    /// The shape Claude Code writes: env vars naming models, values that are
    /// opaque ARNs. Every key that names a family should resolve, including
    /// the underscore-prefixed spares people keep around.
    #[test]
    fn claude_settings_shape_maps_arns_to_families() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"env": {
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "arn:aws:bedrock:us-east-2:1:application-inference-profile/opus",
                "_ANTHROPIC_DEFAULT_OPUS_MODEL_OPUS_47": "arn:aws:bedrock:us-east-2:1:application-inference-profile/opus47",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "arn:aws:bedrock:us-east-2:1:application-inference-profile/sonnet",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "arn:aws:bedrock:us-east-2:1:application-inference-profile/haiku",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES": "thinking,effort"
            }}"#,
        )
        .unwrap();
        let env = json.get("env").unwrap().as_object().unwrap();
        let mut found: Vec<(&str, f64)> = Vec::new();
        for (key, value) in env {
            if !key.to_uppercase().contains("MODEL") {
                continue;
            }
            let Some(model) = value.as_str().map(str::trim).filter(|v| is_model_ref(v)) else {
                continue;
            };
            if let Some((family, input, _)) = family_rate(key) {
                found.push((family, input));
                assert!(model.starts_with("arn:"), "{model} should be a model reference");
            }
        }
        found.sort_by(|a, b| a.0.cmp(b.0).then(a.1.total_cmp(&b.1)));
        assert_eq!(
            found,
            vec![
                ("Claude Haiku", 1.0),
                ("Claude Opus", 5.0),
                ("Claude Opus", 5.0),
                ("Claude Sonnet", 3.0),
            ],
            "the _SUPPORTED_CAPABILITIES key names OPUS and MODEL but holds no model"
        );
    }

    #[test]
    fn capability_lists_are_not_models() {
        assert!(!is_model_ref("thinking,adaptive_thinking,effort"));
        assert!(!is_model_ref("1"));
        assert!(!is_model_ref(""));
        assert!(is_model_ref(
            "arn:aws:bedrock:us-east-2:1:application-inference-profile/wl4brjw0ulcd"
        ));
        assert!(is_model_ref("us.anthropic.claude-opus-4-8"));
        assert!(is_model_ref("claude-sonnet-5"));
    }

    #[test]
    fn an_alias_prices_an_arn_that_nothing_else_can() {
        let arn = "arn:aws:bedrock:us-east-2:1:application-inference-profile/wl4brjw0ulcd";
        let aliases = vec![ModelAlias {
            model: arn.to_string(),
            family: "Claude Opus".into(),
            source: "~/.claude/settings.json".into(),
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
        }];
        let r = row(arn, "p1", "2026-07-01T00:00:00Z", 1_000_000, 0);
        assert_eq!(cost_of(&r, &[], &[]), None, "unpriced without the mapping");
        assert_eq!(cost_of(&r, &[], &aliases), Some(5.0));

        let stats = summarize(&[r], &[], &aliases, "2026-07-02T00:00:00Z");
        assert_eq!(stats.rates[0].source, RateSource::ClaudeSettings);
        assert!(stats.rates[0].note.contains("Claude Opus"), "the UI can say what it is");
    }

    /// A rate you typed outranks one we inferred from a settings file.
    #[test]
    fn your_rate_outranks_an_alias() {
        let arn = "arn:aws:bedrock:us-east-2:1:application-inference-profile/x";
        let aliases = vec![ModelAlias {
            model: arn.to_string(),
            family: "Claude Opus".into(),
            source: "~/.claude/settings.json".into(),
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
        }];
        let prices = vec![ModelPrice {
            model: arn.to_string(),
            input_per_mtok: 2.0,
            output_per_mtok: 4.0,
        }];
        let r = row(arn, "p1", "2026-07-01T00:00:00Z", 1_000_000, 0);
        assert_eq!(cost_of(&r, &prices, &aliases), Some(2.0));
    }

    #[test]
    fn settings_rate_beats_the_built_in_table() {
        let prices = vec![ModelPrice {
            model: "us.anthropic.claude-opus-4-8".into(),
            input_per_mtok: 1.0,
            output_per_mtok: 2.0,
        }];
        let r = row("us.anthropic.claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 1_000_000, 1_000_000);
        assert_eq!(cost_of(&r, &prices, &[]), Some(3.0));
        let stats = summarize(&[r], &prices, &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.rates[0].source, RateSource::Yours);
    }

    #[test]
    fn averages_over_prs_not_requests() {
        let rows = vec![
            row("claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 1_000_000, 0),
            row("claude-opus-4-8", "p1", "2026-07-01T01:00:00Z", 1_000_000, 0),
            row("claude-opus-4-8", "p2", "2026-07-02T00:00:00Z", 2_000_000, 0),
        ];
        let stats = summarize(&rows, &[], &[], "2026-07-03T00:00:00Z");
        assert_eq!(stats.prs, 2);
        assert_eq!(stats.all_time.requests, 3);
        assert_eq!(stats.cost_per_pr, 10.0); // $20 across two PRs
        assert_eq!(stats.by_pr[0].key, "p1", "most expensive first (tie broken by order)");
        assert_eq!(stats.by_day.len(), 2);
    }

    #[test]
    fn thirty_day_window_excludes_older_rows() {
        let rows = vec![
            row("claude-opus-4-8", "p1", "2026-01-01T00:00:00Z", 1_000_000, 0),
            row("claude-opus-4-8", "p2", "2026-07-01T00:00:00Z", 1_000_000, 0),
        ];
        let stats = summarize(&rows, &[], &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.all_time.requests, 2);
        assert_eq!(stats.last_30_days.requests, 1);
    }

    #[test]
    fn cache_reads_are_billed_at_a_tenth_and_counted_as_savings() {
        let mut r = row("claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 0, 0);
        r.cache_read = 1_000_000;
        assert_eq!(cost_of(&r, &[], &[]), Some(0.5)); // $5/M × 0.1
        let stats = summarize(&[r], &[], &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.cache_savings, 4.5); // the other nine tenths
    }
}
