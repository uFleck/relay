//! T3 progress worker and continuation commands (#37). One read per Task at a time, a bounded
//! number of child processes, and a per-Task backoff up to 30 s. Every observation is committed
//! only while the Task is open and the thread and revisions captured before the read still match.
//! A read failure keeps the last good view and marks it stale with a Settings recovery path.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use relay_core::t3_progress::{
    CommitGuard, ObservationContext, RETRY_MESSAGE, SendKind, SendOperation, SendState,
    T3ProgressView, ThreadObservation, confirms_send, decide, task_status,
};
use relay_core::v2::{TaskStatus, V2Store};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Semaphore;

use crate::commands::{CommandError, V2_CHANGE_EVENT};
use crate::t3_cli::{self, SendOutcome};

/// A Task sheet also refreshes metadata once when an open Returned or Failed sheet mounts.
pub const PROGRESS_EVENT: &str = "relay://t3-progress";
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_CONCURRENCY: usize = 4;
const STALE_REASON: &str = "T3 could not be read. Check T3 Code in Settings, then check again.";

pub struct ProgressState {
    reads: Mutex<HashSet<String>>,
    next_read: Mutex<HashMap<String, Instant>>,
    failures: Mutex<HashMap<String, Duration>>,
    permits: Arc<Semaphore>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    outcome: &'static str,
    message: String,
    run_id: Option<String>,
}

impl SendResult {
    fn of(outcome: &'static str, message: impl Into<String>, run_id: Option<String>) -> Self {
        Self {
            outcome,
            message: message.into(),
            run_id,
        }
    }
}

// Keep the worker's state construction explicit: tokio's Semaphore has no Default.
impl Default for ProgressState {
    fn default() -> Self {
        Self {
            reads: Mutex::new(HashSet::new()),
            next_read: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
        }
    }
}

fn publish(app: &AppHandle, task_id: &str) {
    let _ = app.emit(PROGRESS_EVENT, task_id.to_string());
    let _ = app.emit(V2_CHANGE_EVENT, task_id.to_string());
}

impl ProgressState {
    fn begin(&self, task_id: &str) -> bool {
        self.reads
            .lock()
            .expect("relay progress gate poisoned")
            .insert(task_id.to_string())
    }

    fn end(&self, task_id: &str) {
        self.reads
            .lock()
            .expect("relay progress gate poisoned")
            .remove(task_id);
    }

    fn backoff_done(&self, task_id: &str) -> bool {
        !self
            .next_read
            .lock()
            .expect("relay progress backoff poisoned")
            .get(task_id)
            .is_some_and(|at| Instant::now() < *at)
    }

    fn record_failure(&self, task_id: &str) {
        let mut failures = self
            .failures
            .lock()
            .expect("relay progress backoff poisoned");
        let previous = failures.get(task_id).copied().unwrap_or(POLL_INTERVAL);
        let next = previous.saturating_mul(2).min(MAX_BACKOFF);
        failures.insert(task_id.to_string(), next);
        self.next_read
            .lock()
            .expect("relay progress backoff poisoned")
            .insert(task_id.to_string(), Instant::now() + next);
    }

    fn record_success(&self, task_id: &str) {
        self.failures
            .lock()
            .expect("relay progress backoff poisoned")
            .remove(task_id);
        self.next_read
            .lock()
            .expect("relay progress backoff poisoned")
            .remove(task_id);
    }
}

/// Starts the single worker after the v2 store is open. Bound threads without a progress row are
/// backfilled, so a restart resumes observation instead of replaying a dispatch or a send.
pub fn init(app: &AppHandle) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    if let Err(error) = store.backfill_progress() {
        eprintln!("relay: could not backfill T3 progress: {error}");
    }
    let app = app.clone();
    let state = app.state::<Arc<ProgressState>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            poll_once(&app, &state).await;
        }
    });
}

async fn poll_once(app: &AppHandle, state: &Arc<ProgressState>) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let watches = match tokio::task::spawn_blocking(move || store.watch_items()).await {
        Ok(Ok(watches)) => watches,
        _ => return,
    };
    for watch in watches {
        let task_id = watch.task_id.clone();
        if !state.begin(&task_id) || !state.backoff_done(&task_id) {
            state.end(&task_id);
            continue;
        }
        let permit = match Arc::clone(&state.permits).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                state.end(&task_id);
                continue;
            }
        };
        let app = app.clone();
        let state = state.clone();
        tauri::async_runtime::spawn(async move {
            let _permit = permit;
            observe(app, state, watch).await;
        });
    }
}

