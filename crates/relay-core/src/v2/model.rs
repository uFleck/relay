//! Typed contract for the v2 store. Requests are written by the implementing agent
//! (#25 and later); responses are camelCase for Tauri IPC and the Composer.

use serde::{Deserialize, Serialize};

/// Dispatch gate from the Composer's leading sigil (`!` act, `?` wait, `??` grill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Act,
    Wait,
    Grill,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Act => "act",
            Self::Wait => "wait",
            Self::Grill => "grill",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "act" => Some(Self::Act),
            "wait" => Some(Self::Wait),
            "grill" => Some(Self::Grill),
            _ => None,
        }
    }
}

/// The fixed v2 lifecycle. The store owns the transition table; later views own progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Briefing,
    Grilling,
    NeedsProject,
    AwaitingApproval,
    Dispatched,
    NeedsAnswer,
    Returned,
    Failed,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Briefing => "briefing",
            Self::Grilling => "grilling",
            Self::NeedsProject => "needs_project",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Dispatched => "dispatched",
            Self::NeedsAnswer => "needs_answer",
            Self::Returned => "returned",
            Self::Failed => "failed",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "briefing" => Some(Self::Briefing),
            "grilling" => Some(Self::Grilling),
            "needs_project" => Some(Self::NeedsProject),
            "awaiting_approval" => Some(Self::AwaitingApproval),
            "dispatched" => Some(Self::Dispatched),
            "needs_answer" => Some(Self::NeedsAnswer),
            "returned" => Some(Self::Returned),
            "failed" => Some(Self::Failed),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// How a Note write is addressed. Create is idempotent on `submission_key`; Edit is
/// guarded by the revision the caller read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteWrite {
    Create {
        submission_key: String,
    },
    Edit {
        note_id: String,
        expected_revision: i64,
    },
}

/// A Reference the caller has resolved (or only has a link for).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedReference {
    pub kind: String,
    pub target_id: String,
    pub canonical_url: String,
    /// Read-only cached snapshot for the Lens. `None` keeps a link-only Reference.
    pub snapshot_json: Option<String>,
}

/// The intent carried by one directive line. `original_line` is taken from the line
/// text at Task creation and is never rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedTaskIntent {
    pub agent_mode: AgentMode,
    /// A registered v2 project id, or `None` to wait for the user to pick one.
    pub project_id: Option<String>,
}

/// One editable line. `line_id` is `None` for a new line and the existing immutable id
/// otherwise. A line omitted from an Edit keeps its row with a null position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedLine {
    pub line_id: Option<String>,
    pub text: String,
    pub references: Vec<PreparedReference>,
    pub task_intent: Option<PreparedTaskIntent>,
}

/// The structured write #25 will build from the Composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedNoteWrite {
    pub write: NoteWrite,
    pub source: String,
    pub lines: Vec<PreparedLine>,
}

/// A registered project. `owner_repo` is stored lowercase and is unique when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Project {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub repo_path: Option<String>,
    pub owner_repo: Option<String>,
    pub t3_target: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewV2Project {
    pub name: String,
    pub slug: String,
    pub repo_path: Option<String>,
    pub owner_repo: Option<String>,
    pub t3_target: Option<String>,
}

/// Import settings. Both toggles start off and the scope starts unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Settings {
    pub import_clickup_enabled: bool,
    pub import_github_enabled: bool,
    pub import_scope: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2SettingsUpdate {
    pub import_clickup_enabled: bool,
    pub import_github_enabled: bool,
    pub import_scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Reference {
    pub id: String,
    pub kind: String,
    pub target_id: String,
    pub canonical_url: String,
    pub snapshot_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2NoteLine {
    pub id: String,
    pub text: String,
    pub position: Option<i64>,
    pub references: Vec<V2Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Task {
    pub id: String,
    pub source_line_id: String,
    /// Derived through the source line, so removed Task-bearing lines stay addressable.
    pub note_id: String,
    pub original_line: String,
    pub agent_mode: AgentMode,
    pub status: TaskStatus,
    pub project_id: Option<String>,
    pub brief: Option<String>,
    pub references: Vec<V2Reference>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Note {
    pub id: String,
    pub revision: i64,
    pub source: String,
    /// Newline-joined line text: the plain body projection stored for search.
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    /// Active lines only; removed Task-bearing lines are excluded.
    pub lines: Vec<V2NoteLine>,
    /// Every Task of the Note, including Tasks whose source line was removed.
    pub tasks: Vec<V2Task>,
}

/// `save_note` returns the full aggregate so the caller needs no second read.
pub type SavedNote = V2Note;

/// Everything the first v2 IPC read returns in one call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2NotebookSnapshot {
    pub notes: Vec<V2Note>,
    pub projects: Vec<V2Project>,
    pub settings: V2Settings,
}

/// The one read the v2 workspace consumes. Notes stay in timeline order and `tasks` is a flat
/// projection of every Task, including Tasks whose source line was removed from its Note.
/// A selected Task must resolve from this unfiltered set so a filter cannot hide its sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2WorkspaceSnapshot {
    pub notes: Vec<V2Note>,
    pub tasks: Vec<V2Task>,
    pub projects: Vec<V2Project>,
    pub settings: V2Settings,
}

/// Ids from the shared filter path. The caller already owns the workspace snapshot, so it
/// renders matched Notes with their full source-line context and Tasks from their saved fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSearchResult {
    /// The raw query this result answers, so a caller can drop a slow stale response.
    pub query: String,
    pub note_ids: Vec<String>,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewImportRecord {
    pub source_kind: String,
    pub target_id: String,
    pub note_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    pub source_kind: String,
    pub target_id: String,
    pub note_id: Option<String>,
    pub created_at: String,
}
