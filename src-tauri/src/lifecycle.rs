//! Executes the stage selected by core. IPC commands commit facts, then call this module once.

use relay_core::task_lifecycle::{NextStage, next_stage};
use relay_core::v2::{V2Error, V2Note, V2Store};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

pub fn advance(app: &AppHandle, task_id: &str) {
    let store = app.state::<Arc<V2Store>>();
    match next_stage(store.inner(), task_id) {
        Ok(Some(NextStage::Briefing)) => crate::opencode::start_task(app, task_id),
        Ok(Some(NextStage::Grilling)) => crate::grilling::start_task(app, task_id),
        Ok(Some(NextStage::Dispatch)) => crate::dispatch::start_ready_task(app, task_id),
        Ok(None) => {}
        Err(error) => eprintln!("relay: could not select the next Task stage: {error}"),
    }
}

pub fn advance_note(app: &AppHandle, note: &V2Note) {
    for task in &note.tasks {
        advance(app, &task.id);
    }
}

pub fn advance_after_project_assignment(
    app: &AppHandle,
    store: &V2Store,
    task_id: &str,
) -> Result<(), V2Error> {
    relay_core::task_lifecycle::refresh_task_projection(store, task_id)?;
    advance(app, task_id);
    Ok(())
}
