//! The local model. Serde is camelCase for the frontend and MCP; ids are ULIDs as strings.
//!
//! Fields mirror the SQLite columns in `migrations/`. Types stay plain
//! (`String`, `Option<String>`, `i64`) so a row round-trips without conversion layers.
//! `New*` structs are the service inputs; `*View` structs are composites returned to
//! adapters (IPC/MCP) so they never have to re-query the store.

use serde::{Deserialize, Serialize};

/// A new local identifier. ULIDs sort by creation time.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The one timestamp format in the database: fixed-width RFC 3339 UTC with
/// milliseconds. Fixed width matters because leases and `updated_since` are
/// compared as text in SQLite.
fn format_timestamp(value: time::OffsetDateTime) -> String {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .unwrap_or_default()
}

/// UTC now as fixed-width RFC 3339, the format stored in SQLite.
pub fn now() -> String {
    format_timestamp(time::OffsetDateTime::now_utc())
}

/// UTC now shifted by `minutes`, for claim leases.
pub fn now_plus_minutes(minutes: i64) -> String {
    format_timestamp(time::OffsetDateTime::now_utc() + time::Duration::minutes(minutes))
}

/// Normalizes any RFC 3339 value (including an offset) to the stored format.
pub fn format_time(value: time::OffsetDateTime) -> String {
    format_timestamp(value)
}

/// Parses an RFC 3339 timestamp, the only accepted user-supplied time format.
pub fn parse_time(value: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

/// True when `value` is in the past or cannot be parsed. Used for lease checks,
/// where a malformed timestamp must count as expired, never as valid.
pub fn is_past(value: &str) -> bool {
    parse_time(value).is_none_or(|parsed| parsed <= time::OffsetDateTime::now_utc())
}

/// The eight fixed status categories. A project status row maps to exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusCategory {
    Backlog,
    Todo,
    Active,
    Review,
    Merged,
    Deploying,
    Done,
    Blocked,
}

impl StatusCategory {
    pub const ALL: [Self; 8] = [
        Self::Backlog,
        Self::Todo,
        Self::Active,
        Self::Review,
        Self::Merged,
        Self::Deploying,
        Self::Done,
        Self::Blocked,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Review => "review",
            Self::Merged => "merged",
            Self::Deploying => "deploying",
            Self::Done => "done",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }
}

impl std::fmt::Display for StatusCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a task change is. Requirements are `ChangeKind::Requirement` (PLAN.md section 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Requirement,
    NewTask,
    ChangeScope,
    ChangeDeadline,
    ChangePriority,
    ReportStatus,
    Decision,
    NoChange,
}

