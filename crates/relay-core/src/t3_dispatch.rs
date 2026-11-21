//! Dispatch policy for ready Tasks (#36). Core owns the attempt, its frozen inputs and every
//! transition; the Tauri adapter only supplies CLI results. Persistence lives in the v2 store so
//! the same rules apply on startup, on a stage Retry and after a restart with a lost response.
//!
//! A Task has at most one open attempt or one bound thread. The store enforces that with partial
//! unique indexes; this module never assumes the caller remembered to check. A known launch
//! failure releases the slot for one manual Retry. An uncertain start keeps the slot fenced and
//! leaves the Task undispatched until a confirmed thread is adopted or the user links one.

use serde::{Deserialize, Serialize};

use crate::model::{new_id, now};
use crate::v2::{V2Error, V2Store, dispatch_readiness};

/// The failure stage shown on a Task when dispatch, rather than Briefing or Grilling, failed.
pub const FAILURE_STAGE: &str = "dispatch";

/// Dispatch state inside the Task's existing lifecycle. It is stored on the attempt, never as a
/// new Task status: a bound attempt is the only state that mirrors to `dispatched`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    /// The child process may be starting. No response has been classified yet.
    Launching,
    /// An exact pre-launch rejection. The Brief and approval survive; manual Retry is allowed.
    LaunchFailed,
    /// The outcome is unknown. Another start is forbidden; check again or link an existing thread.
    ReconcileRequired,
    /// A validated thread identity is committed together with the Task's `dispatched` status.
    Bound,
}

impl DispatchState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launching => "launching",
            Self::LaunchFailed => "launch_failed",
            Self::ReconcileRequired => "reconcile_required",
            Self::Bound => "bound",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "launching" => Some(Self::Launching),
            "launch_failed" => Some(Self::LaunchFailed),
            "reconcile_required" => Some(Self::ReconcileRequired),
            "bound" => Some(Self::Bound),
            _ => None,
        }
    }

    /// True while a second start must be refused. A known failure is the only released state.
    pub fn is_open(self) -> bool {
        matches!(
            self,
            Self::Launching | Self::ReconcileRequired | Self::Bound
        )
    }
}

/// The durable attempt written before any child process is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAttempt {
    pub attempt_id: String,
    pub task_id: String,
    pub project_id: String,
    /// The T3 selector in `id:<id>` or `path:<absolute path>` storage form.
    pub project_selector: String,
    /// The T3 data directory resolved once for this attempt and passed as `--base-dir`.
    pub t3_data_dir: String,
    /// The Task line frozen at claim time.
    pub frozen_line: String,
    pub brief_revision: i64,
    pub base_ref: Option<String>,
    /// Neutral correlation title, `Relay Task <attempt ULID>`.
    pub title: String,
    pub state: DispatchState,
    pub thread_id: Option<String>,
    pub run_id: Option<String>,
    pub thread_url: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl DispatchAttempt {
    /// A linked thread belongs to this Relay project and T3 profile only when both frozen fields
    /// still match. Manual linking reuses the same check.
    pub fn matches_profile(&self, selector: &str, data_dir: &str) -> bool {
        self.project_selector == selector && self.t3_data_dir == data_dir
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchStart {
    pub task_id: String,
    /// Resolved `T3CODE_HOME` or the Windows default, frozen for this attempt.
    pub data_dir: String,
    /// Explicit worktree base; `None` uses the T3 project defaults.
    pub base_ref: Option<String>,
}

/// The frozen inputs passed to the CLI adapter. The Brief is never reconstructed from the live
/// Task, so a later edit cannot change what the running thread received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchClaim {
    pub attempt: DispatchAttempt,
    pub brief: String,
    pub selector: String,
}

/// A thread id may arrive before the rest of its identity. Saving it fences the Task immediately;
/// the full identity and `dispatched` are committed together by [`bind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadIdentity {
    pub thread_id: String,
    pub run_id: String,
    pub link: String,
    pub project_id: String,
}

impl ThreadIdentity {
    pub fn is_complete(&self) -> bool {
        !self.thread_id.trim().is_empty()
            && !self.run_id.trim().is_empty()
            && !self.link.trim().is_empty()
            && !self.project_id.trim().is_empty()
    }
}

/// A row from `t3 thread list --json`. Only the fields needed for exact attempt-title matching
/// and project verification are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedThread {
    pub thread_id: String,
    pub title: Option<String>,
    pub project_id: Option<String>,
}

/// What a Task sheet may offer after dispatch was attempted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchView {
    pub task_id: String,
    pub state: DispatchState,
    pub attempt_id: String,
    pub thread_id: Option<String>,
    pub thread_url: Option<String>,
    pub failure_reason: Option<String>,
    pub can_retry: bool,
    pub can_check_again: bool,
    pub can_link_existing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRecovery {
    /// Open attempts from a previous process; they must be reconciled, never restarted blindly.
    pub reconcile: Vec<DispatchAttempt>,
    /// Ready Tasks that never reserved an attempt and can be started fresh.
    pub start: Vec<String>,
}

