//! ClickUp adapter. The v1 Task-creating assigned-card sync is gone: the timer, the manual run
//! and the Sources row now all point at the v2 create-only Note Import service. Workspace,
//! space and project selection live in v2 settings; credentials stay in the keyring.

use std::sync::{Arc, Mutex};

use relay_core::clickup::{ClickUpClient, Named};
use relay_core::v2::{V2ImportScope, V2SettingsUpdate, V2Store};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::commands::CommandError;
use crate::note_import;
use crate::secrets;

/// Kept for the legacy credential handlers so a v1 fixture cannot change a v2 connection.
const CLICKUP_TOKEN: &str = "clickup-token";

static LAST: Mutex<Option<Result<SyncResult, String>>> = Mutex::new(None);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub configured: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub created: usize,
    pub updated: usize,
    pub archived: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub workspace_id: String,
    /// `None` tracks the whole workspace. Guests have no spaces to pick from.
    pub space_id: Option<String>,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeView {
    pub scope: Option<Scope>,
    pub last: Option<SyncResult>,
    pub error: Option<String>,
}

/// The last run's failure, for the Sources row.
pub fn last_error() -> Option<String> {
    LAST.lock().ok()?.as_ref()?.as_ref().err().cloned()
}

fn load_scope(store: &V2Store) -> Option<Scope> {
    let settings = store.settings().ok()?;
    let raw = settings.import_scope?;
    let scope = V2ImportScope::from_json(&raw)?;
    Some(Scope {
        workspace_id: scope.workspace_id,
        space_id: scope.space_id,
        project_id: scope.project_id,
    })
}

/// The Sources row. It reports the v2 credential and the last Import result, never a v1 token.
pub fn sync_source() -> relay_core::view::SyncSource {
    use relay_core::view::{SyncSource, SyncState};
    let (state, reason) = if secrets::v2::read(CLICKUP_TOKEN).is_none() {
        (SyncState::Off, "not connected".to_string())
    } else if let Some(error) = last_error() {
        (SyncState::Down, error)
    } else {
        (SyncState::Ok, "token stored".to_string())
    };
    SyncSource {
        name: "ClickUp".into(),
        short: "ClickUp".into(),
        state,
        reason: Some(reason),
    }
}

fn client() -> Result<ClickUpClient, String> {
    let token = secrets::read(CLICKUP_TOKEN).ok_or("ClickUp is not connected")?;
    ClickUpClient::new(token).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clickup_status() -> ConnectorStatus {
    ConnectorStatus {
        configured: secrets::read(CLICKUP_TOKEN).is_some(),
    }
}

#[tauri::command]
pub async fn clickup_set_token(token: String) -> Result<ConnectorStatus, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("the ClickUp token is empty".into());
    }
    let client = ClickUpClient::new(token.clone()).map_err(|error| error.to_string())?;
    client
        .check()
        .await
        .map_err(|error| format!("ClickUp rejected the token: {error}"))?;
    secrets::write(CLICKUP_TOKEN, &token)?;
    Ok(ConnectorStatus { configured: true })
}

#[tauri::command]
pub fn clickup_clear_token() -> ConnectorStatus {
    secrets::delete(CLICKUP_TOKEN);
    ConnectorStatus::default()
}

#[tauri::command]
pub async fn clickup_workspaces() -> Result<Vec<Named>, String> {
    client()?
        .workspaces()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clickup_spaces(workspace_id: String) -> Result<Vec<Named>, String> {
    client()?
        .spaces(&workspace_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clickup_scope(store: State<'_, Arc<V2Store>>) -> ScopeView {
    let last = LAST.lock().ok().and_then(|last| last.clone());
    let (last, error) = match last {
        Some(Ok(result)) => (Some(result), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    ScopeView {
        scope: load_scope(&store),
        last,
        error,
    }
}

#[tauri::command]
pub fn clickup_set_scope(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    scope: Scope,
) -> Result<(), CommandError> {
    let workspace_id = scope.workspace_id.trim();
    let project_id = scope.project_id.trim();
    if workspace_id.is_empty() || project_id.is_empty() {
        return Err(CommandError::new(
            "invalid_input",
            "Choose a workspace and a project.",
        ));
    }
    if !store
        .list_projects()?
        .iter()
        .any(|project| project.id == project_id)
    {
        return Err(CommandError::new(
            "invalid_input",
            "The Import scope names an unknown Relay project.",
        ));
    }
    let import_scope = V2ImportScope {
        workspace_id: workspace_id.to_string(),
        space_id: scope.space_id.filter(|id| !id.trim().is_empty()),
        project_id: project_id.to_string(),
    };
    let settings = store.settings()?;
    store.update_settings(V2SettingsUpdate {
        import_clickup_enabled: settings.import_clickup_enabled,
        import_github_enabled: settings.import_github_enabled,
        import_scope: Some(import_scope.to_json()),
    })?;
    let _ = app.emit(crate::settings::SETTINGS_CHANGE_EVENT, ());
    Ok(())
}

#[tauri::command]
pub async fn clickup_sync_now(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<SyncResult, String> {
    let result = note_import::run_clickup(&app, &store).await;
    let outcome = result
        .as_ref()
        .map(|run| SyncResult {
            created: run.created,
            truncated: run.truncated,
            ..SyncResult::default()
        })
        .map_err(|error| error.clone());
    if let Ok(mut last) = LAST.lock() {
        *last = Some(outcome.clone());
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_round_trips_through_v2_settings() {
        let store = V2Store::open_in_memory().unwrap();
        let settings = store.settings().unwrap();
        let scope = V2ImportScope {
            workspace_id: "WS".into(),
            space_id: None,
            project_id: "p1".into(),
        };
        store
            .update_settings(V2SettingsUpdate {
                import_clickup_enabled: settings.import_clickup_enabled,
                import_github_enabled: settings.import_github_enabled,
                import_scope: Some(scope.to_json()),
            })
            .unwrap();
        let loaded = load_scope(&store).unwrap();
        assert_eq!(loaded.workspace_id, "WS");
        assert_eq!(loaded.space_id, None);
        assert_eq!(loaded.project_id, "p1");
    }
}