async fn observe(
    app: AppHandle,
    state: Arc<ProgressState>,
    watch: relay_core::t3_progress::ProgressWatch,
) {
    let task_id = watch.task_id.clone();
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let thread_id = watch.thread_id.clone();
    let data_dir = watch.data_dir.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let observation =
            t3_cli::show_observation(&thread_id, &data_dir).map_err(|e| e.to_string())?;
        commit_captured(&store, &watch, &observation).map_err(|e| e.to_string())
    })
    .await;
    match outcome {
        Ok(Ok(changed)) => {
            state.record_success(&task_id);
            if changed {
                publish(&app, &task_id);
            }
        }
        Ok(Err(error)) => {
            let store = app.state::<Arc<V2Store>>().inner().clone();
            let stale_task = task_id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let _ = store.mark_stale(&stale_task, &format!("{STALE_REASON} ({error})"));
            })
            .await;
            state.record_failure(&task_id);
            publish(&app, &task_id);
        }
        Err(_) => state.record_failure(&task_id),
    }
    state.end(&task_id);
}

/// Commits with the revision and operation captured before the read. A closure or a newer
/// observation that landed meanwhile makes this a no-op.
fn commit_captured(
    store: &V2Store,
    watch: &relay_core::t3_progress::ProgressWatch,
    observation: &ThreadObservation,
) -> Result<bool, relay_core::v2::V2Error> {
    let view = store
        .progress_view(&watch.task_id)?
        .ok_or(relay_core::v2::V2Error::NotFound(watch.task_id.clone()))?;
    let status = task_status(&view.task_status).unwrap_or(TaskStatus::Dispatched);
    let context = ObservationContext {
        current_status: status,
        expected_run_id: view.run_id.as_deref(),
        send: watch.send.as_ref(),
        observation,
    };
    let Some(effect) = decide(&context) else {
        return Ok(false);
    };
    let guard = CommitGuard {
        task_id: watch.task_id.clone(),
        attempt_id: watch.attempt_id.clone(),
        thread_id: watch.thread_id.clone(),
        revision: watch.revision,
        operation_revision: watch.send.as_ref().map(|operation| operation.revision),
    };
    store.commit_observation(&guard, &effect, observation)
}

/// A fresh, user-initiated refresh (one read when an open sheet mounts) uses current revisions.
fn refresh_now(store: &Arc<V2Store>, task_id: &str) -> Result<bool, CommandError> {
    let view = store
        .progress_view(task_id)?
        .ok_or_else(|| CommandError::new("invalid_input", "this Task has no T3 thread"))?;
    if !view.can_close || view.can_follow_up {
        // Only an open Returned or Failed sheet refreshes metadata once.
        return Ok(false);
    }
    let observation = t3_cli::show_observation(&view.thread_id, &data_dir_for(store, task_id)?)
        .map_err(|error| CommandError::new("invalid_input", error.to_string()))?;
    let status = task_status(&view.task_status).unwrap_or(TaskStatus::Dispatched);
    let context = ObservationContext {
        current_status: status,
        expected_run_id: view.run_id.as_deref(),
        send: view.active_send.as_ref(),
        observation: &observation,
    };
    let Some(effect) = decide(&context) else {
        return Ok(false);
    };
    let guard = CommitGuard {
        task_id: task_id.to_string(),
        attempt_id: view.attempt_id,
        thread_id: view.thread_id,
        revision: view.revision,
        operation_revision: view.active_send.as_ref().map(|send| send.revision),
    };
    Ok(store.commit_observation(&guard, &effect, &observation)?)
}

fn data_dir_for(store: &Arc<V2Store>, task_id: &str) -> Result<String, CommandError> {
    let attempt = store
        .latest_dispatch_attempt(task_id)?
        .ok_or_else(|| CommandError::new("invalid_input", "this Task has no T3 attempt"))?;
    Ok(attempt.t3_data_dir)
}

#[tauri::command(async)]
pub fn v2_t3_progress(
    task_id: String,
    store: State<'_, Arc<V2Store>>,
) -> Result<Option<T3ProgressView>, CommandError> {
    Ok(store.progress_view(&task_id)?)
}

/// One metadata refresh when an open Returned or Failed sheet mounts.
#[tauri::command(async)]
pub fn v2_t3_refresh(
    task_id: String,
    app: AppHandle,
    state: State<'_, Arc<ProgressState>>,
) -> Result<(), CommandError> {
    if !state.begin(&task_id) {
        return Ok(());
    }
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let changed = refresh_now(&store, &task_id)?;
    if changed {
        publish(&app, &task_id);
    }
    state.end(&task_id);
    Ok(())
}

