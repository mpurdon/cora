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

/// Input/output dollars per million tokens for a model id, or None when we
/// have no basis for a number.
fn rate_for(model: &str, prices: &[ModelPrice]) -> Option<(f64, f64)> {
    if let Some(p) = prices.iter().find(|p| p.model == model) {
        return Some((p.input_per_mtok, p.output_per_mtok));
    }
    let id = model.to_lowercase();
    KNOWN_RATES
        .iter()
        .find(|(needle, _, _)| id.contains(needle))
        .map(|(_, input, output)| (*input, *output))
}

/// Dollars for one row, and whether we could price it at all.
fn cost_of(row: &UsageRow, prices: &[ModelPrice]) -> Option<f64> {
    let (input, output) = rate_for(&row.model, prices)?;
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
    /// False when neither settings nor the built-in table knows this model.
    pub known: bool,
    /// True when the rate came from the built-in table rather than settings.
    pub built_in: bool,
}

/// Roll rows up into everything the dashboard shows. One pass per grouping,
/// all in memory — see `Store::usage_rows` for why that's the right call.
pub fn summarize(rows: &[UsageRow], prices: &[ModelPrice], now: &str) -> UsageStats {
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
        let cost = cost_of(row, prices);
        stats.all_time.add(row, cost);
        if row.at.as_str() >= cutoff.as_str() {
            stats.last_30_days.add(row, cost);
        }
        if let Some((input_rate, _)) = rate_for(&row.model, prices) {
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
            let from_settings = prices.iter().find(|p| p.model == g.key);
            let resolved = rate_for(&g.key, prices);
            ModelRate {
                model: g.key.clone(),
                input_per_mtok: resolved.map(|r| r.0).unwrap_or(0.0),
                output_per_mtok: resolved.map(|r| r.1).unwrap_or(0.0),
                known: resolved.is_some(),
                built_in: resolved.is_some() && from_settings.is_none(),
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
        assert_eq!(cost_of(&r, &[]), Some(5.0));
    }

    /// An inference-profile ARN names no model — pricing it from a guess
    /// would be worse than admitting we don't know.
    #[test]
    fn leaves_unrecognized_models_unpriced() {
        let r = row("arn:aws:bedrock:us-east-2:1:application-inference-profile/x", "p1", "2026-07-01T00:00:00Z", 1_000_000, 0);
        assert_eq!(cost_of(&r, &[]), None);
        let stats = summarize(&[r], &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.all_time.unpriced_requests, 1);
        assert_eq!(stats.all_time.cost, 0.0);
        assert!(!stats.rates[0].known);
    }

    #[test]
    fn settings_rate_beats_the_built_in_table() {
        let prices = vec![ModelPrice {
            model: "us.anthropic.claude-opus-4-8".into(),
            input_per_mtok: 1.0,
            output_per_mtok: 2.0,
        }];
        let r = row("us.anthropic.claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 1_000_000, 1_000_000);
        assert_eq!(cost_of(&r, &prices), Some(3.0));
        let stats = summarize(&[r], &prices, "2026-07-02T00:00:00Z");
        assert!(!stats.rates[0].built_in, "an explicit rate is not built-in");
    }

    #[test]
    fn averages_over_prs_not_requests() {
        let rows = vec![
            row("claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 1_000_000, 0),
            row("claude-opus-4-8", "p1", "2026-07-01T01:00:00Z", 1_000_000, 0),
            row("claude-opus-4-8", "p2", "2026-07-02T00:00:00Z", 2_000_000, 0),
        ];
        let stats = summarize(&rows, &[], "2026-07-03T00:00:00Z");
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
        let stats = summarize(&rows, &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.all_time.requests, 2);
        assert_eq!(stats.last_30_days.requests, 1);
    }

    #[test]
    fn cache_reads_are_billed_at_a_tenth_and_counted_as_savings() {
        let mut r = row("claude-opus-4-8", "p1", "2026-07-01T00:00:00Z", 0, 0);
        r.cache_read = 1_000_000;
        assert_eq!(cost_of(&r, &[]), Some(0.5)); // $5/M × 0.1
        let stats = summarize(&[r], &[], "2026-07-02T00:00:00Z");
        assert_eq!(stats.cache_savings, 4.5); // the other nine tenths
    }
}
