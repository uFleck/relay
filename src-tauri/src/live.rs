//! Thin Tauri boundary for live Reference reads. Credentials stay in Rust; all query policy,
//! allowlisting, typing and ranking lives in `relay_core::v2::live`.

use std::sync::Arc;

use relay_core::model::now;
use relay_core::v2::{
    LiveCredentials, LiveQueryResult, ReferenceKey, ReferenceSnapshot, V2PickerRow, V2Store,
    live_query, read_exact,
};
use tauri::{AppHandle, Emitter, State};

use crate::commands::CommandError;
use crate::{github, secrets, slack};

const GITHUB_KEY: &str = "github-token";
const CLICKUP_KEY: &str = "clickup-token";
/// A picked or recovered Reference changed. Both windows refresh their local picker rows.
pub const REFERENCE_EVENT: &str = "relay://reference-change";

/// Resolves v2 credentials in the same order as Settings: stored token, `gh` session, public
/// access for GitHub; a stored token is required for ClickUp. Registered repos come only from
/// v2 projects, so the allowlist cannot include a stale v1 registration.
pub async fn live_credentials(store: &V2Store) -> LiveCredentials {
    let registered_repos = store
        .list_projects()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|project| project.owner_repo)
        .collect();
    let github_token = match secrets::v2::read(GITHUB_KEY) {
        Some(token) => Some(token),
        None => github::session_token().await,
    };
    LiveCredentials {
        github_token,
        clickup_token: secrets::v2::read(CLICKUP_KEY),
        registered_repos,
    }
}

#[tauri::command]
pub async fn v2_reference_search(
    query: String,
    store: State<'_, Arc<V2Store>>,
) -> Result<LiveQueryResult, CommandError> {
    let creds = live_credentials(&store).await;
    Ok(live_query(&query, &creds, &now()).await)
}

/// A deduplicated exact read for a pasted or linked key while the draft is open. Any snapshot
/// already stored is left alone when the target is unreadable, so cached content survives.
#[tauri::command]
pub async fn v2_reference_exact(
    key: String,
    canonical_url: Option<String>,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<Option<V2PickerRow>, CommandError> {
    let Some(identity) = ReferenceKey::from_identity(&key, canonical_url.as_deref()) else {
        return Ok(None);
    };
    if let Some(read) = slack::reader::resolve(&app, &store, &identity).await {
        slack::reader::persist(&app, &store, &identity, &read).await;
        return Ok(Some(read.row().clone()));
    }
    let creds = live_credentials(&store).await;
    let Some(row) = read_exact(&identity, &creds, &now()).await.ok() else {
        return Ok(None);
    };
    if let Some(snapshot) = &row.snapshot {
        store.save_reference_snapshot(&identity, snapshot)?;
        let _ = app.emit(REFERENCE_EVENT, row.key.clone());
    }
    Ok(Some(row))
}

/// Persists a picked live result as a read-only snapshot. No Note, Task or Import row is
/// created here; the picker simply gains a local row for future keystrokes and windows.
#[tauri::command]
pub fn v2_reference_pick(
    row: V2PickerRow,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<(), CommandError> {
    let Some(identity) = ReferenceKey::from_identity(&row.key, Some(&row.canonical_link)) else {
        return Err(CommandError::new(
            "invalid_input",
            "Unrecognised Reference key.",
        ));
    };
    let snapshot = row.snapshot.clone().unwrap_or_else(|| ReferenceSnapshot {
        title: Some(row.title.clone()),
        state: row.state.clone(),
        metadata: row.metadata.clone(),
        cached_at: Some(now()),
        ..ReferenceSnapshot::default()
    });
    store.save_reference_snapshot(&identity, &snapshot)?;
    let _ = app.emit(REFERENCE_EVENT, row.key.clone());
    Ok(())
}