/// Follow-up and Retry share one operation path. The pre-send read can resume an already active
/// T3 run without sending the draft, and the message is reserved before the CLI is invoked.
#[tauri::command(async)]
pub fn v2_t3_send(
    task_id: String,
    message: String,
    kind: String,
    app: AppHandle,
    state: State<'_, Arc<ProgressState>>,
) -> Result<SendResult, CommandError> {
    let send_kind = match kind.as_str() {
        "followup" => SendKind::Followup,
        "retry" => SendKind::Retry,
        _ => {
            return Err(CommandError::new(
                "invalid_input",
                "unknown continuation kind",
            ));
        }
    };
    if !state.begin(&task_id) {
        return Ok(SendResult::of(
            "busy",
            "A T3 read is already in flight.",
            None,
        ));
    }
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let result = send_message(&store, &task_id, send_kind, &message);
    state.end(&task_id);
    let result = result?;
    publish(&app, &task_id);
    Ok(result)
}

fn send_message(
    store: &Arc<V2Store>,
    task_id: &str,
    kind: SendKind,
    message: &str,
) -> Result<SendResult, CommandError> {
    let view = store
        .progress_view(task_id)?
        .ok_or_else(|| CommandError::new("invalid_input", "this Task has no T3 thread"))?;
    let eligible = match kind {
        SendKind::Followup => view.can_follow_up,
        SendKind::Retry => view.can_retry_run,
        SendKind::Resend => false,
    };
    if !eligible {
        return Err(CommandError::new(
            "invalid_input",
            "this Task cannot be continued in that state",
        ));
    }
    // Retry always sends the pinned sentence, never text derived from the live Task.
    let message = if kind == SendKind::Retry {
        RETRY_MESSAGE
    } else {
        message
    };
    let data_dir = data_dir_for(store, task_id)?;
    // Eligibility and the run baseline come from a read, never from the stored view alone.
    let observed = t3_cli::show_observation(&view.thread_id, &data_dir).map_err(|error| {
        CommandError::new(
            "invalid_input",
            format!("could not read the T3 thread: {error}"),
        )
    })?;
    if observed.status.is_active() || observed.pending_request_count > 0 {
        let _ = refresh_now(store, task_id);
        return Ok(SendResult::of(
            "draft_unsent",
            "T3 is still working. Relay is observing that run; the draft was not sent.",
            observed.latest_run_id,
        ));
    }
    let Some(operation) = store.reserve_send(task_id, kind, message)? else {
        return Ok(SendResult::of(
            "fenced",
            "Another continuation is still unresolved in T3.",
            None,
        ));
    };
    deliver(store, &operation, &data_dir, false)
}

/// Sends a reserved draft. When the CLI gives no run id, or the response was lost on a previous
/// run, a read must prove exactly one new run with the exact message before it is acknowledged.
fn deliver(
    store: &Arc<V2Store>,
    operation: &SendOperation,
    data_dir: &str,
    reopened: bool,
) -> Result<SendResult, CommandError> {
    let outcome =
        t3_cli::send(&operation.thread_id, data_dir, &operation.message).map_err(|error| {
            CommandError::new("invalid_input", format!("could not send to T3: {error}"))
        })?;
    match outcome {
        SendOutcome::Accepted(response) if response.run_id.is_some() => {
            let run_id = response.run_id.clone().unwrap_or_default();
            let _ = store.acknowledge_send(&operation.operation_id, &run_id, "accepted");
            Ok(SendResult::of(
                "sent",
                "T3 accepted the message.",
                Some(run_id),
            ))
        }
        SendOutcome::Accepted(_) => confirm_or_fence(store, operation, data_dir, reopened),
        SendOutcome::KnownFailure(reason) => {
            let _ = store.fail_send(&operation.operation_id, true, reason);
            Ok(SendResult::of(
                "draft_kept",
                "T3 rejected the message. The draft is kept for an explicit retry.",
                None,
            ))
        }
        SendOutcome::Uncertain(reason) => {
            let _ = store.fail_send(&operation.operation_id, false, reason);
            Ok(SendResult::of(
                "needs_confirmation",
                "The send response was lost. Confirm in T3, then check again or resend.",
                None,
            ))
        }
    }
}

fn confirm_or_fence(
    store: &Arc<V2Store>,
    operation: &SendOperation,
    data_dir: &str,
    reopened: bool,
) -> Result<SendResult, CommandError> {
    let observed = t3_cli::show_observation(&operation.thread_id, data_dir).ok();
    if let Some(run_id) = observed
        .as_ref()
        .and_then(|observation| confirms_send(operation, observation))
    {
        if reopened {
            let _ = store.confirm_send(&operation.operation_id, &run_id);
        } else {
            let _ = store.acknowledge_send(&operation.operation_id, &run_id, "confirmed_by_show");
        }
        return Ok(SendResult::of(
            "sent",
            "T3 queued the message.",
            Some(run_id),
        ));
    }
    let _ = store.fail_send(
        &operation.operation_id,
        false,
        "the send response was lost and show did not confirm the message",
    );
    Ok(SendResult::of(
        "needs_confirmation",
        "Confirm in T3 whether the message arrived, then check again or resend.",
        None,
    ))
}

