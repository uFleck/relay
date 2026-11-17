//! The local model. Serde is camelCase for the frontend and MCP; ids are ULIDs as strings.
//!
//! Fields mirror the SQLite columns in `migrations/0001_init.sql`. Types stay plain
//! (`String`, `Option<String>`, `i64`) so a row round-trips without conversion layers.

use serde::{Deserialize, Serialize};

/// A new local identifier. ULIDs sort by creation time.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// UTC now as RFC 3339, the format stored in SQLite.
pub fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
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
        Self::ALL.into_iter().find(|category| category.as_str() == value)
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
    /// `completed`, `blocked` or `abandoned` once released.
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
