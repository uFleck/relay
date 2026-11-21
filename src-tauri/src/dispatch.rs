//! Dispatch orchestration (#36). Core owns the attempt; this module runs the CLI off the UI
//! thread, classifies the boundary, hydrates a partial identity and publishes the Task change.
//! It never holds a SQLite guard across a child process.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use relay_core::t3_dispatch::{
    DispatchClaim, DispatchState as AttemptState, DispatchStart, ListedThread, ThreadIdentity,
    bind, bind_after_cancel, claim, identity_matches, link_existing, record_known_failure,
    record_uncertain, recover, save_thread_id, unique_candidate, view,
};
use relay_core::v2::{T3Target, V2Note, V2Store, dispatch_readiness};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{CommandError, V2_CHANGE_EVENT};
use crate::t3_cli::{self, CliError, LaunchClass, StartResponse};

const DISPATCH_EVENT: &str = "relay://dispatch";

/// Single-flight gate. Cancellation only releases the UI wait; the spawned work still persists a
/// validated identity, so a response arriving after cancellation links the thread without
/// reopening the Task for another start.
#[derive(Default)]
pub struct DispatchGate {
    inflight: Mutex<HashSet<String>>,
}

impl DispatchGate {
    fn begin(&self, task_id: &str) -> bool {
        self.inflight
            .lock()
            .expect("relay dispatch gate poisoned")
            .insert(task_id.to_string())
    }

    fn end(&self, task_id: &str) {
        self.inflight
            .lock()
            .expect("relay dispatch gate poisoned")
            .remove(task_id);
    }
}

fn publish(app: &AppHandle, task_id: &str) {
    let _ = app.emit(DISPATCH_EVENT, task_id.to_string());
    let _ = app.emit(V2_CHANGE_EVENT, task_id.to_string());
}

fn data_dir() -> Result<String, CommandError> {
    t3_cli::t3_home()
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| {
            CommandError::new(
                "invalid_input",
                "T3's data directory is unavailable. Check the T3 installation.",
            )
        })
}

/// Reserves the attempt and starts the CLI on a blocking thread. `base_ref` comes from the explicit
/// request for worktree isolation, or `None` for T3's project defaults.
fn begin_attempt(
    app: &AppHandle,
    task_id: &str,
    base_ref: Option<String>,
    cancelled: bool,
) -> Result<(), CommandError> {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let start = DispatchStart {
        task_id: task_id.to_string(),
        data_dir: data_dir()?,
        base_ref,
    };
    let Some(claim) = claim(&store, &start)? else {
        publish(app, task_id);
        return Ok(());
    };
    spawn_claim(app.clone(), claim, cancelled);
    Ok(())
}

fn spawn_claim(app: AppHandle, claim: DispatchClaim, cancelled: bool) {
    let task_id = claim.attempt.task_id.clone();
    let attempt_id = claim.attempt.attempt_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Arc<V2Store>>().inner().clone();
        let result = t3_cli::start(
            &claim.selector,
            &claim.attempt.t3_data_dir,
            &claim.attempt.title,
            &claim.brief,
            claim.attempt.base_ref.as_deref(),
        );
        match result {
            Ok(LaunchClass::Responded(response)) => {
                hydrate_and_bind(&app, &store, &attempt_id, response, cancelled);
            }
            Ok(LaunchClass::KnownFailure(reason)) => {
                let _ = record_known_failure(&store, &attempt_id, reason);
            }
            Ok(LaunchClass::Uncertain(reason)) => {
                let _ = record_uncertain(&store, &attempt_id, reason);
            }
            Err(CliError::LauncherMissing) => {
                let _ = record_known_failure(
                    &store,
                    &attempt_id,
                    "The t3 launcher was not found. Install T3 Code or restart Relay.",
                );
            }
            Err(error) => {
                let _ = record_uncertain(&store, &attempt_id, &error.to_string());
            }
        }
        // A late response still records the link; cancellation never reopens the gate for a start.
        app.state::<DispatchGate>().end(&task_id);
        publish(&app, &task_id);
    });
}