/// Checks a fenced send. Automatic confirmation requires exactly one run beyond the saved baseline
/// carrying the exact user message; otherwise the UI offers Open in T3 and Check again.
#[tauri::command(async)]
pub fn v2_t3_check_again(
    task_id: String,
    app: AppHandle,
    state: State<'_, Arc<ProgressState>>,
) -> Result<CheckResult, CommandError> {
    if !state.begin(&task_id) {
        return Ok(CheckResult { confirmed: false });
    }
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let confirmed = check_send(&store, &task_id).unwrap_or(false);
    state.end(&task_id);
    if confirmed {
        publish(&app, &task_id);
    }
    Ok(CheckResult { confirmed })
}

fn check_send(store: &Arc<V2Store>, task_id: &str) -> Result<bool, CommandError> {
    let view = store
        .progress_view(task_id)?
        .ok_or_else(|| CommandError::new("invalid_input", "this Task has no T3 thread"))?;
    let Some(operation) = view.active_send else {
        return Ok(false);
    };
    if operation.state != SendState::NeedsConfirmation {
        return Ok(false);
    }
    let data_dir = data_dir_for(store, task_id)?;
    let observation = t3_cli::show_observation(&operation.thread_id, &data_dir).ok();
    let Some(run_id) = observation
        .as_ref()
        .and_then(|observed| confirms_send(&operation, observed))
    else {
        return Ok(false);
    };
    Ok(store
        .confirm_send(&operation.operation_id, &run_id)
        .unwrap_or(false))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub confirmed: bool,
}

/// An explicit resend on the same thread, only after the user checked that the first message was
/// absent. A known pre-send failure needs no extra confirmation because no run received it.
#[tauri::command(async)]
pub fn v2_t3_resend(
    task_id: String,
    user_confirmed: bool,
    app: AppHandle,
    state: State<'_, Arc<ProgressState>>,
) -> Result<SendResult, CommandError> {
    if !state.begin(&task_id) {
        return Ok(SendResult::of(
            "busy",
            "A T3 read is already in flight.",
            None,
        ));
    }
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let result = resend_draft(&store, &task_id, user_confirmed);
    state.end(&task_id);
    let result = result?;
    publish(&app, &task_id);
    Ok(result)
}

fn resend_draft(
    store: &Arc<V2Store>,
    task_id: &str,
    user_confirmed: bool,
) -> Result<SendResult, CommandError> {
    let view = store
        .progress_view(task_id)?
        .ok_or_else(|| CommandError::new("invalid_input", "this Task has no T3 thread"))?;
    let Some(existing) = view.active_send else {
        return Err(CommandError::new(
            "invalid_input",
            "there is no retained draft to resend",
        ));
    };
    if existing.state == SendState::NeedsConfirmation && !user_confirmed {
        return Err(CommandError::new(
            "invalid_input",
            "confirm in T3 that the first message was absent before resending",
        ));
    }
    if !matches!(
        existing.state,
        SendState::NeedsConfirmation | SendState::Failed
    ) {
        return Err(CommandError::new(
            "invalid_input",
            "this draft cannot be resent in its current state",
        ));
    }
    let Some(reopened) = store.reopen_send(&existing.operation_id)? else {
        return Err(CommandError::new(
            "invalid_input",
            "the retained draft could not be reopened",
        ));
    };
    let data_dir = data_dir_for(store, task_id)?;
    deliver(store, &reopened, &data_dir, true)
}

/// Explicit local closure. The revision bump stops run polling and invalidates any read in flight;
/// a PR merge or a settled T3 thread never calls this path.
#[tauri::command(async)]
pub fn v2_t3_close(
    task_id: String,
    status: String,
    app: AppHandle,
    state: State<'_, Arc<ProgressState>>,
    store: State<'_, Arc<V2Store>>,
) -> Result<(), CommandError> {
    let target = match status.as_str() {
        "done" => TaskStatus::Done,
        "cancelled" => TaskStatus::Cancelled,
        _ => return Err(CommandError::new("invalid_input", "unknown closure")),
    };
    if !store.close_task(&task_id, target)? {
        return Err(CommandError::new(
            "invalid_input",
            "this Task is already closed",
        ));
    }
    state.end(&task_id);
    publish(&app, &task_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_gate_is_per_task_and_backoff_grows_to_thirty_seconds() {
        let state = ProgressState::default();
        assert!(state.begin("a"));
        assert!(!state.begin("a"));
        assert!(state.begin("b"));
        state.record_failure("a");
        assert!(!state.backoff_done("a"));
        assert!(state.backoff_done("b"));
        state.record_success("a");
        assert!(state.backoff_done("a"));
        assert!(state.permits.available_permits() == MAX_CONCURRENCY);
    }
}
