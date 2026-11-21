//! Observed T3 progress and continuation policy for #37. relay-core owns the lifecycle rules;
//! the Tauri adapter only decodes `thread show`/`thread send` output and runs the CLI. Nothing here
//! starts a process or touches SQLite directly: the store owns the transition transaction.
//!
//! An observation is attributed to a run before it can change a Task. Active run states always show
//! progress, but a terminal result, a reply or a failure needs the run Relay expects (or an assistant
//! message on the observed run when no baseline exists). A read for an older run may still surface a
//! pending question; it can never return or fail the Task for a newer run. A lost or unknown status
//! preserves the last known state.

use serde::{Deserialize, Serialize};

use crate::v2::TaskStatus;

/// Decodes a stored lifecycle status for an adapter. The store's own parser stays crate-private.
pub fn task_status(value: &str) -> Option<crate::v2::TaskStatus> {
    crate::v2::TaskStatus::parse(value)
}

/// The failure stage shown when a T3 run, rather than a stage before dispatch, failed.
pub const FAILURE_STAGE: &str = "t3-run";
pub const RETRY_MESSAGE: &str = "Please retry the last request.";

/// The run states the pinned CLI reports, plus `idle` for a thread with no active run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Preparing,
    Queued,
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
    RolledBack,
    Idle,
    Unknown(String),
}

impl RunStatus {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "preparing" => Self::Preparing,
            "queued" => Self::Queued,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            "cancelled" | "canceled" => Self::Cancelled,
            "rolled_back" | "rolledback" => Self::RolledBack,
            "idle" => Self::Idle,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Preparing => "preparing",
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::RolledBack => "rolled_back",
            Self::Idle => "idle",
            Self::Unknown(raw) => raw,
        }
    }

    /// preparing, queued, starting, running and waiting all display Dispatched.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Queued | Self::Starting | Self::Running | Self::Waiting
        )
    }

    /// A terminal result stops polling. Cancelled and rolled_back are user-actionable failures.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Interrupted | Self::Cancelled | Self::RolledBack
        )
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

/// One message from a bounded `thread show` view. Text and role are all the policy reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedMessage {
    pub role: String,
    pub run_id: String,
    pub text: String,
}

/// A PR T3 reports as linked. State is a read-only snapshot; a merge never closes a Task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedPullRequest {
    pub url: String,
    pub source: Option<String>,
    pub state: Option<String>,
    pub watching: bool,
}

/// The typed result of one successful `thread show`. Run, question, branch and PR fields are
/// decoded by the adapter; relay-core never sees raw CLI JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadObservation {
    pub thread_id: String,
    pub active_run_id: Option<String>,
    pub latest_run_id: Option<String>,
    pub status: RunStatus,
    pub pending_request_count: u32,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub link: Option<String>,
    pub project_id: Option<String>,
    pub run_count: u64,
    pub messages: Vec<ObservedMessage>,
    pub pull_requests: Vec<ObservedPullRequest>,
    pub error: Option<String>,
}