/// Atomically recheck every gate and reserve one attempt. `None` means the Task is not ready, its
/// mapping is missing, or an attempt or thread already owns it. No child process is started here.
pub fn claim(store: &V2Store, start: &DispatchStart) -> Result<Option<DispatchClaim>, V2Error> {
    let Some(readiness) = dispatch_readiness(store, &start.task_id)? else {
        return Ok(None);
    };
    // A ready Task with no T3 mapping is a Settings problem, not a dispatch. It stays ready so the
    // user can map the project and start again; no attempt is reserved.
    let Some(target) = store.t3_target(&readiness.project_id)? else {
        return Ok(None);
    };
    let attempt_id = new_id();
    let title = format!("Relay Task {attempt_id}");
    let attempt = DispatchAttempt {
        attempt_id,
        task_id: start.task_id.clone(),
        project_id: readiness.project_id.clone(),
        project_selector: target.to_storage(),
        t3_data_dir: start.data_dir.trim().to_string(),
        frozen_line: task_line(store, &start.task_id)?,
        brief_revision: readiness.brief_revision,
        base_ref: start
            .base_ref
            .clone()
            .filter(|value| !value.trim().is_empty()),
        title,
        state: DispatchState::Launching,
        thread_id: None,
        run_id: None,
        thread_url: None,
        failure_reason: None,
        created_at: now(),
        updated_at: now(),
    };
    let saved = store.claim_dispatch(&attempt, &readiness)?;
    Ok(saved.map(|attempt| DispatchClaim {
        attempt,
        brief: readiness.rendered_brief,
        selector: target.value().to_string(),
    }))
}

fn task_line(store: &V2Store, task_id: &str) -> Result<String, V2Error> {
    Ok(store.task_line_state(task_id)?.line_text)
}

/// Stage Retry reuses the same claim path. A known failure has released the slot; an unresolved
/// attempt is still fenced by the store's unique index, so Retry never produces a second start.
pub fn retry(store: &V2Store, start: &DispatchStart) -> Result<Option<DispatchClaim>, V2Error> {
    claim(store, start)
}

/// Records an exact, verified pre-launch rejection. The Brief, its approval and the Grilling
/// transcript are untouched, so only the dispatch stage needs another attempt.
pub fn record_known_failure(
    store: &V2Store,
    attempt_id: &str,
    reason: &str,
) -> Result<Option<DispatchAttempt>, V2Error> {
    store.finish_dispatch(
        attempt_id,
        &[DispatchState::Launching],
        DispatchState::LaunchFailed,
        None,
        Some(sanitize_reason(reason)),
        TaskStatus::Failed,
        Some("Retry dispatch after setup recovers.".to_string()),
    )
}

/// Records a possibly successful start whose result was lost, ambiguous or malformed. The Task
/// stays undispatched and offers Check again and Link existing thread.
pub fn record_uncertain(
    store: &V2Store,
    attempt_id: &str,
    reason: &str,
) -> Result<Option<DispatchAttempt>, V2Error> {
    store.finish_dispatch(
        attempt_id,
        &[DispatchState::Launching, DispatchState::ReconcileRequired],
        DispatchState::ReconcileRequired,
        None,
        Some(sanitize_reason(reason)),
        TaskStatus::Failed,
        Some("Check again or link the existing T3 thread.".to_string()),
    )
}

/// Saves a validated thread id as soon as it arrives, before the rest of the identity is hydrated.
/// The attempt moves to `reconcile_required`, which keeps another start fenced.
pub fn save_thread_id(
    store: &V2Store,
    attempt_id: &str,
    thread_id: &str,
) -> Result<Option<DispatchAttempt>, V2Error> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err(V2Error::InvalidInput(
            "the returned thread id is empty".into(),
        ));
    }
    store.save_dispatch_thread_id(attempt_id, thread_id)
}

/// Commits a complete thread identity and the Task's `dispatched` status in one transaction. An
/// incomplete identity is rejected; callers hydrate missing fields with read-only `show` first.
pub fn bind(
    store: &V2Store,
    attempt_id: &str,
    identity: &ThreadIdentity,
) -> Result<Option<DispatchAttempt>, V2Error> {
    if !identity.is_complete() {
        return Err(V2Error::InvalidInput(
            "the thread identity is incomplete; hydrate it with show before binding".into(),
        ));
    }
    let Some(attempt) = store.dispatch_attempt(attempt_id)? else {
        return Ok(None);
    };
    if !identity_matches(&attempt, identity) {
        return Err(V2Error::InvalidInput(
            "that thread does not belong to this project and T3 profile".into(),
        ));
    }
    store.finish_dispatch(
        attempt_id,
        &[DispatchState::Launching, DispatchState::ReconcileRequired],
        DispatchState::Bound,
        Some(identity),
        None,
        TaskStatus::Dispatched,
        None,
    )
}

