//! IPC commands the main window calls. They stay thin: the store owns the rules, this module
//! only reads the app version, maps the UI's decision string and shapes errors for the page.

use relay_core::model::{Note, ProposalDecision, Task};
use relay_core::store::{Store, StoreError};
use relay_core::v2::{
    ComposerSaveRequest, V2Error, V2Note, V2NotebookSnapshot, V2Store, V2Task, composer,
};
use relay_core::view::Snapshot;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::github::{self, GithubState};
use crate::imports;
use crate::slack::SlackState;

/// The IPC error shape the UI logs. `kind` is a stable code; `message` is for humans.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: String,
    pub message: String,
}

impl CommandError {
    fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.into(),
        }
    }

    fn stale_rev(task_id: &str, expected: i64, actual: i64) -> Self {
        Self::new(
            "stale_rev",
            format!("task {task_id} is at rev {actual}, expected {expected}"),
        )
    }

    fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }
}

impl From<StoreError> for CommandError {
    fn from(error: StoreError) -> Self {
        // `ClaimConflict` already renders the holder name, claim id and lease.
        Self::new(error.kind(), error.to_string())
    }
}

impl From<V2Error> for CommandError {
    fn from(error: V2Error) -> Self {
        Self::new(error.kind(), error.to_string())
    }
}

/// Sent to both windows after a committed v2 write, so a Dock submission refreshes the
/// main Notebook without waiting for focus or a timer.
pub const V2_CHANGE_EVENT: &str = "relay://v2-change";

/// The v2 read boundary. It opens on an empty fresh store with typed defaults and
/// never touches the legacy store or any credential.
#[tauri::command(async)]
pub fn v2_notebook_snapshot(
    store: State<'_, Arc<V2Store>>,
) -> Result<V2NotebookSnapshot, CommandError> {
    Ok(store.notebook_snapshot()?)
}

/// Parses and saves a Composer submission. The core owns parsing, project inference and the
/// guarded write; this command only reports the saved aggregate and announces the change.
#[tauri::command(async)]
pub fn v2_composer_save(
    request: ComposerSaveRequest,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<V2Note, CommandError> {
    let saved = composer::save_submission(&store, request)?;
    let _ = app.emit(V2_CHANGE_EVENT, saved.id.clone());
    Ok(saved)
}

/// Names a project for an unassigned Task. A repeat of the same choice is a no-op.
#[tauri::command(async)]
pub fn v2_task_assign_project(
    task_id: String,
    project_id: String,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<V2Task, CommandError> {
    let task = store.assign_task_project(&task_id, &project_id)?;
    let _ = app.emit(V2_CHANGE_EVENT, task.note_id.clone());
    Ok(task)
}

/// The snapshot behind `getSnapshot()` in `src/ipc.ts`. Off the main thread, so a refresh
/// that waits on the store lock behind a background sync cannot freeze the window.
#[tauri::command(async)]
pub fn relay_snapshot(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<Snapshot, CommandError> {
    let version = app.package_info().version.to_string();
    // MCP has its own dot in the rail and Settings; it is not repeated here.
    let [slack, jev] = app.state::<SlackState>().sync_sources();
    let sync = vec![
        slack,
        jev,
        github::sync_source(&app.state::<GithubState>().get()),
        imports::sync_source(),
    ];
    Ok(store.ui_snapshot(&version, sync)?)
}

/// A human ack from the task sheet. The rev guard keeps the optimistic UI honest.
#[tauri::command]
pub fn ack_change(
    task_id: String,
    change_id: String,
    rev: i64,
    store: State<'_, Arc<Store>>,
) -> Result<(), CommandError> {
    let task = store.get_task(&task_id)?;
    if task.rev != rev {
        return Err(CommandError::stale_rev(&task_id, rev, task.rev));
    }
    store.ack_change(None, &change_id)?;
    Ok(())
}

/// A review-queue decision. `decision` is the UI union string.
#[tauri::command]
pub fn decide_proposal(
    proposal_id: String,
    decision: String,
    rev: i64,
    store: State<'_, Arc<Store>>,
) -> Result<(), CommandError> {
    let decision = match decision.as_str() {
        "approve" => ProposalDecision::Approve,
        "reject" => ProposalDecision::Reject,
        other => {
            return Err(CommandError::invalid_input(format!(
                "decision must be `approve` or `reject`, got `{other}`"
            )));
        }
    };

    let proposal = store.get_proposal(&proposal_id)?;
    if let Some(task_id) = &proposal.task_id {
        let task = store.get_task(task_id)?;
        if task.rev != rev {
            return Err(CommandError::stale_rev(task_id, rev, task.rev));
        }
    }

    store.decide_proposal(&proposal_id, decision, "you")?;
    Ok(())
}

// Notes. Every command is async so a write waiting on the store lock never blocks the window.

#[tauri::command(async)]
pub fn notes_list(
    query: Option<String>,
    store: State<'_, Arc<Store>>,
) -> Result<Vec<Note>, CommandError> {
    Ok(store.list_notes(query.as_deref(), None)?)
}

#[tauri::command(async)]
pub fn note_create(
    title: String,
    body: String,
    store: State<'_, Arc<Store>>,
) -> Result<Note, CommandError> {
    Ok(store.create_note(&title, &body)?)
}

#[tauri::command(async)]
pub fn note_update(
    id: String,
    title: String,
    body: String,
    store: State<'_, Arc<Store>>,
) -> Result<Note, CommandError> {
    Ok(store.update_note(&id, &title, &body)?)
}

#[tauri::command(async)]
pub fn note_set_pinned(
    id: String,
    pinned: bool,
    store: State<'_, Arc<Store>>,
) -> Result<Note, CommandError> {
    Ok(store.set_note_pinned(&id, pinned)?)
}

#[tauri::command(async)]
pub fn note_delete(id: String, store: State<'_, Arc<Store>>) -> Result<(), CommandError> {
    Ok(store.delete_note(&id)?)
}

#[tauri::command(async)]
pub fn note_to_task(
    id: String,
    project_id: String,
    store: State<'_, Arc<Store>>,
) -> Result<Task, CommandError> {
    Ok(store.note_to_task(&id, &project_id)?)
}

/// Brings T3 Code to the front through its `t3code://` protocol. The prompt travels on the
/// clipboard; Relay opens the app and never starts an agent.
#[tauri::command(async)]
pub fn open_t3(app: AppHandle) -> Result<(), CommandError> {
    app.opener()
        .open_url("t3code://", None::<&str>)
        .map_err(|error| {
            CommandError::new("open_failed", format!("could not open T3 Code: {error}"))
        })
}