/// Saves the validated thread id immediately, then hydrates a missing run id or link with read-only
/// `show` and commits the complete identity together with `dispatched`.
fn hydrate_and_bind(
    _app: &AppHandle,
    store: &Arc<V2Store>,
    attempt_id: &str,
    response: StartResponse,
    cancelled: bool,
) {
    if let Some(saved) = save_thread_id(store, attempt_id, &response.thread_id)
        .ok()
        .flatten()
    {
        let _ = saved;
    }
    let mut identity = ThreadIdentity {
        thread_id: response.thread_id.clone(),
        run_id: response.run_id.unwrap_or_default(),
        link: response.link.unwrap_or_default(),
        project_id: response.project_id.unwrap_or_default(),
    };
    if !identity.is_complete() {
        match t3_cli::show(&response.thread_id, &data_dir().unwrap_or_default()) {
            Ok(shown) => {
                if identity.run_id.is_empty() {
                    identity.run_id = shown.run_id;
                }
                if identity.link.is_empty() {
                    identity.link = shown.link;
                }
                if identity.project_id.is_empty() {
                    identity.project_id = shown.project_id;
                }
            }
            Err(error) => {
                let _ = record_uncertain(store, attempt_id, &error.to_string());
                return;
            }
        }
    }
    let result = if cancelled {
        bind_after_cancel(store, attempt_id, &identity)
    } else {
        bind(store, attempt_id, &identity)
    };
    if let Err(error) = result {
        let _ = record_uncertain(store, attempt_id, &error.to_string());
    }
}

/// Re-runs read-only reconciliation for one unresolved attempt. A unique exact attempt-title match
/// is adopted only after `show` confirms the thread and project. Zero, several or a read error
/// leave the Task fenced for manual recovery.
fn reconcile_attempt(app: AppHandle, attempt: relay_core::t3_dispatch::DispatchAttempt) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let task_id = attempt.task_id.clone();
    let attempt_id = attempt.attempt_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selector = T3Target::from_storage(&attempt.project_selector)
            .map(|target| target.value().to_string())
            .unwrap_or_default();
        let candidates: Vec<ListedThread> = match t3_cli::list(&selector, &attempt.t3_data_dir) {
            Ok(threads) => threads,
            Err(error) => {
                let _ = record_uncertain(&store, &attempt_id, &error.to_string());
                publish(&app, &task_id);
                return;
            }
        };
        let Some(candidate) = unique_candidate(&attempt, &candidates) else {
            let _ = record_uncertain(
                &store,
                &attempt_id,
                "No unique thread with the attempt title was found. Check again or link it.",
            );
            publish(&app, &task_id);
            return;
        };
        let thread_id = candidate.thread_id.clone();
        let identity = match t3_cli::show(&thread_id, &attempt.t3_data_dir) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = record_uncertain(&store, &attempt_id, &error.to_string());
                publish(&app, &task_id);
                return;
            }
        };
        if !identity_matches(&attempt, &identity) {
            let _ = record_uncertain(
                &store,
                &attempt_id,
                "The matching thread belongs to another project or profile.",
            );
            publish(&app, &task_id);
            return;
        }
        let _ = bind(&store, &attempt_id, &identity);
        publish(&app, &task_id);
    });
}

/// Starts Tasks that became ready on a committed write. Act is ready as soon as its Brief and
/// project exist; a Wait or Grill Task reaches readiness only through its own Go command.
pub fn start_for_tasks(app: &AppHandle, note: &V2Note) {
    for task in &note.tasks {
        start_ready_task(app, &task.id);
    }
}

/// The single trigger used after a project is named or an approval is recorded. It re-reads
/// readiness, so a Task that is still waiting on a gate is left alone.
pub fn start_ready_task(app: &AppHandle, task_id: &str) {
    let store = app.state::<Arc<V2Store>>();
    if store
        .latest_dispatch_attempt(task_id)
        .unwrap_or(None)
        .is_some()
        || dispatch_readiness(store.inner(), task_id)
            .unwrap_or(None)
            .is_none()
    {
        return;
    }
    let gate = app.state::<DispatchGate>();
    if gate.begin(task_id) && begin_attempt(app, task_id, None, false).is_err() {
        gate.end(task_id);
    }
}