impl ThreadObservation {
    /// The run the thread is currently on, preferring the active run over the latest one.
    pub fn observed_run_id(&self) -> Option<&str> {
        self.active_run_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .or_else(|| self.latest_run_id.as_deref().filter(|id| !id.is_empty()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendKind {
    Followup,
    Retry,
    Resend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendState {
    /// Reserved in SQLite; the CLI call has not been acknowledged yet.
    Reserved,
    /// The CLI confirmed a run id; Relay is observing that run.
    Acknowledged,
    /// The response was lost. Sends are fenced until `show` confirms the exact message.
    NeedsConfirmation,
    /// A known pre-send rejection. The draft is retained for an explicit retry.
    Failed,
    /// Superseded by a later operation or by local closure.
    Stale,
}

/// A reserved continuation. The message is frozen before the CLI runs and is never rebuilt from the
/// live Task, so a Follow-up stays the user's exact multiline text on Retry and across a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOperation {
    pub operation_id: String,
    pub attempt_id: String,
    pub task_id: String,
    pub thread_id: String,
    pub kind: SendKind,
    pub state: SendState,
    pub message: String,
    pub baseline_run_id: Option<String>,
    pub baseline_run_count: u64,
    pub run_id: Option<String>,
    pub delivery_state: Option<String>,
    pub failure_reason: Option<String>,
    pub revision: i64,
}

impl SendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Followup => "followup",
            Self::Retry => "retry",
            Self::Resend => "resend",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "followup" => Some(Self::Followup),
            "retry" => Some(Self::Retry),
            "resend" => Some(Self::Resend),
            _ => None,
        }
    }
}

impl SendState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Acknowledged => "acknowledged",
            Self::NeedsConfirmation => "needs_confirmation",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "reserved" => Some(Self::Reserved),
            "acknowledged" => Some(Self::Acknowledged),
            "needs_confirmation" => Some(Self::NeedsConfirmation),
            "failed" => Some(Self::Failed),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

impl SendOperation {
    pub fn is_open(&self) -> bool {
        matches!(
            self.state,
            SendState::Reserved | SendState::Acknowledged | SendState::NeedsConfirmation
        )
    }
}

/// A Task status an observation wants to commit. `None` means preserve the last known state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEffect {
    pub status: TaskStatus,
    pub pending_action: Option<String>,
    pub failure_stage: Option<String>,
    pub failure_reason: Option<String>,
}

/// The inputs that decide whether a read may change a Task. The store rechecks all of them inside
/// its write transaction; a late read with a stale revision is discarded.
#[derive(Debug, Clone, Copy)]
pub struct ObservationContext<'a> {
    pub current_status: TaskStatus,
    /// The run Relay expects: the acknowledged continuation run or the bound start run.
    pub expected_run_id: Option<&'a str>,
    pub send: Option<&'a SendOperation>,
    pub observation: &'a ThreadObservation,
}

/// Selects a reply by assistant role and matching run id. With no run id, the last assistant
/// message wins. A plain-text question from the model is not an answer.
pub fn assistant_reply(messages: &[ObservedMessage], run_id: Option<&str>) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role.eq_ignore_ascii_case("assistant")
                && run_id.is_none_or(|expected| message.run_id == expected)
        })
        .map(|message| message.text.clone())
        .filter(|text| !text.trim().is_empty())
}

fn attributable(ctx: &ObservationContext<'_>, observed_run: Option<&str>) -> bool {
    match ctx.expected_run_id {
        Some(expected) => observed_run == Some(expected),
        None => assistant_reply(&ctx.observation.messages, observed_run).is_some(),
    }
}

fn needs_answer() -> ProgressEffect {
    ProgressEffect {
        status: TaskStatus::NeedsAnswer,
        pending_action: Some("Answer the question in T3; Relay keeps watching this thread.".into()),
        failure_stage: None,
        failure_reason: None,
    }
}

fn dispatched() -> ProgressEffect {
    ProgressEffect {
        status: TaskStatus::Dispatched,
        pending_action: Some("T3 is working on this Task.".into()),
        failure_stage: None,
        failure_reason: None,
    }
}

fn returned() -> ProgressEffect {
    ProgressEffect {
        status: TaskStatus::Returned,
        pending_action: Some("Review the reply, follow up or close the Task.".into()),
        failure_stage: None,
        failure_reason: None,
    }
}

fn failure_reason(observation: &ThreadObservation) -> String {
    let reason = observation
        .error
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("T3 did not report a reason.");
    let one_line = reason
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "T3 run {}: {}",
        observation.status.as_str(),
        one_line.chars().take(220).collect::<String>()
    )
}

fn failed(observation: &ThreadObservation) -> ProgressEffect {
    ProgressEffect {
        status: TaskStatus::Failed,
        pending_action: Some("Retry the T3 run, follow up or close the Task.".into()),
        failure_stage: Some(FAILURE_STAGE.to_string()),
        failure_reason: Some(failure_reason(observation)),
    }
}

