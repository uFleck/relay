//! Tauri-side conversation directory and explicit search. All cursor, validation and merge
//! policy lives in `relay_core::v2::slack_directory`; this module only chooses a process runner,
//! keeps the CLI off the async thread and publishes quiet progress to both windows.

use std::sync::Arc;

use relay_core::claude_slack::{self as core, ClaudeError, Runner};
use relay_core::model::now;
use relay_core::v2::{DetailedHit, V2Store, scan_directory, search_messages as core_search};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use super::claude::ProcessRunner;
use crate::live::REFERENCE_EVENT;
use crate::slack::find_claude;

/// The picker's progress row listens on this event. No raw connector text is published.
pub const DIRECTORY_EVENT: &str = "relay://slack-directory";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryProgress {
    pub state: String,
    pub conversations: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackDirectoryReport {
    pub conversations: usize,
    pub invocations: usize,
    pub cost_usd: f64,
    pub scanned_at: Option<String>,
    pub error: Option<String>,
    pub coalesced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackSearchReport {
    pub query: String,
    pub hits: Vec<DetailedHit>,
    pub error: Option<String>,
}

struct MissingCli;

impl Runner for MissingCli {
    fn run(&self, _request: &core::RunRequest) -> Result<String, ClaudeError> {
        Err(ClaudeError::NotFound)
    }
}

fn runner(app: &AppHandle) -> Box<dyn Runner + Send> {
    match find_claude() {
        Some(claude) => {
            let cwd = app
                .path()
                .app_data_dir()
                .map(|dir| dir.join("claude-slack"))
                .unwrap_or_else(|_| std::env::temp_dir().join("relay-claude-slack"));
            let _ = std::fs::create_dir_all(&cwd);
            Box::new(ProcessRunner { claude, cwd })
        }
        None => Box::new(MissingCli),
    }
}

fn publish(app: &AppHandle, state: &str, conversations: usize, error: Option<String>) {
    let _ = app.emit(
        DIRECTORY_EVENT,
        DirectoryProgress {
            state: state.to_string(),
            conversations,
            error,
        },
    );
}

async fn run_refresh(app: &AppHandle, store: &Arc<V2Store>) -> SlackDirectoryReport {
    let state = store.directory_state().unwrap_or_default();
    publish(app, "scanning", 0, None);
    let runner = runner(app);
    let store = store.clone();
    let at = now();
    let outcome = tokio::task::spawn_blocking(move || scan_directory(&store, &*runner, &at))
        .await
        .unwrap_or_else(|_| relay_core::v2::DirectoryScanResult {
            error: Some("the directory scan could not start".into()),
            ..Default::default()
        });
    let report = SlackDirectoryReport {
        conversations: outcome.conversations,
        invocations: outcome.invocations,
        cost_usd: outcome.cost_usd,
        scanned_at: outcome.scanned_at.clone(),
        error: outcome.error.clone(),
        coalesced: outcome.coalesced,
    };
    publish(
        app,
        if outcome.error.is_none() {
            "ok"
        } else {
            "error"
        },
        outcome.conversations,
        outcome.error,
    );
    let _ = app.emit(REFERENCE_EVENT, "");
    let _ = app.emit("relay://v2-change", ());
    let _ = state;
    report
}

/// Manual **Refresh conversations** from the picker. The single-flight guard is inside core, so
/// a second press keeps the last directory and returns `coalesced` without another CLI run.
#[tauri::command]
pub async fn v2_slack_refresh(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<SlackDirectoryReport, String> {
    Ok(run_refresh(&app, &store).await)
}

/// One explicit search per Enter press. Typing never calls this command.
#[tauri::command]
pub async fn v2_slack_search(
    query: String,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<SlackSearchReport, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(SlackSearchReport {
            query,
            hits: Vec::new(),
            error: None,
        });
    }
    let runner = runner(&app);
    let store = store.inner().clone();
    let search_query = query.clone();
    let at = now();
    let outcome =
        tokio::task::spawn_blocking(move || core_search(&store, &*runner, &search_query, &at))
            .await
            .unwrap_or_else(|_| relay_core::v2::slack_directory::SearchOutcome {
                error: Some("the search could not start".into()),
                ..Default::default()
            });
    if outcome.error.is_none() {
        let _ = app.emit(REFERENCE_EVENT, "");
    }
    Ok(SlackSearchReport {
        query,
        hits: outcome.hits,
        error: outcome.error,
    })
}

/// Startup trigger: a missing timestamp or one older than seven days, and a Claude binary on
/// PATH. The scan has its own guard, so it never takes the thread gate or blocks window reads.
pub fn scan_if_due(app: &AppHandle) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    if find_claude().is_none() {
        return;
    }
    let ready = store
        .directory_state()
        .map(|state| state.due(&now()))
        .unwrap_or(false);
    if !ready {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_refresh(&app, &store).await;
    });
}

/// Settings-recheck trigger: setup that completed outside Relay publishes a ready Slack status;
/// a first successful scan is then due because the directory has no freshness timestamp yet.
pub fn scan_if_ready(app: &AppHandle, ready: bool) {
    if !ready {
        return;
    }
    scan_if_due(app);
}