/// Startup: reconcile attempts left uncertain by a restart and start ready Tasks that never
/// reserved one. Known failures are deliberately left for manual Retry.
pub fn init(app: &AppHandle) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let recovery = match recover(&store) {
        Ok(recovery) => recovery,
        Err(error) => {
            eprintln!("relay: could not recover dispatch attempts: {error}");
            return;
        }
    };
    for attempt in recovery.reconcile {
        reconcile_attempt(app.clone(), attempt);
    }
    for task_id in recovery.start {
        let gate = app.state::<DispatchGate>();
        if gate.begin(&task_id) {
            if let Err(error) = begin_attempt(app, &task_id, None, false) {
                eprintln!("relay: could not start dispatch for {task_id}: {error}");
                gate.end(&task_id);
            }
        }
    }
}

#[tauri::command(async)]
pub fn v2_dispatch_view(
    task_id: String,
    store: State<'_, Arc<V2Store>>,
) -> Result<Option<relay_core::t3_dispatch::DispatchView>, CommandError> {
    Ok(view(store.inner(), &task_id)?)
}

/// Stage Retry after a known failure. An unresolved attempt is still fenced by core, so this
/// cannot start a second thread while the first outcome is uncertain.
#[tauri::command(async)]
pub fn v2_dispatch_retry(
    task_id: String,
    base_ref: Option<String>,
    app: AppHandle,
    gate: State<'_, DispatchGate>,
) -> Result<(), CommandError> {
    if !gate.begin(&task_id) {
        return Ok(());
    }
    if let Err(error) = begin_attempt(&app, &task_id, base_ref, false) {
        gate.end(&task_id);
        return Err(error);
    }
    Ok(())
}

/// Check again after an uncertain outcome: read-only reconciliation, never a second start.
#[tauri::command(async)]
pub fn v2_dispatch_check(
    task_id: String,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<(), CommandError> {
    let attempt = store
        .latest_dispatch_attempt(&task_id)?
        .filter(|attempt| attempt.state == AttemptState::ReconcileRequired)
        .ok_or_else(|| CommandError::new("invalid_input", "there is no uncertain attempt"))?;
    reconcile_attempt(app, attempt);
    Ok(())
}

/// Links a thread the user found in T3. The UI must have compared its first message with the
/// frozen Brief; core checks the project, profile and existing bindings before committing.
#[tauri::command(async)]
pub fn v2_dispatch_link(
    task_id: String,
    thread_id: String,
    user_confirmed: bool,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<(), CommandError> {
    let attempt = store
        .latest_dispatch_attempt(&task_id)?
        .filter(|attempt| attempt.state == AttemptState::ReconcileRequired)
        .ok_or_else(|| CommandError::new("invalid_input", "there is no attempt to reconcile"))?;
    let selector = T3Target::from_storage(&attempt.project_selector)
        .map(|target| target.value().to_string())
        .unwrap_or_default();
    let identity = t3_cli::show(&thread_id, &attempt.t3_data_dir).map_err(|error| {
        CommandError::new(
            "invalid_input",
            format!("could not verify that thread: {}", error),
        )
    })?;
    let linked = link_existing(
        store.inner(),
        &attempt,
        &identity,
        &selector,
        &attempt.t3_data_dir,
        user_confirmed,
    )?;
    if linked.is_none() {
        return Err(CommandError::new(
            "invalid_input",
            "that thread could not be linked",
        ));
    }
    publish(&app, &task_id);
    Ok(())
}

/// Stops the UI wait. A response that is already in flight still records its thread identity.
#[tauri::command(async)]
pub fn v2_dispatch_cancel(task_id: String, gate: State<'_, DispatchGate>) -> Result<(), CommandError> {
    gate.end(&task_id);
    Ok(())
}
