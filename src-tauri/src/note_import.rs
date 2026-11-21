//! v2 create-only Note Import (R11). Discovery uses the verified ClickUp `assigned_tasks` and
//! GitHub `search_my_pulls` reads through the same credentials as v2 Settings; live state uses
//! #30's exact reads on an independent schedule. Both surfaces commit through `V2Store`, and
//! neither creates a Task, calls the one-shot importer or reanalyses Slack.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use relay_core::clickup::ClickUpClient;
use relay_core::github::GithubClient;
use relay_core::model::now;
use relay_core::provider::ProviderFailure;
use relay_core::v2::{
    ExactItem, ImportCandidate, ImportOutcome, MAX_IN_FLIGHT, ReferenceKey, ReferenceSnapshot,
    V2ImportScope, V2Store, clickup_imported_state, github_imported_state, read_exact_item,
    refresh_failure_update, refresh_snapshot, refresh_success_update,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::CommandError;
use crate::{github, live, secrets};

const CLICKUP_KEY: &str = "clickup-token";
const GITHUB_KEY: &str = "github-token";
const CLICKUP_START: Duration = Duration::from_secs(30);
const GITHUB_START: Duration = Duration::from_secs(20);
const REFRESH_TICK: Duration = Duration::from_secs(5);
/// The signal a committed Import write shares with the Notebook and picker.
pub const IMPORT_EVENT: &str = "relay://v2-change";
pub const REFERENCE_EVENT: &str = "relay://reference-change";

/// One enabled source's contribution to a run.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRun {
    pub created: usize,
    pub skipped: usize,
    pub truncated: bool,
}

/// The manual **Import now** result shown in v2 Settings.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRunResult {
    pub created: usize,
    pub skipped: usize,
    pub truncated: bool,
    pub clickup_error: Option<String>,
    pub github_error: Option<String>,
}

fn merge(result: &mut ImportRunResult, run: SourceRun) {
    result.created += run.created;
    result.skipped += run.skipped;
    result.truncated |= run.truncated;
}

/// Commits candidates one canonical key at a time. One bad item cannot stop the other targets.
fn commit(store: &V2Store, candidates: Vec<ImportCandidate>) -> SourceRun {
    let mut run = SourceRun::default();
    for candidate in candidates {
        match store.import_note(&candidate) {
            Ok(ImportOutcome::Created { .. }) => run.created += 1,
            Ok(ImportOutcome::AlreadyImported) | Err(_) => run.skipped += 1,
        }
    }
    run
}

pub async fn run_clickup(app: &AppHandle, store: &V2Store) -> Result<SourceRun, String> {
    let settings = store.settings().map_err(|error| error.to_string())?;
    if !settings.import_clickup_enabled {
        return Ok(SourceRun::default());
    }
    let raw = settings
        .import_scope
        .as_deref()
        .ok_or("Choose a ClickUp workspace and project in Settings first.")?;
    let scope = V2ImportScope::from_json(raw).ok_or("The saved Import scope is malformed.")?;
    let token = secrets::v2::read(CLICKUP_KEY).ok_or("ClickUp is not connected.")?;
    let client =
        ClickUpClient::new(token).map_err(|error| error.failure().reason_text().to_string())?;
    let me = client
        .me()
        .await
        .map_err(|error| error.failure().reason_text().to_string())?;
    let assigned = client
        .assigned_tasks(&scope.workspace_id, &me, scope.space_id.as_deref())
        .await
        .map_err(|error| error.failure().reason_text().to_string())?;
    let candidates = relay_core::clickup_sync::import_candidates(&assigned, &me, &now());
    let mut run = commit(store, candidates);
    run.truncated = assigned.truncated;
    if run.created > 0 {
        let _ = app.emit(IMPORT_EVENT, ());
    }
    Ok(run)
}