/// A response that arrives after local cancellation still records the thread link. Cancellation
/// only stops the UI wait; it never discards a validated identity.
pub fn bind_after_cancel(
    store: &V2Store,
    attempt_id: &str,
    identity: &ThreadIdentity,
) -> Result<Option<DispatchAttempt>, V2Error> {
    bind(store, attempt_id, identity)
}

/// Startup pass. Attempts left in `launching` become uncertain because a child may have started;
/// they are returned for read-only reconciliation. Tasks with no attempt at all are fresh starts.
pub fn recover(store: &V2Store) -> Result<DispatchRecovery, V2Error> {
    store.mark_launches_unresolved()?;
    let reconcile = store.open_dispatch_attempts()?;
    let start = store.tasks_without_dispatch()?;
    Ok(DispatchRecovery { reconcile, start })
}

/// The single title-matching rule. Exactly one exact match is required; zero or several keeps the
/// Task fenced. Project identity is checked later, from `show`, not from the list row.
pub fn unique_candidate<'a>(
    attempt: &DispatchAttempt,
    candidates: &'a [ListedThread],
) -> Option<&'a ListedThread> {
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate.title.as_deref() == Some(attempt.title.as_str()));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

/// Verifies that a `show` result belongs to the claimed attempt. The thread id must match the
/// candidate and the project must still be the frozen selector.
pub fn identity_matches(attempt: &DispatchAttempt, identity: &ThreadIdentity) -> bool {
    identity.is_complete()
        && attempt
            .thread_id
            .as_deref()
            .is_none_or(|saved| saved == identity.thread_id)
        && selector_matches_project(&attempt.project_selector, &identity.project_id)
}

fn selector_matches_project(selector: &str, project_id: &str) -> bool {
    match crate::v2::T3Target::from_storage(selector) {
        Some(crate::v2::T3Target::Id(expected)) => expected == project_id,
        // A path selector cannot be proved from a project id alone. `show` must return the same
        // project id for every candidate; the adapter compares candidates with one another and
        // requires a single verified match before calling `bind`.
        Some(crate::v2::T3Target::Path(_)) => !project_id.trim().is_empty(),
        None => false,
    }
}

/// Manual recovery for an edited title or an unavailable launch field. The adapter has already
/// used read-only `show` to confirm project and profile; `user_confirmed` records that the user
/// compared the thread's first message with the frozen Brief in T3.
pub fn link_existing(
    store: &V2Store,
    attempt: &DispatchAttempt,
    identity: &ThreadIdentity,
    selector: &str,
    data_dir: &str,
    user_confirmed: bool,
) -> Result<Option<DispatchAttempt>, V2Error> {
    if !user_confirmed {
        return Err(V2Error::InvalidInput(
            "compare the thread's first message with the Brief before linking".into(),
        ));
    }
    if !attempt.matches_profile(selector, data_dir) || !identity_matches(attempt, identity) {
        return Err(V2Error::InvalidInput(
            "that thread does not belong to this project and T3 profile".into(),
        ));
    }
    // The unique thread index rejects a thread bound to another Task; map that to a clear error.
    store.finish_dispatch(
        &attempt.attempt_id,
        &[DispatchState::ReconcileRequired],
        DispatchState::Bound,
        Some(identity),
        None,
        TaskStatus::Dispatched,
        None,
    )
}

/// The read model for a Task sheet. Buttons are derived from the stored state, never from a live
/// CLI probe, so the UI cannot accidentally unlock a second start.
pub fn view(store: &V2Store, task_id: &str) -> Result<Option<DispatchView>, V2Error> {
    let Some(attempt) = store.latest_dispatch_attempt(task_id)? else {
        return Ok(None);
    };
    Ok(Some(DispatchView {
        task_id: attempt.task_id.clone(),
        state: attempt.state,
        attempt_id: attempt.attempt_id.clone(),
        thread_id: attempt.thread_id.clone(),
        thread_url: attempt.thread_url.clone(),
        failure_reason: attempt.failure_reason.clone(),
        can_retry: attempt.state == DispatchState::LaunchFailed,
        can_check_again: attempt.state == DispatchState::ReconcileRequired,
        can_link_existing: attempt.state == DispatchState::ReconcileRequired,
    }))
}

use crate::v2::TaskStatus;

fn sanitize_reason(reason: &str) -> String {
    let one_line: String = reason
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect();
    if one_line.is_empty() {
        "T3 dispatch failed.".to_string()
    } else {
        one_line
    }
}

#[cfg(test)]
mod tests;
