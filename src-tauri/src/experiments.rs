//! The lab: run the same PRs under different AI configurations and keep the
//! results side by side. A variant is a captured AI configuration; a run is
//! one PR analyzed under one variant, with its timing, tokens, cost, and the
//! full result. Runs never touch the analyses the review screen shows, and
//! every run is a fresh read — no prior-head seeding — so variants compare
//! like for like.

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::analysis::types::{AnalysisLevel, AnalysisResult};
use crate::error::{AppError, AppResult};
use crate::models::Settings;
use crate::store::Store;

pub mod events {
    /// The whole experiment, after any change — a run landing, a variant
    /// added. Coarse on purpose: the pane re-renders from one payload.
    pub const CHANGED: &str = "experiment:changed";
    /// Progress lines from a run inside an experiment. Same payload as
    /// analysis:progress, on its own channel so the review screen does not
    /// mistake lab runs for its own.
    pub const PROGRESS: &str = "experiment:progress";
}

/// Everything in Settings that shapes what an analysis produces. Not the
/// credentials, not the polling — the parts an experiment varies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub bedrock_model_id: String,
    pub bedrock_drill_model_id: String,
    pub bedrock_chat_model_id: String,
    pub bedrock_scout_model_id: String,
    pub bedrock_effort_arch: String,
    pub bedrock_effort_drill: String,
    pub bedrock_effort_code: String,
    #[ts(type = "number")]
    pub arch_max_output_tokens: u64,
    #[ts(type = "number")]
    pub code_max_output_tokens: u64,
    pub route_routine_prs_to_drill_model: bool,
    pub code_findings_pass: bool,
    pub custom_system_prompt: String,
    pub review_conventions: String,
}

impl AiConfig {
    pub fn capture(s: &Settings) -> Self {
        Self {
            bedrock_model_id: s.bedrock_model_id.clone(),
            bedrock_drill_model_id: s.bedrock_drill_model_id.clone(),
            bedrock_chat_model_id: s.bedrock_chat_model_id.clone(),
            bedrock_scout_model_id: s.bedrock_scout_model_id.clone(),
            bedrock_effort_arch: s.bedrock_effort_arch.clone(),
            bedrock_effort_drill: s.bedrock_effort_drill.clone(),
            bedrock_effort_code: s.bedrock_effort_code.clone(),
            arch_max_output_tokens: s.arch_max_output_tokens,
            code_max_output_tokens: s.code_max_output_tokens,
            route_routine_prs_to_drill_model: s.route_routine_prs_to_drill_model,
            code_findings_pass: s.code_findings_pass,
            custom_system_prompt: s.custom_system_prompt.clone(),
            review_conventions: s.review_conventions.clone(),
        }
    }