pub async fn run_github(app: &AppHandle, store: &V2Store) -> Result<SourceRun, String> {
    let settings = store.settings().map_err(|error| error.to_string())?;
    if !settings.import_github_enabled {
        return Ok(SourceRun::default());
    }
    // GitHub discovery stays behind the signed-in identity: a stored v2 token first, then the
    // external `gh` session. Public access never drives Import.
    let token = match secrets::v2::read(GITHUB_KEY) {
        Some(token) => Some(token),
        None => github::session_token().await,
    };
    if token.is_none() {
        return Err("GitHub is not signed in. Add a token or sign in with gh.".into());
    }
    let client =
        GithubClient::new(token).map_err(|error| error.failure().reason_text().to_string())?;
    let found = client
        .search_my_pulls()
        .await
        .map_err(|error| error.failure().reason_text().to_string())?;
    let registered = store
        .list_projects()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|project| project.owner_repo)
        .collect::<Vec<_>>();
    let candidates = relay_core::repos::import_candidates(&found, &registered, &now());
    let run = commit(store, candidates);
    if run.created > 0 {
        let _ = app.emit(IMPORT_EVENT, ());
    }
    Ok(run)
}

/// Runs every enabled source. A failure on one side is reported without losing the other side.
pub async fn run_enabled(app: &AppHandle, store: &V2Store) -> ImportRunResult {
    let mut result = ImportRunResult::default();
    match run_clickup(app, store).await {
        Ok(run) => merge(&mut result, run),
        Err(error) => result.clickup_error = Some(error),
    }
    match run_github(app, store).await {
        Ok(run) => merge(&mut result, run),
        Err(error) => result.github_error = Some(error),
    }
    result
}

#[tauri::command]
pub async fn v2_import_now(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<ImportRunResult, CommandError> {
    Ok(run_enabled(&app, &store).await)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// One refresh pass. Due links are read with #30's exact reads and their cadence, regardless of
/// discovery toggles. Absence from a discovery list, a truncated page and request failures never
/// write state; only a successful exact read does, and it touches the Reference snapshot only.
async fn refresh_once(app: &AppHandle) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let at = now();
    let ms = now_ms();
    let due = match store.due_import_refreshes(ms, MAX_IN_FLIGHT) {
        Ok(due) => due,
        Err(_) => return,
    };
    if due.is_empty() {
        return;
    }
    let creds = live::live_credentials(&store).await;

    for target in due {
        let expected = target.updated_at.clone();
        let key = target.key.clone();
        let identity = ReferenceKey::from_storage(
            &target.kind,
            &target.target_id,
            Some(&target.canonical_url),
        );
        let outcome = match identity.as_ref() {
            Some(key) => read_exact_item(key, &creds, &at).await,
            None => Err(ProviderFailure::Unavailable),
        };
        let (update, snapshot_json) = match outcome {
            Ok((row, item)) => {
                let state = match item {
                    ExactItem::Github(hit) => github_imported_state(&hit),
                    ExactItem::Clickup(task) => {
                        clickup_imported_state(&task, target.account_id.as_deref())
                    }
                };
                let previous = store
                    .reference_snapshot_json(&target.kind, &target.target_id)
                    .ok()
                    .flatten()
                    .map(|json| ReferenceSnapshot::from_json(&json))
                    .or_else(|| row.snapshot.clone())
                    .unwrap_or_default();
                let snapshot = refresh_snapshot(&previous, state, &at);
                (
                    refresh_success_update(&target.provider, ms),
                    Some(snapshot.to_json()),
                )
            }
            Err(failure) => (
                refresh_failure_update(target.attempts.saturating_add(1), &failure, ms),
                None,
            ),
        };
        let applied = store
            .record_import_refresh(&key, &expected, snapshot_json.as_deref(), &update, &now())
            .unwrap_or(false);
        if applied && snapshot_json.is_some() {
            let _ = app.emit(REFERENCE_EVENT, key);
            let _ = app.emit(IMPORT_EVENT, ());
        }
    }
}

/// One GitHub Import pass outside the schedule, after a repo registration.
pub fn run_github_in_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let store = app.state::<Arc<V2Store>>().inner().clone();
        let _ = run_github(&app, &store).await;
    });
}