/// The single observation transition. Active states always move to Dispatched or Needs answer; a
/// terminal state needs run attribution. An acknowledged continuation for a different run keeps the
/// Task in flight while the previous run finishes, except that a pending question is still shown.
pub fn decide(ctx: &ObservationContext<'_>) -> Option<ProgressEffect> {
    let observation = ctx.observation;
    if !observation.status.is_known() {
        return None;
    }
    let observed_run = observation.observed_run_id();
    let matching = attributable(ctx, observed_run);
    let send_acknowledged = ctx
        .send
        .is_some_and(|send| send.state == SendState::Acknowledged);

    if send_acknowledged && !matching {
        // A queued send: the observation belongs to the prior run. Its question is real, but its
        // terminal result must not return or fail the Task for the new run.
        return (observation.pending_request_count > 0 && observation.status.is_active())
            .then(needs_answer);
    }
    if !matching && !observation.status.is_active() {
        return None;
    }

    if observation.status.is_active() {
        return Some(if observation.pending_request_count > 0 {
            needs_answer()
        } else {
            dispatched()
        });
    }
    match observation.status {
        RunStatus::Completed => Some(returned()),
        RunStatus::Idle => assistant_reply(&observation.messages, observed_run).map(|_| returned()),
        RunStatus::Failed
        | RunStatus::Interrupted
        | RunStatus::Cancelled
        | RunStatus::RolledBack => Some(failed(observation)),
        RunStatus::Preparing
        | RunStatus::Queued
        | RunStatus::Starting
        | RunStatus::Running
        | RunStatus::Waiting
        | RunStatus::Unknown(_) => None,
    }
}

/// A queued send is confirmed only when `show` has exactly one run beyond the saved baseline and
/// that run carries the exact user message. A subset of messages is never enough to guess.
pub fn confirms_send(send: &SendOperation, observation: &ThreadObservation) -> Option<String> {
    let baseline = send.baseline_run_count;
    let new_count = observation.run_count;
    if new_count != baseline.saturating_add(1) {
        return None;
    }
    let run_id = observation
        .latest_run_id
        .as_deref()
        .filter(|id| !id.is_empty())?;
    let exact = observation.messages.iter().any(|message| {
        message.role.eq_ignore_ascii_case("user")
            && message.run_id == run_id
            && message.text == send.message
    });
    exact.then(|| run_id.to_string())
}

/// The read model for a Task sheet. Buttons are derived from the stored Task status and operation
/// state, never from a live CLI probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct T3ProgressView {
    pub task_id: String,
    pub attempt_id: String,
    pub thread_id: String,
    pub thread_url: Option<String>,
    pub run_id: Option<String>,
    pub run_count: u64,
    /// The last observed T3 run status, kept even when the Task lifecycle is unchanged.
    pub status: String,
    /// Relay's own lifecycle status, derived from committed observations and explicit closure.
    pub task_status: String,
    pub pending_request_count: u32,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub reply: Option<String>,
    pub error: Option<String>,
    /// Set by a failed read. The last good view is kept; Settings is the recovery path.
    pub stale_reason: Option<String>,
    /// Bumped on every committed observation, failed read, reserved send and closure. A read that
    /// started before the bump is discarded instead of overwriting newer state.
    pub revision: i64,
    pub pull_requests: Vec<ObservedPullRequest>,
    pub active_send: Option<SendOperation>,
    pub can_follow_up: bool,
    pub can_retry_run: bool,
    pub can_close: bool,
    pub can_open_t3: bool,
    pub can_check_again: bool,
    pub can_resend: bool,
}

/// Guards a late observation. A Task that was closed, a thread that was relinked or an operation
/// that was superseded while the read was in flight must not commit stale results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitGuard {
    pub task_id: String,
    pub attempt_id: String,
    pub thread_id: String,
    pub revision: i64,
    pub operation_revision: Option<i64>,
}

/// One Task the polling worker must observe: an open bound Task or one with an unresolved send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressWatch {
    pub task_id: String,
    pub attempt_id: String,
    pub thread_id: String,
    pub data_dir: String,
    pub revision: i64,
    pub send: Option<SendOperation>,
}

#[cfg(test)]
mod tests;