    pub fn apply(&self, s: &mut Settings) {
        s.bedrock_model_id = self.bedrock_model_id.clone();
        s.bedrock_drill_model_id = self.bedrock_drill_model_id.clone();
        s.bedrock_chat_model_id = self.bedrock_chat_model_id.clone();
        s.bedrock_scout_model_id = self.bedrock_scout_model_id.clone();
        s.bedrock_effort_arch = self.bedrock_effort_arch.clone();
        s.bedrock_effort_drill = self.bedrock_effort_drill.clone();
        s.bedrock_effort_code = self.bedrock_effort_code.clone();
        s.arch_max_output_tokens = self.arch_max_output_tokens;
        s.code_max_output_tokens = self.code_max_output_tokens;
        s.route_routine_prs_to_drill_model = self.route_routine_prs_to_drill_model;
        s.code_findings_pass = self.code_findings_pass;
        s.custom_system_prompt = self.custom_system_prompt.clone();
        s.review_conventions = self.review_conventions.clone();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub config: AiConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Queued,
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub variant_id: String,
    pub pr_id: String,
    pub status: RunStatus,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Wall clock of the whole run, both passes, queue to result.
    #[ts(type = "number")]
    pub elapsed_ms: i64,
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    /// Dollars from the usage rows the run recorded; None when a model in
    /// the run has no rate.
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
    /// The result without its trace — the pane compares outcomes, not steps.
    pub result: Option<AnalysisResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Experiment {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// The bench: tracked PR ids, in the order they were added.
    pub pr_ids: Vec<String>,
    pub variants: Vec<Variant>,
    pub runs: Vec<RunRecord>,
    /// Why the last queue stopped early, if it did — shown until the next
    /// queue starts. A queue that runs to the end leaves this empty.
    #[serde(default)]
    pub notice: Option<String>,
}

/// A failure that will repeat on every run until something outside the
/// experiment changes — expired credentials, an endpoint being held. The
/// queue stops on these instead of collecting one red cell per pair.
fn is_blocking(error: &str) -> bool {
    use crate::analysis::types::{classify_error, AnalysisErrorKind};
    let lower = error.to_lowercase();
    matches!(classify_error(error), AnalysisErrorKind::AwsAuth | AnalysisErrorKind::GithubAuth)
        || lower.contains("unreachable")
        || lower.contains("holding")
}

fn friendly(error: &str) -> String {
    use crate::analysis::types::{classify_error, AnalysisErrorKind};
    match classify_error(error) {
        AnalysisErrorKind::AwsAuth => "AWS session expired — sign in from the AWS pane, then run again".into(),
        AnalysisErrorKind::GithubAuth => "GitHub token missing or rejected — check the GitHub pane".into(),
        AnalysisErrorKind::Other if error.to_lowercase().contains("unreachable") || error.to_lowercase().contains("holding") => {
            "Bedrock is unreachable — check the endpoint or VPN, then run again".into()
        }
        AnalysisErrorKind::Other => error.lines().next().unwrap_or(error).to_string(),
    }
}

/// Drop this queue's untouched records so their cells read "—" again and a
/// plain Run picks them up, no force needed.
fn unqueue(exp: &mut Experiment, pairs: &[(String, String)]) {
    exp.runs.retain(|r| {
        !(r.status == RunStatus::Queued && pairs.iter().any(|(v, p)| v == &r.variant_id && p == &r.pr_id))
    });
}

fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "{prefix}-{:x}-{:x}",
        Utc::now().timestamp_millis(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn load(store: &Store, id: &str) -> AppResult<Experiment> {
    store
        .get_experiment(id)?
        .ok_or_else(|| AppError::Other("experiment not found".into()))
}

fn persist(app: &AppHandle, store: &Store, exp: &Experiment) -> AppResult<()> {
    store.put_experiment(exp)?;
    let _ = app.emit(events::CHANGED, exp);
    Ok(())
}

fn upsert_run(exp: &mut Experiment, run: RunRecord) {
    match exp
        .runs
        .iter_mut()
        .find(|r| r.variant_id == run.variant_id && r.pr_id == run.pr_id)
    {
        Some(slot) => *slot = run,
        None => exp.runs.push(run),
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn list_experiments(orgs: State<'_, crate::orgs::Orgs>) -> AppResult<Vec<Experiment>> {
    orgs.active().list_experiments()
}

#[tauri::command]
pub fn current_ai_config(orgs: State<'_, crate::orgs::Orgs>) -> AppResult<AiConfig> {
    Ok(AiConfig::capture(&orgs.active().settings()?))
}

#[tauri::command]
pub fn create_experiment(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    name: String,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let exp = Experiment {
        id: new_id("exp"),
        name: if name.trim().is_empty() { "Untitled experiment".into() } else { name.trim().into() },
        created_at: Utc::now().to_rfc3339(),
        pr_ids: Vec::new(),
        variants: Vec::new(),
        runs: Vec::new(),
        notice: None,
    };
    persist(&app, &store, &exp)?;
    Ok(exp)
}

#[tauri::command]
pub fn rename_experiment(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    name: String,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let mut exp = load(&store, &id)?;
    if !name.trim().is_empty() {
        exp.name = name.trim().into();
    }
    persist(&app, &store, &exp)?;
    Ok(exp)
}

#[tauri::command]
pub fn delete_experiment(orgs: State<'_, crate::orgs::Orgs>, id: String) -> AppResult<()> {
    orgs.active().delete_experiment(&id)
}

#[tauri::command]
pub fn set_experiment_prs(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    pr_ids: Vec<String>,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let mut exp = load(&store, &id)?;
    exp.runs.retain(|r| pr_ids.contains(&r.pr_id));
    exp.pr_ids = pr_ids;
    persist(&app, &store, &exp)?;
    Ok(exp)
}

/// A new variant: the given configuration, or the live AI settings when
/// none is passed.
#[tauri::command]
pub fn add_variant(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    name: String,
    config: Option<AiConfig>,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let mut exp = load(&store, &id)?;
    let n = exp.variants.len() + 1;
    let config = match config {
        Some(c) => c,
        None => AiConfig::capture(&store.settings()?),
    };
    exp.variants.push(Variant {
        id: new_id("var"),
        name: if name.trim().is_empty() { format!("Variant {n}") } else { name.trim().into() },
        created_at: Utc::now().to_rfc3339(),
        config,
    });
    persist(&app, &store, &exp)?;
    Ok(exp)
}

/// Edit a variant. A changed configuration retires its runs — results from
/// a configuration that no longer exists would only mislead.
#[tauri::command]
pub fn update_variant(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    variant_id: String,
    name: String,
    config: AiConfig,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let mut exp = load(&store, &id)?;
    if exp
        .runs
        .iter()
        .any(|r| r.variant_id == variant_id && matches!(r.status, RunStatus::Running | RunStatus::Queued))
    {
        return Err(AppError::Other("this variant is running — wait for it to finish".into()));
    }
    let Some(v) = exp.variants.iter_mut().find(|v| v.id == variant_id) else {
        return Err(AppError::Other("variant not found".into()));
    };
    if !name.trim().is_empty() {
        v.name = name.trim().into();
    }
    let changed = v.config != config;
    v.config = config;
    if changed {
        exp.runs.retain(|r| r.variant_id != variant_id);
    }
    persist(&app, &store, &exp)?;
    Ok(exp)
}

#[tauri::command]
pub fn rename_variant(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    variant_id: String,
    name: String,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let mut exp = load(&store, &id)?;
    if let Some(v) = exp.variants.iter_mut().find(|v| v.id == variant_id) {
        if !name.trim().is_empty() {
            v.name = name.trim().into();
        }
    }
    persist(&app, &store, &exp)?;
    Ok(exp)
}

#[tauri::command]
pub fn remove_variant(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    variant_id: String,
) -> AppResult<Experiment> {
    let store = orgs.active();
    let mut exp = load(&store, &id)?;
    exp.variants.retain(|v| v.id != variant_id);
    exp.runs.retain(|r| r.variant_id != variant_id);
    persist(&app, &store, &exp)?;
    Ok(exp)
}

/// Make a variant the live configuration.
#[tauri::command]
pub fn apply_variant(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    variant_id: String,
) -> AppResult<Settings> {
    let store = orgs.active();
    let exp = load(&store, &id)?;
    let variant = exp
        .variants
        .iter()
        .find(|v| v.id == variant_id)
        .ok_or_else(|| AppError::Other("variant not found".into()))?;
    let mut settings = store.settings()?;
    variant.config.apply(&mut settings);
    store.save_settings(&settings)?;
    crate::analysis::chat::invalidate_all_contexts(&app);
    Ok(settings)
}

/// Run one variant over the bench — the PRs without a good result, or all
/// of them when `rerun` is set.
#[tauri::command]
pub fn run_variant(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    variant_id: String,
    rerun: bool,
) -> AppResult<()> {
    let store = orgs.active();
    let exp = load(&store, &id)?;
    if exp.variants.iter().all(|v| v.id != variant_id) {
        return Err(AppError::Other("variant not found".into()));
    }
    start_queue(app, store, exp, Some(variant_id), rerun)
}

/// Run every variant over the bench, one PR at a time across all of them.
#[tauri::command]
pub fn run_experiment(
    app: AppHandle,
    orgs: State<'_, crate::orgs::Orgs>,
    id: String,
    rerun: bool,
) -> AppResult<()> {
    let store = orgs.active();
    let exp = load(&store, &id)?;
    start_queue(app, store, exp, None, rerun)
}

/// Queue (variant, PR) pairs and run them sequentially, so each wall-clock
/// number is the run alone on the wire. One queue per experiment at a time.
fn start_queue(
    app: AppHandle,
    store: Arc<Store>,
    mut exp: Experiment,
    only_variant: Option<String>,
    rerun: bool,
) -> AppResult<()> {
    if exp
        .runs
        .iter()
        .any(|r| matches!(r.status, RunStatus::Running | RunStatus::Queued))
    {
        return Err(AppError::Other("this experiment is already running — wait for the queue to finish".into()));
    }
    let variant_ids: Vec<String> = exp
        .variants
        .iter()
        .map(|v| v.id.clone())
        .filter(|vid| only_variant.as_ref().is_none_or(|only| only == vid))
        .collect();
    let mut pending: Vec<(String, String)> = Vec::new();
    for vid in &variant_ids {
        for pr_id in &exp.pr_ids {
            let done = exp
                .runs
                .iter()
                .any(|r| &r.variant_id == vid && &r.pr_id == pr_id && r.status == RunStatus::Ok);
            if rerun || !done {
                pending.push((vid.clone(), pr_id.clone()));
            }
        }
    }
    if pending.is_empty() {
        return Err(AppError::Other(
            "nothing to run — every PR already has a result; turn on force to re-run".into(),
        ));
    }
    exp.notice = None;
    let now = Utc::now().to_rfc3339();
    for (vid, pr_id) in &pending {
        upsert_run(&mut exp, RunRecord {
            variant_id: vid.clone(),
            pr_id: pr_id.clone(),
            status: RunStatus::Queued,
            started_at: now.clone(),
            finished_at: None,
            elapsed_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
            error: None,
            result: None,
        });
    }
    persist(&app, &store, &exp)?;
    let id = exp.id.clone();
    tauri::async_runtime::spawn(run_queue(app, store, id, pending));
    Ok(())
}

async fn run_queue(app: AppHandle, store: Arc<Store>, id: String, pending: Vec<(String, String)>) {
    // Credentials first: an expired SSO session would fail every pair the
    // same way, and each failure costs a Bedrock round trip to learn.
    if let Ok(settings) = store.settings() {
        if let Err(e) = crate::commands::check_aws(settings.aws_profile.clone(), settings.aws_region.clone()).await {
            if let Ok(Some(mut exp)) = store.get_experiment(&id) {
                unqueue(&mut exp, &pending);
                exp.notice = Some(format!(
                    "Nothing was run: {}. ({})",
                    friendly(&e.to_string()),
                    e.to_string().lines().next().unwrap_or("")
                ));
                let _ = persist(&app, &store, &exp);
            }
            return;
        }
    }
    let total = pending.len();
    for (done, (variant_id, pr_id)) in pending.clone().into_iter().enumerate() {
        // Re-read every time: the bench or the variant may have changed
        // under us; a removed variant or PR just skips its pairs.
        let Ok(Some(mut exp)) = store.get_experiment(&id) else { return };
        let Some(variant) = exp.variants.iter().find(|v| v.id == variant_id).cloned() else {
            continue;
        };
        if !exp.pr_ids.contains(&pr_id) {
            continue;
        }
        let started_at = Utc::now().to_rfc3339();
        let started = std::time::Instant::now();
        let mut record = RunRecord {
            variant_id: variant_id.clone(),
            pr_id: pr_id.clone(),
            status: RunStatus::Running,
            started_at: started_at.clone(),
            finished_at: None,
            elapsed_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
            error: None,
            result: None,
        };

        let pr = match store.get_pr(&pr_id) {
            Ok(Some(pr)) => pr,
            _ => {
                record.status = RunStatus::Failed;
                record.error = Some("PR is no longer tracked".into());
                record.finished_at = Some(Utc::now().to_rfc3339());
                upsert_run(&mut exp, record);
                let _ = persist(&app, &store, &exp);
                continue;
            }
        };
        // The review screen must not analyze the same PR at the same time —
        // the two would race on Bedrock and the usage rows would blur.
        let key = crate::commands::analysis_key(&pr_id, AnalysisLevel::Context, &None);
        {
            let runs = app.state::<crate::commands::AnalysisRuns>();
            if !runs.0.lock().unwrap().insert(key.clone()) {
                record.status = RunStatus::Failed;
                record.error = Some("this PR is being analyzed from the review screen — try again when it finishes".into());
                record.finished_at = Some(Utc::now().to_rfc3339());
                upsert_run(&mut exp, record);
                let _ = persist(&app, &store, &exp);
                continue;
            }
        }
        upsert_run(&mut exp, record.clone());
        let _ = persist(&app, &store, &exp);

        let settings = match store.settings() {
            Ok(mut s) => {
                variant.config.apply(&mut s);
                s
            }
            Err(e) => {
                app.state::<crate::commands::AnalysisRuns>().0.lock().unwrap().remove(&key);
                record.status = RunStatus::Failed;
                record.error = Some(e.to_string());
                upsert_run(&mut exp, record);
                let _ = persist(&app, &store, &exp);
                continue;
            }
        };
        let outcome = crate::analysis::engine::EXPERIMENT_SCOPE
            .scope(
                true,
                crate::commands::analyze_pr(
                    &app,
                    &pr,
                    &settings,
                    AnalysisLevel::Context,
                    None,
                    None,
                    None,
                ),
            )
            .await;
        app.state::<crate::commands::AnalysisRuns>().0.lock().unwrap().remove(&key);

        let finished_at = Utc::now().to_rfc3339();
        record.finished_at = Some(finished_at.clone());
        record.elapsed_ms = started.elapsed().as_millis() as i64;
        let mut stop: Option<String> = None;
        match outcome {
            Ok(mut result) => {
                result.trace.clear();
                record.input_tokens = result.usage.input_tokens;
                record.output_tokens = result.usage.output_tokens;
                record.cost_usd = cost_between(&store, &settings, &pr_id, &started_at, &finished_at);
                record.status = RunStatus::Ok;
                record.result = Some(result);
            }
            Err(e) => {
                let raw = e.to_string();
                record.status = RunStatus::Failed;
                record.error = Some(format!("{}\n{raw}", friendly(&raw)));
                if is_blocking(&raw) {
                    let left = total - done - 1;
                    stop = Some(format!(
                        "Stopped after {} of {total}: {}. {} run{} not attempted — they will run again with a plain Run.",
                        done + 1,
                        friendly(&raw),
                        left,
                        if left == 1 { " was" } else { "s were" }
                    ));
                }
            }
        }
        // The experiment may have changed while the run was in flight.
        let Ok(Some(mut exp)) = store.get_experiment(&id) else { return };
        if exp.variants.iter().any(|v| v.id == variant_id) {
            upsert_run(&mut exp, record);
        }
        if let Some(notice) = stop {
            unqueue(&mut exp, &pending);
            exp.notice = Some(notice);
            let _ = persist(&app, &store, &exp);
            return;
        }
        let _ = persist(&app, &store, &exp);
    }
}

/// Dollars for the usage rows this run recorded — both passes, every
/// request. None when any model in the window has no rate.
fn cost_between(store: &Store, settings: &Settings, pr_id: &str, from: &str, to: &str) -> Option<f64> {
    let rows = store.usage_rows_between(pr_id, from, to).ok()?;
    if rows.is_empty() {
        return None;
    }
    let aliases = crate::usage::claude_settings_aliases();
    let mut total = 0.0;
    for row in &rows {
        total += crate::usage::cost_of(row, &settings.model_prices, &aliases)?;
    }
    Some(total)
}
