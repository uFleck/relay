//! IPC commands the main window calls. They stay thin: the store owns the rules, this module
//! only reads the app version, maps the UI's decision string and shapes errors for the page.

use relay_core::model::ProposalDecision;
use relay_core::store::{Store, StoreError};
use relay_core::view::Snapshot;
use serde::Serialize;
use tauri::{AppHandle, State};

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

/// The snapshot behind `getSnapshot()` in `src/ipc.ts`.
#[tauri::command]
pub fn relay_snapshot(app: AppHandle, store: State<'_, Store>) -> Result<Snapshot, CommandError> {
    let version = app.package_info().version.to_string();
    // Sync sources and update state are wired later; the caller supplies none for now.
    Ok(store.ui_snapshot(&version, Vec::new())?)
}

/// A human ack from the task sheet. The rev guard keeps the optimistic UI honest.
#[tauri::command]
pub fn ack_change(
    task_id: String,
    change_id: String,
    rev: i64,
    store: State<'_, Store>,
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
    store: State<'_, Store>,
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