/// Starts discovery at the provider cadences and the independent live-refresh worker.
pub fn start(app: AppHandle) {
    let clickup_app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLICKUP_START).await;
        loop {
            let store = clickup_app.state::<Arc<V2Store>>().inner().clone();
            let _ = run_clickup(&clickup_app, &store).await;
            tokio::time::sleep(Duration::from_millis(
                relay_core::v2::CLICKUP_REFRESH_MS as u64,
            ))
            .await;
        }
    });
    // GitHub discovery is create-only Note Import behind its toggle. One loop owns it; setup,
    // registration and the manual button share the same guarded service.
    let github_app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(GITHUB_START).await;
        loop {
            let store = github_app.state::<Arc<V2Store>>().inner().clone();
            let _ = run_github(&github_app, &store).await;
            tokio::time::sleep(Duration::from_millis(
                relay_core::v2::GITHUB_REFRESH_MS as u64,
            ))
            .await;
        }
    });
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(REFRESH_TICK).await;
            refresh_once(&app).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::clickup::ClickUpTask;
    use relay_core::github::{GithubHit, GithubItem};
    use relay_core::v2::{ImportCandidate, ReferenceKey};

    fn card(id: &str) -> ImportCandidate {
        ImportCandidate {
            key: ReferenceKey::Clickup { id: id.into() },
            source_kind: "clickup".into(),
            title: "Ship the RC".into(),
            canonical_url: format!("https://app.clickup.com/t/{id}"),
            snapshot_json: Some(
                ReferenceSnapshot::from_provider(
                    "Ship the RC",
                    "open",
                    vec![],
                    "2026-11-21T00:00:00.000Z",
                )
                .to_json(),
            ),
            account_id: Some("42".into()),
        }
    }

    #[test]
    fn fixture_states_only_rewrite_the_reference_snapshot_when_discovery_is_off() {
        let store = V2Store::open_in_memory().unwrap();
        let settings = store.settings().unwrap();
        assert!(!settings.import_clickup_enabled);
        assert!(!settings.import_github_enabled);
        store.import_note(&card("fixture")).unwrap();

        let hit = |state: &str, merged: bool| GithubHit {
            owner_repo: "acme/relay".into(),
            number: 7,
            title: "Ship the RC".into(),
            url: "https://github.com/acme/relay/pull/7".into(),
            kind: GithubItem::Pull,
            state: state.into(),
            updated_at: None,
            draft: false,
            merged,
        };
        assert_eq!(
            github_imported_state(&hit("closed", false)),
            ImportedStateProxy::Closed
        );
        assert_eq!(
            github_imported_state(&hit("closed", true)),
            ImportedStateProxy::Merged
        );

        let reassigned = ClickUpTask {
            id: "fixture".into(),
            name: "Ship the RC".into(),
            description: None,
            status: Some("doing".into()),
            status_type: Some("custom".into()),
            list_name: None,
            due_date: None,
            priority: None,
            url: None,
            date_updated: None,
            assignee_ids: vec!["77".into()],
        };
        assert_eq!(
            clickup_imported_state(&reassigned, Some("42")),
            ImportedStateProxy::Reassigned
        );

        let before = store.workspace_snapshot().unwrap();
        let previous = ReferenceSnapshot::from_provider(
            "Ship the RC",
            "open",
            vec![],
            "2026-11-21T00:00:00.000Z",
        );
        let snapshot = refresh_snapshot(
            &previous,
            ImportedStateProxy::Reassigned,
            "2026-11-21T00:10:00.000Z",
        );
        let key = "clickup:fixture";
        let current = store
            .due_import_refreshes(i64::MAX, 10)
            .unwrap()
            .into_iter()
            .find(|target| target.key == key)
            .unwrap()
            .updated_at;
        let update = refresh_success_update("clickup", 1_000_000);
        assert!(
            store
                .record_import_refresh(
                    key,
                    &current,
                    Some(&snapshot.to_json()),
                    &update,
                    "2026-11-21T00:10:00.000Z"
                )
                .unwrap()
        );
        let after = store.workspace_snapshot().unwrap();
        assert_eq!(after.notes.len(), before.notes.len());
        assert_eq!(after.notes[0].body, before.notes[0].body);
        assert_eq!(after.notes[0].tasks.len(), 0);
        let changed = ReferenceSnapshot::from_json(
            after.notes[0].lines[0].references[0]
                .snapshot_json
                .as_deref()
                .unwrap(),
        );
        assert_eq!(changed.state.as_deref(), Some("unassigned"));
        assert_eq!(changed.title.as_deref(), Some("Ship the RC"));
    }

    // Alias so the test doesn't need to import the enum under a second name.
    use relay_core::v2::ImportedState as ImportedStateProxy;
}
