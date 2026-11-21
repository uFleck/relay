//! IPC commands the windows call. They stay thin: the v2 store owns the rules, this
//! module only shapes results and errors for the UI. There is no legacy intake path.

use relay_core::v2::{
    ComposerSaveRequest, ReferenceParseResult, V2Error, V2Note, V2NotebookSnapshot, V2PickerRow,
    V2Store, V2Task, V2WorkspaceSnapshot, WorkspaceSearchResult, composer, parse_urls,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

/// The IPC error shape the UI logs. `kind` is a stable code; `message` is for humans.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: String,
    pub message: String,
}

impl CommandError {
    pub(crate) fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<V2Error> for CommandError {
    fn from(error: V2Error) -> Self {
        Self::new(error.kind(), error.to_string())
    }
}

/// The app version, read from Cargo at build time. Neutral surface for the rail,
/// Settings and the Dock; no legacy snapshot is needed to render it.
#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Sent to both windows after a committed v2 write, so a Dock submission refreshes the
/// main Notebook without waiting for focus or a timer.
pub const V2_CHANGE_EVENT: &str = "relay://v2-change";

/// The Notebook read used by the dev capture surface. The main window uses the flat
/// workspace snapshot below.
#[tauri::command(async)]
pub fn v2_notebook_snapshot(
    store: State<'_, Arc<V2Store>>,
) -> Result<V2NotebookSnapshot, CommandError> {
    Ok(store.notebook_snapshot()?)
}

/// The one workspace read used by Notebook and Tasks. `tasks` is flat, so a routed Task
/// resolves even when the active filter or a day's collapsed state hides its source Note.
#[tauri::command(async)]
pub fn v2_workspace_snapshot(
    store: State<'_, Arc<V2Store>>,
) -> Result<V2WorkspaceSnapshot, CommandError> {
    Ok(store.workspace_snapshot()?)
}

/// Shared filter path: standalone `+project` and `@reference` tokens plus literal FTS text.
/// The result is ids only; the caller already holds the workspace snapshot.
#[tauri::command(async)]
pub fn v2_workspace_search(
    query: String,
    store: State<'_, Arc<V2Store>>,
) -> Result<WorkspaceSearchResult, CommandError> {
    Ok(store.search_workspace(&query)?)
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
    // #34: every new Act or Wait Task starts its own Brief attempt right after the commit,
    // independent of a project and of which window submitted it.
    crate::opencode::start_for_tasks(&app, &saved);
    // #35: a ?? Task starts its Grilling exchange on the same committed write.
    crate::grilling::start_for_tasks(&app, &saved);
    // #36: an Act Task that is already ready dispatches on the same committed write.
    crate::dispatch::start_for_tasks(&app, &saved);
    let _ = app.emit(V2_CHANGE_EVENT, saved.id.clone());
    Ok(saved)
}

/// The local `@` picker query. Core owns the ranking and grouping; this command is a thin
/// read over the v2 store so a missing IPC binding cannot be mistaken for an empty result.
#[tauri::command(async)]
pub fn v2_reference_picker(
    query: String,
    store: State<'_, Arc<V2Store>>,
) -> Result<Vec<V2PickerRow>, CommandError> {
    Ok(store.reference_picker(&query)?)
}

/// Canonical URL recognition over free text. The Composer calls this on paste and on a
/// typed delimiter so chip conversion uses the same parser as #25's saved submission.
#[tauri::command(async)]
pub fn v2_reference_parse(text: String) -> Result<ReferenceParseResult, CommandError> {
    Ok(parse_urls(&text))
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
    // A finished Grilling exchange waiting for a project becomes the closing-approval turn.
    crate::grilling::project_assigned(store.inner(), &task_id)?;
    // Naming a project can close the last dispatch gate for Act, Wait or Grill.
    crate::dispatch::start_ready_task(&app, &task_id);
    let _ = app.emit(V2_CHANGE_EVENT, task.note_id.clone());
    Ok(task)
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