impl ChangeKind {
    pub const ALL: [Self; 8] = [
        Self::Requirement,
        Self::NewTask,
        Self::ChangeScope,
        Self::ChangeDeadline,
        Self::ChangePriority,
        Self::ReportStatus,
        Self::Decision,
        Self::NoChange,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requirement => "requirement",
            Self::NewTask => "new_task",
            Self::ChangeScope => "change_scope",
            Self::ChangeDeadline => "change_deadline",
            Self::ChangePriority => "change_priority",
            Self::ReportStatus => "report_status",
            Self::Decision => "decision",
            Self::NoChange => "no_change",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

impl std::fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Changes are append-only; retraction and supersession keep history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeState {
    Active,
    Retracted,
    Superseded,
}

impl ChangeState {
    pub const ALL: [Self; 3] = [Self::Active, Self::Retracted, Self::Superseded];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retracted => "retracted",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }
}

impl std::fmt::Display for ChangeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub repo_path: Option<String>,
    pub color: Option<String>,
    pub integrations_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub category: StatusCategory,
    pub position: i64,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub status_id: String,
    pub title: String,
    pub notes: String,
    pub priority: Option<i64>,
    pub due_at: Option<String>,
    pub position: i64,
    pub source: String,
    pub rev: i64,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub id: String,
    pub task_id: String,
    pub holder_kind: String,
    pub holder_name: String,
    pub thread_ref: Option<String>,
    pub session_ref: Option<String>,
    pub lease_expires_at: String,
    pub released_at: Option<String>,
    /// `completed`, `blocked`, `abandoned` or `expired` once released.
    pub outcome: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskChange {
    pub id: String,
    pub task_id: String,
    pub claim_id: Option<String>,
    pub kind: ChangeKind,
    pub body: String,
    pub excerpt: Option<String>,
    pub permalink: Option<String>,
    pub source_kind: String,
    pub source_id: Option<String>,
    pub confidence: Option<f64>,
    pub state: ChangeState,
    pub created_at: String,
    pub acked_at: Option<String>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: String,
    pub task_id: String,
    pub kind: String,
    pub actor: Option<String>,
    pub actor_kind: Option<String>,
    pub body: Option<String>,
    pub data_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLink {
    pub id: String,
    pub task_id: String,
    /// `slack`, `t3`, `pr`, `branch`, `commit`, `clickup`, `github` or `url`.
    pub kind: String,
    pub external_id: Option<String>,
    pub url: String,
    pub title: Option<String>,
    pub state: Option<String>,
    pub metadata_json: String,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub channel_id: String,
    pub peer_id: String,
    pub peer_name: Option<String>,
    pub episode_no: i64,
    pub state: String,
    pub opened_at: String,
    pub last_message_at: String,
    pub closed_at: Option<String>,
    pub jev_json: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub ts: String,
    pub author_id: String,
    pub author_name: Option<String>,
    pub direction: String,
    pub text: String,
    pub thread_ts: Option<String>,
    pub permalink: Option<String>,
    pub is_bot: bool,
    pub latest_reply: Option<String>,
    pub raw_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub job_id: Option<String>,
    pub project_id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub base_rev: Option<i64>,
    pub patch_json: String,
    pub rationale: Option<String>,
    pub jev_json: Option<String>,
    pub confidence: Option<f64>,
    pub state: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub applied_event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisJob {
    pub id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub watermark: String,
    pub kind: String,
    pub state: String,
    pub attempts: i64,
    pub jev_json: Option<String>,
    pub agent_session: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Service inputs. Optional fields are absence, never "clear": clearing a task
// field is spelled `null` in `TaskPatch` (see the double option there).
// ---------------------------------------------------------------------------

/// `create_project_with_default_workflow(name, slug, repo_path?)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProject {
    pub name: String,
    pub slug: String,
    pub repo_path: Option<String>,
    pub color: Option<String>,
    /// JSON object; defaults to `{}`.
    pub integrations_json: Option<String>,
}

impl NewProject {
    pub fn new(name: &str, slug: &str, repo_path: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            slug: slug.to_string(),
            repo_path: repo_path.map(str::to_string),
            color: None,
            integrations_json: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWithStatuses {
    pub project: Project,
    pub statuses: Vec<ProjectStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    pub project_id: String,
    /// Defaults to the project's first `todo` status, else its first status.
    pub status_id: Option<String>,
    pub title: String,
    pub notes: Option<String>,
    pub priority: Option<i64>,
    pub due_at: Option<String>,
    pub position: Option<i64>,
    /// Defaults to `local`.
    pub source: Option<String>,
    /// JSON object; defaults to `{}`.
    pub metadata_json: Option<String>,
}

impl NewTask {
    pub fn new(project_id: &str, title: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            status_id: None,
            title: title.to_string(),
            notes: None,
            priority: None,
            due_at: None,
            position: None,
            source: None,
            metadata_json: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskFilter {
    pub project_id: Option<String>,
    pub status_id: Option<String>,
    pub category: Option<StatusCategory>,
    /// Case-insensitive substring over title and notes. `search_tasks` is the FTS path.
    pub text: Option<String>,
    pub updated_since: Option<String>,
    pub include_archived: bool,
    /// Defaults to 100, capped at 1000.
    pub limit: Option<usize>,
}

/// Task field changes. An absent field is unchanged; `priority: null` and
/// `due_at: null` clear the column (deserialized as `Some(None)`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_some"
    )]
    pub priority: Option<Option<i64>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_some"
    )]
    pub due_at: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
}

impl TaskPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.notes.is_none()
            && self.priority.is_none()
            && self.due_at.is_none()
            && self.status_id.is_none()
            && self.position.is_none()
            && self.archived.is_none()
            && self.metadata_json.is_none()
    }
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewChange {
    pub task_id: String,
    /// The claim recording this change, when an agent wrote it. Extends that lease.
    pub claim_id: Option<String>,
    pub kind: ChangeKind,
    pub body: String,
    pub excerpt: Option<String>,
    pub permalink: Option<String>,
    /// `slack`, `jev`, `opencode`, `t3`, `human`, `system`, ...
    pub source_kind: String,
    pub source_id: Option<String>,
    pub confidence: Option<f64>,
}

impl NewChange {
    pub fn requirement(task_id: &str, body: &str, source_kind: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            claim_id: None,
            kind: ChangeKind::Requirement,
            body: body.to_string(),
            excerpt: None,
            permalink: None,
            source_kind: source_kind.to_string(),
            source_id: None,
            confidence: None,
        }
    }
}

