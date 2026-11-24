//! T3 dispatch adapter. Core owns the durable session sequence. This module supplies CLI results,
//! runs blocking work off the UI thread, creates the progress row and publishes Task changes.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use relay_core::t3_dispatch::{
    DispatchAttempt, DispatchStart, DispatchState as AttemptState, ListedThread, ThreadIdentity,
    link_existing, recover, view,
};
use relay_core::t3_session::{self, StartOutcome, StartRequest, Transport, TransportError};
use relay_core::v2::{T3Target, V2Store, dispatch_readiness};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{CommandError, V2_CHANGE_EVENT};
use crate::t3_cli::{self, CliError, LaunchClass, StartResponse};

const DISPATCH_EVENT: &str = "relay://dispatch";

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

struct CliTransport;

impl Transport for CliTransport {
    fn start(&self, request: &StartRequest<'_>) -> Result<StartOutcome, TransportError> {
        t3_cli::start(
            request.selector,
            request.data_dir,
            request.title,
            request.brief,
            request.base_ref,
        )
        .map(|outcome| match outcome {
            LaunchClass::Responded(response) => StartOutcome::Responded(identity_from(response)),
            LaunchClass::KnownFailure(reason) => StartOutcome::KnownFailure(reason.to_string()),
            LaunchClass::Uncertain(reason) => StartOutcome::Uncertain(reason.to_string()),
        })
        .map_err(cli_error)
    }

    fn list(&self, selector: &str, data_dir: &str) -> Result<Vec<ListedThread>, TransportError> {
        t3_cli::list(selector, data_dir).map_err(cli_error)
    }

    fn show(&self, thread_id: &str, data_dir: &str) -> Result<ThreadIdentity, TransportError> {
        t3_cli::show(thread_id, data_dir).map_err(cli_error)
    }
}

fn identity_from(response: StartResponse) -> ThreadIdentity {
    ThreadIdentity {
        thread_id: response.thread_id,
        run_id: response.run_id.unwrap_or_default(),
        link: response.link.unwrap_or_default(),
        project_id: response.project_id.unwrap_or_default(),
    }
}

fn cli_error(error: CliError) -> TransportError {
    match error {
        CliError::LauncherMissing => TransportError::LauncherMissing,
        other => TransportError::Other(other.to_string()),
    }
}

fn publish(app: &AppHandle, task_id: &str) {
    let _ = app.emit(DISPATCH_EVENT, task_id.to_string());
    let _ = app.emit(V2_CHANGE_EVENT, task_id.to_string());
}

fn track_bound(store: &Arc<V2Store>, attempt: &DispatchAttempt) {
    if attempt.state == AttemptState::Bound
        && let Some(thread_id) = attempt.thread_id.clone()
    {
        let _ = store.ensure_progress(&attempt.attempt_id, &thread_id, attempt.run_id.as_deref());
    }
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

fn begin_attempt(
    app: &AppHandle,
    task_id: &str,
    base_ref: Option<String>,
) -> Result<(), CommandError> {
    let start = DispatchStart {
        task_id: task_id.to_string(),
        data_dir: data_dir()?,
        base_ref,
    };
    let app = app.clone();
    let task_id = task_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Arc<V2Store>>().inner().clone();
        match t3_session::start(&store, &CliTransport, &start) {
            Ok(Some(attempt)) => track_bound(&store, &attempt),
            Ok(None) => {}
            Err(error) => eprintln!("relay: could not run the T3 session: {error}"),
        }
        app.state::<DispatchGate>().end(&task_id);
        publish(&app, &task_id);
    });
    Ok(())
}

fn reconcile_attempt(app: AppHandle, attempt: DispatchAttempt) {
    let task_id = attempt.task_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Arc<V2Store>>().inner().clone();
        match t3_session::reconcile(&store, &CliTransport, &attempt) {
            Ok(Some(bound)) => track_bound(&store, &bound),
            Ok(None) => {}
            Err(error) => eprintln!("relay: could not reconcile the T3 session: {error}"),
        }
        publish(&app, &task_id);
    });
}

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
    if gate.begin(task_id) && begin_attempt(app, task_id, None).is_err() {
        gate.end(task_id);
    }
}

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
        if gate.begin(&task_id)
            && let Err(error) = begin_attempt(app, &task_id, None)
        {
            eprintln!("relay: could not start dispatch for {task_id}: {error}");
            gate.end(&task_id);
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
    if let Err(error) = begin_attempt(&app, &task_id, base_ref) {
        gate.end(&task_id);
        return Err(error);
    }
    Ok(())
}

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
    let listed = t3_cli::list(&selector, &attempt.t3_data_dir).map_err(|error| {
        CommandError::new(
            "invalid_input",
            format!("could not list threads for this T3 project: {error}"),
        )
    })?;
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
        &listed,
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
    track_bound(store.inner(), &attempt);
    publish(&app, &task_id);
    Ok(())
}

#[tauri::command(async)]
pub fn v2_dispatch_cancel(
    task_id: String,
    gate: State<'_, DispatchGate>,
) -> Result<(), CommandError> {
    gate.end(&task_id);
    Ok(())
}