/// A change as seen by one claim (or by a human when `claim_id` is `None`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeView {
    pub change: TaskChange,
    /// The scope the `acked` flag refers to. `None` is the human view.
    pub claim_id: Option<String>,
    /// Acked for `claim_id`; for a human view, acked by anyone.
    pub acked: bool,
    /// Acked by someone else but not for this claim. A new claimant re-acks.
    pub acked_elsewhere: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckResult {
    pub change: ChangeView,
    /// Still unacked for the same scope, so one response is enough.
    pub unacked_changes: Vec<ChangeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewClaim {
    pub task_id: String,
    /// `agent`, `human` or `system`.
    pub holder_kind: String,
    pub holder_name: String,
    pub thread_ref: Option<String>,
    pub session_ref: Option<String>,
}

impl NewClaim {
    pub fn new(task_id: &str, holder_kind: &str, holder_name: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            holder_kind: holder_kind.to_string(),
            holder_name: holder_name.to_string(),
            thread_ref: None,
            session_ref: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimView {
    pub claim: Claim,
    /// Active changes this claim has not acked. Empty on a fresh unclaimed task.
    pub unacked_changes: Vec<ChangeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdate {
    pub claim: Claim,
    pub event: TaskEvent,
    pub unacked_changes: Vec<ChangeView>,
}

/// Everything the task side sheet needs in one read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    pub task: Task,
    pub status: ProjectStatus,
    pub changes: Vec<TaskChange>,
    pub links: Vec<TaskLink>,
    pub events: Vec<TaskEvent>,
    pub active_claim: Option<Claim>,
    /// Scoped to the `claim_id` passed to `get_task_detail`; the human queue when `None`.
    pub unacked_changes: Vec<ChangeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewLink {
    pub task_id: String,
    /// The claim attaching the link; extends that lease when present.
    pub claim_id: Option<String>,
    pub kind: String,
    pub external_id: Option<String>,
    pub url: String,
    pub title: Option<String>,
    pub state: Option<String>,
    /// JSON object; defaults to `{}`.
    pub metadata_json: Option<String>,
    pub last_synced_at: Option<String>,
}

impl NewLink {
    pub fn new(task_id: &str, kind: &str, url: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            claim_id: None,
            kind: kind.to_string(),
            external_id: None,
            url: url.to_string(),
            title: None,
            state: None,
            metadata_json: None,
            last_synced_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewEvent {
    pub task_id: String,
    pub kind: String,
    pub actor: Option<String>,
    pub actor_kind: Option<String>,
    pub body: Option<String>,
    /// JSON object; defaults to `{}`.
    pub data_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProposal {
    pub job_id: Option<String>,
    pub project_id: String,
    /// Field-change proposals need the task and its `base_rev` to apply safely.
    pub task_id: Option<String>,
    pub kind: String,
    pub base_rev: Option<i64>,
    /// JSON object of `TaskPatch` fields; unknown fields are ignored on apply.
    pub patch: serde_json::Value,
    pub rationale: Option<String>,
    pub jev_json: Option<String>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    pub proposal: Proposal,
    /// The parsed `patch_json`, for review UI; `null` when it cannot be parsed.
    pub patch: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCursor {
    pub connector: String,
    pub scope: String,
    pub cursor: Option<String>,
    pub last_synced_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_fixed_width_rfc3339() {
        let value = now();
        assert_eq!(value.len(), 24, "{value}");
        assert!(value.ends_with('Z'));
        assert!(parse_time(&value).is_some());

        // An offset input normalizes to UTC, so text comparison stays valid.
        let normalized = format_time(parse_time("2026-11-17T13:00:00+01:00").unwrap());
        assert_eq!(normalized, "2026-11-17T12:00:00.000Z");

        assert!(is_past("2000-01-01T00:00:00Z"));
        assert!(!is_past(&now_plus_minutes(30)));
        assert!(is_past("not a timestamp"));
    }

    #[test]
    fn task_patch_distinguishes_absent_from_null() {
        let absent: TaskPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.priority, None);
        assert!(absent.is_empty());

        let clear: TaskPatch = serde_json::from_str(r#"{"priority":null,"dueAt":null}"#).unwrap();
        assert_eq!(clear.priority, Some(None));
        assert_eq!(clear.due_at, Some(None));
        assert!(!clear.is_empty());

        let set: TaskPatch =
            serde_json::from_str(r#"{"priority":3,"dueAt":"2026-11-18T09:00:00Z"}"#).unwrap();
        assert_eq!(set.priority, Some(Some(3)));
        assert_eq!(set.due_at, Some(Some("2026-11-18T09:00:00Z".to_string())));
    }

    #[test]
    fn status_categories_round_trip() {
        for category in StatusCategory::ALL {
            assert_eq!(StatusCategory::parse(category.as_str()), Some(category));
            let json = serde_json::to_string(&category).unwrap();
            assert_eq!(json, format!("\"{}\"", category.as_str()));
        }
        assert_eq!(StatusCategory::parse("nope"), None);
    }

    #[test]
    fn models_are_camel_case() {
        let task = Task {
            id: "t".into(),
            project_id: "p".into(),
            status_id: "s".into(),
            title: "Deploy".into(),
            notes: String::new(),
            priority: None,
            due_at: None,
            position: 0,
            source: "local".into(),
            rev: 1,
            archived: false,
            created_at: now(),
            updated_at: now(),
            completed_at: None,
            metadata_json: "{}".into(),
        };
        let json = serde_json::to_value(&task).unwrap();
        assert!(json.get("projectId").is_some());
        assert!(json.get("metadataJson").is_some());
        assert!(json.get("project_id").is_none());
    }
}
