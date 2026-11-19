//! SQLite store: one writer behind a mutex, migrations through `PRAGMA user_version`.
//!
//! The database is the local source of truth. Everything else (Slack, JEV, GitHub,
//! ClickUp, MCP) reads and writes through these services; adapters stay thin.
//!
//! P1 service surface: projects with the fixed default workflow, tasks with optimistic
//! concurrency (`rev`), append-only changes with per-claim acks, claims with a 30-minute
//! lease, links, events, proposals, settings and sync cursors.
//!
//! Every multi-row write runs in a transaction; claim acquisition and completion use
//! `BEGIN IMMEDIATE`. User data is validated and surfaces as a typed [`StoreError`],
//! never as a panic.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{
    Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter,
    types::Value as SqlValue,
};

use crate::model::{
    self, AckResult, AnalysisJob, ChangeKind, ChangeState, ChangeView, Claim, ClaimView,
    EpisodeDetail, EpisodeSummary, JobOp, NewChange, NewClaim, NewEvent, NewJob, NewLink,
    NewProject, NewProposal, NewTask, ProgressUpdate, Project, ProjectStatus, ProjectWithStatuses,
    Proposal, ProposalDecision, ProposalView, RawSlackMessage, Setting, SlackEpisode, SlackMessage,
    SlackPeer, StatusCategory, SyncCursor, Task, TaskChange, TaskDetail, TaskEvent, TaskFilter,
    TaskLink, TaskPatch, new_id, now, now_plus_minutes,
};
use crate::view;

mod notes;

const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_change_acks.sql"),
    include_str!("../../migrations/0003_slack_jobs.sql"),
    include_str!("../../migrations/0004_conversation_summaries.sql"),
    include_str!("../../migrations/0005_notes.sql"),
];

/// Claim lease in minutes; every claimant write extends it.
const CLAIM_LEASE_MINUTES: i64 = 30;
const DEFAULT_TASK_LIMIT: usize = 100;
const MAX_TASK_LIMIT: usize = 1000;
/// How long a second connection waits for a writer before SQLite reports busy.
/// The claim compare-and-set relies on this: a loser blocks on `BEGIN IMMEDIATE`,
/// then reads the winner's claim instead of surfacing `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// The fixed P1 workflow (docs/PLAN.md section 2). `Deployed to RC` is `deploying`;
/// `Done` is the only `done`.
const DEFAULT_WORKFLOW: [(&str, StatusCategory); 9] = [
    ("Backlog", StatusCategory::Backlog),
    ("Todo", StatusCategory::Todo),
    ("In Progress", StatusCategory::Active),
    ("In Review", StatusCategory::Review),
    ("Merged", StatusCategory::Merged),
    ("Waiting for deploy", StatusCategory::Deploying),
    ("Deployed to RC", StatusCategory::Deploying),
    ("Done", StatusCategory::Done),
    ("Blocked", StatusCategory::Blocked),
];

const TASK_COLUMNS: &str = "id, project_id, status_id, title, notes, priority, due_at, position, \
     source, rev, archived, created_at, updated_at, completed_at, metadata_json";
const PROJECT_COLUMNS: &str =
    "id, name, slug, repo_path, color, integrations_json, created_at, updated_at";
const STATUS_COLUMNS: &str = "id, project_id, name, category, position, color";
const CLAIM_COLUMNS: &str = "id, task_id, holder_kind, holder_name, thread_ref, session_ref, \
     lease_expires_at, released_at, outcome, created_at";
const CHANGE_COLUMNS: &str = "id, task_id, claim_id, kind, body, excerpt, permalink, source_kind, \
     source_id, confidence, state, created_at, acked_at, superseded_by";
const EVENT_COLUMNS: &str = "id, task_id, kind, actor, actor_kind, body, data_json, created_at";
const LINK_COLUMNS: &str =
    "id, task_id, kind, external_id, url, title, state, metadata_json, last_synced_at";
const PROPOSAL_COLUMNS: &str = "id, job_id, project_id, task_id, kind, base_rev, patch_json, \
     rationale, jev_json, confidence, state, decided_by, decided_at, applied_event_id";
const CURSOR_COLUMNS: &str = "connector, scope, cursor, last_synced_at, error";
const SLACK_PEER_COLUMNS: &str = "id, user_id, name, is_bot, excluded, created_at";
/// `conversations` is the episode table; the aliases keep the model names.
const EPISODE_COLUMNS: &str = "id, channel_id, peer_id, state, opened_at AS started_at, \
     last_message_at, closed_at, jev_json, summary, opened_at AS created_at";
/// `messages` joined with its conversation, so a row carries its peer and channel.
const MESSAGE_SELECT: &str = "m.id, m.conversation_id, c.peer_id, c.channel_id, m.ts, \
     m.thread_ts, m.direction, m.author_id AS user_id, m.text, m.permalink, m.edited_at, \
     m.deleted, m.historical, m.created_at";
const MESSAGE_FROM: &str = "FROM messages m JOIN conversations c ON c.id = m.conversation_id";
const JOB_COLUMNS: &str = "id, conversation_id, project_id, watermark, kind, state, attempts, \
     route, jev_json, candidates_json, summary, error, superseded_by, agent_session, created_at, \
     updated_at, started_at, finished_at";
const JOB_OP_COLUMNS: &str = "id, job_id, op_index, tool, result_json, created_at";

/// Episode states the store accepts (PLAN 3.1).
const EPISODE_STATES: [&str; 5] = ["open", "waiting", "closed", "analyzed", "needs_review"];
/// Job states the store accepts (PLAN 3.2-3.3).
const JOB_STATES: [&str; 7] = [
    "pending",
    "running",
    "pending_agent",
    "done",
    "failed",
    "superseded",
    "shadow",
];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("task {id} is at rev {actual}, expected {expected}")]
    StaleRevision {
        id: String,
        expected: i64,
        actual: i64,
    },
    /// Boxed so `StoreError` stays small enough for `Result` returns.
    #[error(
        "task {task_id} is claimed by {name} (claim {id}) until {lease}",
        name = .holder.holder_name,
        id = .holder.id,
        lease = .holder.lease_expires_at
    )]
    ClaimConflict { task_id: String, holder: Box<Claim> },
    #[error("claim {0} was released")]
    ClaimReleased(String),
    #[error("claim {0} lease expired")]
    ClaimExpired(String),
    #[error("claim {claim_id} does not belong to task {task_id}")]
    ClaimTaskMismatch { claim_id: String, task_id: String },
    #[error("{entity} already exists: {value}")]
    Duplicate { entity: &'static str, value: String },
    #[error("proposal {0} is already decided")]
    ProposalDecided(String),
}

impl StoreError {
    /// Stable code for IPC/MCP error mapping; the message is for humans.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io { .. } => "io",
            Self::Sql(_) => "sql",
            Self::NotFound(_) => "not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::StaleRevision { .. } => "stale_rev",
            Self::ClaimConflict { .. } => "claim_conflict",
            Self::ClaimReleased(_) => "claim_released",
            Self::ClaimExpired(_) => "claim_expired",
            Self::ClaimTaskMismatch { .. } => "claim_task_mismatch",
            Self::Duplicate { .. } => "duplicate",
            Self::ProposalDecided(_) => "proposal_decided",
        }
    }
}

/// A connection to the local database. One writer, so a mutex is enough for now.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (or creates) a file database and enables WAL.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|source| StoreError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::from_connection(conn)
    }

    /// Opens a private in-memory database. Used by tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("relay store mutex poisoned")
    }

    // -----------------------------------------------------------------------
    // Projects and statuses
    // -----------------------------------------------------------------------

    /// Creates a project and the fixed default workflow in one transaction.
    ///
    /// `NewProject::new(name, slug, repo_path)` is the `(name, slug, repo_path?)` form.
    pub fn create_project_with_default_workflow(
        &self,
        new: NewProject,
    ) -> Result<ProjectWithStatuses, StoreError> {
        let NewProject {
            name,
            slug,
            repo_path,
            color,
            integrations_json,
        } = new;

        let name = name.trim();
        let slug = slug.trim();
        if name.is_empty() {
            return Err(StoreError::InvalidInput("project name is required".into()));
        }
        validate_slug(slug)?;
        let integrations_json = integrations_json.unwrap_or_else(|| "{}".to_string());
        validate_json(&integrations_json, "integrations_json")?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let exists = tx
            .query_row(
                "SELECT 1 FROM projects WHERE slug = ?1",
                params![slug],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Err(StoreError::Duplicate {
                entity: "project",
                value: slug.to_string(),
            });
        }

        let at = now();
        let project = Project {
            id: new_id(),
            name: name.to_string(),
            slug: slug.to_string(),
            repo_path,
            color,
            integrations_json,
            created_at: at.clone(),
            updated_at: at,
        };
        tx.execute(
            "INSERT INTO projects
                 (id, name, slug, repo_path, color, integrations_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project.id,
                project.name,
                project.slug,
                project.repo_path,
                project.color,
                project.integrations_json,
                project.created_at,
                project.updated_at,
            ],
        )?;

        let mut statuses = Vec::with_capacity(DEFAULT_WORKFLOW.len());
        for (position, (status_name, category)) in DEFAULT_WORKFLOW.iter().enumerate() {
            let status = ProjectStatus {
                id: new_id(),
                project_id: project.id.clone(),
                name: (*status_name).to_string(),
                category: *category,
                position: position as i64,
                color: None,
            };
            tx.execute(
                "INSERT INTO project_statuses (id, project_id, name, category, position, color)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    status.id,
                    status.project_id,
                    status.name,
                    status.category.as_str(),
                    status.position,
                    status.color,
                ],
            )?;
            statuses.push(status);
        }

        tx.commit()?;
        Ok(ProjectWithStatuses { project, statuses })
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY name COLLATE NOCASE, id"
        ))?;
        let rows = stmt.query_map([], project_from_row)?;
        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    pub fn get_project(&self, id: &str) -> Result<Project, StoreError> {
        let conn = self.conn();
        project_by_id(&conn, id)
    }

    /// Sets or clears the project's checkout path. `None`/blank clears it.
    pub fn set_project_repo(
        &self,
        id: &str,
        repo_path: Option<&str>,
    ) -> Result<Project, StoreError> {
        let repo_path = none_if_blank(repo_path.map(str::to_string));
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE projects SET repo_path = ?1, updated_at = ?2 WHERE id = ?3",
            params![repo_path, now(), id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("project {id}")));
        }
        project_by_id(&conn, id)
    }

    pub fn list_statuses(&self, project_id: &str) -> Result<Vec<ProjectStatus>, StoreError> {
        let conn = self.conn();
        project_by_id(&conn, project_id)?;
        statuses_by_project(&conn, project_id)
    }

    // -----------------------------------------------------------------------
    // UI snapshot
    // -----------------------------------------------------------------------

    /// One read of everything the main window needs, already in UI vocabulary.
    ///
    /// The mapping lives in [`crate::view`]; this method only selects rows. It reads under
    /// a single connection so a refresh cannot see half of a claim/ack batch. Tasks are
    /// capped at the same 1000 rows as `list_tasks`.
    pub fn ui_snapshot(
        &self,
        version: &str,
        sync: Vec<view::SyncSource>,
    ) -> Result<view::Snapshot, StoreError> {
        let conn = self.conn();

        let projects = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY name COLLATE NOCASE, id"
            ))?;
            let rows = stmt.query_map([], project_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut statuses = Vec::new();
        for project in &projects {
            statuses.extend(statuses_by_project(&conn, &project.id)?);
        }

        let tasks = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE archived = 0
                 ORDER BY position, created_at, rowid LIMIT ?1"
            ))?;
            let rows = stmt.query_map(params![MAX_TASK_LIMIT as i64], task_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // An active claim is unreleased and still inside its lease. Expired claims are
        // closed lazily by the store, so the filter here is what the UI should show.
        let claims = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {CLAIM_COLUMNS} FROM claims
                 WHERE released_at IS NULL AND lease_expires_at > ?1
                 ORDER BY created_at DESC, rowid DESC"
            ))?;
            let rows = stmt.query_map(params![now()], claim_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // The newest `progress` event per task; that is the claim's last progress line.
        let progress = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {EVENT_COLUMNS} FROM task_events e
                 WHERE e.kind = 'progress'
                   AND e.task_id IN (SELECT id FROM tasks WHERE archived = 0)
                   AND e.rowid = (
                       SELECT newest.rowid FROM task_events newest
                       WHERE newest.task_id = e.task_id AND newest.kind = 'progress'
                       ORDER BY newest.created_at DESC, newest.rowid DESC LIMIT 1)
                 ORDER BY e.task_id"
            ))?;
            let rows = stmt.query_map([], event_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let changes = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {CHANGE_COLUMNS} FROM task_changes
                 WHERE state = 'active'
                   AND task_id IN (SELECT id FROM tasks WHERE archived = 0)
                 ORDER BY created_at, rowid"
            ))?;
            let rows = stmt.query_map([], change_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // First ack wins, matching `task_changes.acked_at` (the timeline's "acked at").
        let acks = {
            let mut stmt = conn
                .prepare("SELECT change_id, claim_id FROM change_acks ORDER BY acked_at, rowid")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let links = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {LINK_COLUMNS} FROM task_links
                 WHERE task_id IN (SELECT id FROM tasks WHERE archived = 0)
                 ORDER BY kind, rowid"
            ))?;
            let rows = stmt.query_map([], link_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let pending_proposals: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proposals WHERE state = 'pending'",
            [],
            |row| row.get(0),
        )?;
        let last_proposal_handled_at: Option<String> =
            conn.query_row("SELECT MAX(decided_at) FROM proposals", [], |row| {
                row.get(0)
            })?;

        Ok(view::build_snapshot(
            view::SnapshotData {
                projects,
                statuses,
                tasks,
                claims,
                progress,
                changes,
                acks,
                links,
                pending_proposals: pending_proposals.max(0) as usize,
                last_proposal_handled_at,
            },
            version,
            sync,
        ))
    }

    // -----------------------------------------------------------------------
    // Tasks
    // -----------------------------------------------------------------------

    /// Creates a task at `rev = 1` and writes a `created` event.
    pub fn create_task(&self, new: NewTask) -> Result<Task, StoreError> {
        let NewTask {
            project_id,
            status_id,
            title,
            notes,
            priority,
            due_at,
            position,
            source,
            metadata_json,
        } = new;

        let title = title.trim();
        if title.is_empty() {
            return Err(StoreError::InvalidInput("task title is required".into()));
        }
        let source = source
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "local".to_string());
        let due_at = match due_at {
            Some(value) => Some(normalize_time_required(&value, "due_at")?),
            None => None,
        };
        let metadata_json = metadata_json.unwrap_or_else(|| "{}".to_string());
        validate_json(&metadata_json, "metadata_json")?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project = project_by_id(&tx, &project_id)?;
        let statuses = statuses_by_project(&tx, &project.id)?;

        let status_id = match status_id {
            Some(id) => {
                let status = status_by_id(&tx, &id)?;
                if status.project_id != project.id {
                    return Err(StoreError::InvalidInput(format!(
                        "status {id} does not belong to project {}",
                        project.id
                    )));
                }
                id
            }
            None => statuses
                .iter()
                .find(|status| status.category == StatusCategory::Todo)
                .or_else(|| statuses.first())
                .map(|status| status.id.clone())
                .ok_or_else(|| {
                    StoreError::NotFound(format!("a status for project {}", project.id))
                })?,
        };

        let at = now();
        let position = match position {
            Some(position) => position,
            None => next_task_position(&tx, &project.id)?,
        };
        let task = Task {
            id: new_id(),
            project_id: project.id,
            status_id,
            title: title.to_string(),
            notes: notes.unwrap_or_default(),
            priority,
            due_at,
            position,
            source,
            rev: 1,
            archived: false,
            created_at: at.clone(),
            updated_at: at,
            completed_at: None,
            metadata_json,
        };
        tx.execute(
            "INSERT INTO tasks
                 (id, project_id, status_id, title, notes, priority, due_at, position, source,
                  rev, archived, created_at, updated_at, completed_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                task.id,
                task.project_id,
                task.status_id,
                task.title,
                task.notes,
                task.priority,
                task.due_at,
                task.position,
                task.source,
                task.rev,
                task.archived,
                task.created_at,
                task.updated_at,
                task.completed_at,
                task.metadata_json,
            ],
        )?;
        insert_event(
            &tx,
            &task.id,
            "created",
            None,
            Some("system"),
            None,
            &serde_json::json!({"statusId": task.status_id, "source": task.source}),
        )?;

        tx.commit()?;
        Ok(task)
    }

    pub fn get_task(&self, id: &str) -> Result<Task, StoreError> {
        let conn = self.conn();
        task_by_id(&conn, id)
    }

    /// Filters tasks. `text` is a case-insensitive substring over title and notes;
    /// `search_tasks` is the FTS5 path for candidate shortlists.
    pub fn list_tasks(&self, filter: TaskFilter) -> Result<Vec<Task>, StoreError> {
        let TaskFilter {
            project_id,
            status_id,
            category,
            text,
            updated_since,
            include_archived,
            limit,
        } = filter;

        let project_id = project_id.filter(|id| !id.is_empty());
        let status_id = status_id.filter(|id| !id.is_empty());
        let category = category.map(|category| category.as_str());
        let text = text.and_then(|text| {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(like_pattern(text))
            }
        });
        let updated_since = match updated_since
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(value) => Some(normalize_time_required(value, "updated_since")?),
            None => None,
        };
        let limit = limit.unwrap_or(DEFAULT_TASK_LIMIT).clamp(1, MAX_TASK_LIMIT) as i64;

        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM tasks
             WHERE (?1 = 1 OR archived = 0)
               AND (?2 IS NULL OR project_id = ?2)
               AND (?3 IS NULL OR status_id = ?3)
               AND (?4 IS NULL OR EXISTS (
                     SELECT 1 FROM project_statuses s
                     WHERE s.id = tasks.status_id AND s.category = ?4))
               AND (?5 IS NULL OR title LIKE ?5 ESCAPE '\\' OR notes LIKE ?5 ESCAPE '\\')
               AND (?6 IS NULL OR updated_at >= ?6)
             ORDER BY position, created_at, rowid
             LIMIT ?7"
        );

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                include_archived as i64,
                project_id.as_deref(),
                status_id.as_deref(),
                category,
                text.as_deref(),
                updated_since.as_deref(),
                limit,
            ],
            task_from_row,
        )?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    /// FTS5 search over title and notes, for task search and JEV candidates.
    ///
    /// `text` is literal: every whitespace-separated token is quoted before it
    /// reaches `MATCH`, so FTS operators (`OR`, `NOT`, `NEAR`, column filters)
    /// and wildcards (`*`) in user input cannot change the query. An empty
    /// search has no candidates.
    pub fn search_tasks(&self, text: &str, limit: usize) -> Result<Vec<Task>, StoreError> {
        let text = text.trim();
        if text.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let query = fts_query(text);
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.project_id, t.status_id, t.title, t.notes, t.priority, t.due_at,
                    t.position, t.source, t.rev, t.archived, t.created_at, t.updated_at,
                    t.completed_at, t.metadata_json
             FROM tasks_fts JOIN tasks t ON t.rowid = tasks_fts.rowid
             WHERE tasks_fts MATCH ?1 AND t.archived = 0
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], task_from_row)?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    /// Updates allowed task fields with optimistic concurrency and writes an `updated`
    /// event. `patch` must not be empty; `priority: null` and `due_at: null` clear.
    pub fn update_task(
        &self,
        id: &str,
        patch: TaskPatch,
        expected_rev: i64,
    ) -> Result<Task, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (task, _event) = apply_task_patch(&tx, id, &patch, expected_rev, None, Some("user"))?;
        tx.commit()?;
        Ok(task)
    }

    /// Task plus status, timeline, links, events, active claim and the unacked changes
    /// for `claim_id` (the human queue when `None`).
    pub fn get_task_detail(
        &self,
        id: &str,
        claim_id: Option<&str>,
    ) -> Result<TaskDetail, StoreError> {
        let conn = self.conn();
        let task = task_by_id(&conn, id)?;
        let status = status_by_id(&conn, &task.status_id)?;
        let changes = changes_for_task(&conn, id)?;
        let links = links_for_task(&conn, id)?;
        let events = events_for_task(&conn, id)?;
        let active_claim = active_claim_for_task(&conn, id, &now())?;
        let unacked_changes = match claim_id {
            Some(claim_id) => {
                let claim = claim_by_id(&conn, claim_id)?;
                if claim.task_id != task.id {
                    return Err(StoreError::ClaimTaskMismatch {
                        claim_id: claim_id.to_string(),
                        task_id: task.id.clone(),
                    });
                }
                unacked_for_claim(&conn, &claim)?
            }
            None => unacked_for_human(&conn, id)?,
        };
        Ok(TaskDetail {
            task,
            status,
            changes,
            links,
            events,
            active_claim,
            unacked_changes,
        })
    }

    // -----------------------------------------------------------------------
    // Changes (append-only) and acks
    // -----------------------------------------------------------------------

    /// Appends a change. A change carrying a `claim_id` is a claimant write: the claim
    /// must own the task, be live, and its lease is extended.
    pub fn add_change(&self, new: NewChange) -> Result<TaskChange, StoreError> {
        let NewChange {
            task_id,
            claim_id,
            kind,
            body,
            excerpt,
            permalink,
            source_kind,
            source_id,
            confidence,
        } = new;

        if body.trim().is_empty() {
            return Err(StoreError::InvalidInput("change body is required".into()));
        }
        let source_kind = source_kind.trim();
        if source_kind.is_empty() {
            return Err(StoreError::InvalidInput(
                "change source_kind is required".into(),
            ));
        }
        if let Some(confidence) = confidence
            && !confidence.is_finite()
        {
            return Err(StoreError::InvalidInput(
                "change confidence must be a finite number".into(),
            ));
        }

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = task_by_id(&tx, &task_id)?;

        let claim = match claim_id {
            Some(claim_id) => {
                let claim = claim_by_id(&tx, &claim_id)?;
                if claim.task_id != task.id {
                    return Err(StoreError::ClaimTaskMismatch {
                        claim_id,
                        task_id: task.id.clone(),
                    });
                }
                ensure_claim_active(&claim)?;
                Some(extend_claim(&tx, &claim)?)
            }
            None => None,
        };

        let change = TaskChange {
            id: new_id(),
            task_id: task.id.clone(),
            claim_id: claim.as_ref().map(|claim| claim.id.clone()),
            kind,
            body,
            excerpt: none_if_blank(excerpt),
            permalink: none_if_blank(permalink),
            source_kind: source_kind.to_string(),
            source_id: none_if_blank(source_id),
            confidence,
            state: ChangeState::Active,
            created_at: now(),
            acked_at: None,
            superseded_by: None,
        };
        tx.execute(
            "INSERT INTO task_changes
                 (id, task_id, claim_id, kind, body, excerpt, permalink, source_kind, source_id,
                  confidence, state, created_at, acked_at, superseded_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                change.id,
                change.task_id,
                change.claim_id,
                change.kind.as_str(),
                change.body,
                change.excerpt,
                change.permalink,
                change.source_kind,
                change.source_id,
                change.confidence,
                change.state.as_str(),
                change.created_at,
                change.acked_at,
                change.superseded_by,
            ],
        )?;

        let actor = claim.as_ref().map(|claim| claim.holder_name.as_str());
        let actor_kind = claim
            .as_ref()
            .map(|claim| claim.holder_kind.as_str())
            .or(Some(source_kind));
        insert_event(
            &tx,
            &change.task_id,
            "change_added",
            actor,
            actor_kind,
            Some(&change.body),
            &serde_json::json!({
                "changeId": change.id,
                "kind": change.kind.as_str(),
                "sourceKind": change.source_kind,
            }),
        )?;

        tx.commit()?;
        Ok(change)
    }

    /// The full timeline, oldest first, including retracted and superseded changes.
    pub fn list_changes(&self, task_id: &str) -> Result<Vec<TaskChange>, StoreError> {
        let conn = self.conn();
        changes_for_task(&conn, task_id)
    }

    /// Marks a change `retracted` without deleting history. Idempotent.
    pub fn retract_change(&self, change_id: &str) -> Result<TaskChange, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let change = change_by_id(&tx, change_id)?;

        if change.state == ChangeState::Retracted {
            tx.commit()?;
            return Ok(change);
        }
        if change.state == ChangeState::Superseded {
            return Err(StoreError::InvalidInput(format!(
                "change {change_id} is superseded and cannot be retracted"
            )));
        }

        tx.execute(
            "UPDATE task_changes SET state = 'retracted' WHERE id = ?1",
            params![change.id],
        )?;
        insert_event(
            &tx,
            &change.task_id,
            "change_retracted",
            None,
            Some("system"),
            None,
            &serde_json::json!({"changeId": change.id}),
        )?;
        let updated = change_by_id(&tx, change_id)?;
        tx.commit()?;
        Ok(updated)
    }

    /// Marks `old_id` superseded by `new_id`, keeping both rows. Returns the old change.
    /// Idempotent when the same pair is repeated.
    pub fn supersede_change(&self, old_id: &str, new_id: &str) -> Result<TaskChange, StoreError> {
        if old_id == new_id {
            return Err(StoreError::InvalidInput(
                "a change cannot supersede itself".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = change_by_id(&tx, old_id)?;
        let new = change_by_id(&tx, new_id)?;
        if old.task_id != new.task_id {
            return Err(StoreError::InvalidInput(
                "changes belong to different tasks".into(),
            ));
        }
        if old.state == ChangeState::Superseded {
            if old.superseded_by.as_deref() == Some(new_id) {
                tx.commit()?;
                return Ok(old);
            }
            return Err(StoreError::InvalidInput(format!(
                "change {old_id} is already superseded by another change"
            )));
        }
        if old.state == ChangeState::Retracted {
            return Err(StoreError::InvalidInput(format!(
                "change {old_id} is retracted and cannot be superseded"
            )));
        }

        tx.execute(
            "UPDATE task_changes SET state = 'superseded', superseded_by = ?1 WHERE id = ?2",
            params![new.id, old.id],
        )?;
        insert_event(
            &tx,
            &old.task_id,
            "change_superseded",
            None,
            Some("system"),
            None,
            &serde_json::json!({"changeId": old.id, "supersededBy": new.id}),
        )?;
        let updated = change_by_id(&tx, old_id)?;
        tx.commit()?;
        Ok(updated)
    }

    /// Acknowledges a change for one claim, or for a human when `claim_id` is `None`.
    /// A claimant ack requires a live claim that owns the change's task and extends its
    /// lease. Human acks are deliberately a separate scope: they never clear an agent's
    /// queue, and a new claimant never inherits the previous claimant's acks.
    pub fn ack_change(
        &self,
        claim_id: Option<&str>,
        change_id: &str,
    ) -> Result<AckResult, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let change = change_by_id(&tx, change_id)?;

        let claim = match claim_id {
            Some(claim_id) => {
                let claim = claim_by_id(&tx, claim_id)?;
                if claim.task_id != change.task_id {
                    return Err(StoreError::ClaimTaskMismatch {
                        claim_id: claim_id.to_string(),
                        task_id: change.task_id.clone(),
                    });
                }
                ensure_claim_active(&claim)?;
                Some(extend_claim(&tx, &claim)?)
            }
            None => None,
        };

        let at = now();
        let recorded = tx.execute(
            "INSERT OR IGNORE INTO change_acks (change_id, claim_id, acked_at)
             VALUES (?1, ?2, ?3)",
            params![change.id, claim.as_ref().map(|claim| claim.id.as_str()), at],
        )?;
        // First ack of any kind is the timeline's "acknowledged at".
        tx.execute(
            "UPDATE task_changes SET acked_at = COALESCE(acked_at, ?1) WHERE id = ?2",
            params![at, change.id],
        )?;
        if recorded > 0 {
            insert_event(
                &tx,
                &change.task_id,
                "change_acked",
                claim.as_ref().map(|claim| claim.holder_name.as_str()),
                claim.as_ref().map(|claim| claim.holder_kind.as_str()),
                Some(&format!("acknowledged {}", change.kind)),
                &serde_json::json!({
                    "changeId": change.id,
                    "claimId": claim.as_ref().map(|claim| claim.id.as_str()),
                }),
            )?;
        }

        let change = change_by_id(&tx, change_id)?;
        let unacked = match &claim {
            Some(claim) => unacked_for_claim(&tx, claim)?,
            None => unacked_for_human(&tx, &change.task_id)?,
        };
        let scope_claim_id = claim.map(|claim| claim.id);
        tx.commit()?;

        Ok(AckResult {
            change: ChangeView {
                change,
                claim_id: scope_claim_id,
                acked: true,
                acked_elsewhere: false,
            },
            unacked_changes: unacked,
        })
    }

    /// Active changes this claim has not acked. Reads work for a released claim too.
    pub fn unacked_changes(&self, claim_id: &str) -> Result<Vec<ChangeView>, StoreError> {
        let conn = self.conn();
        let claim = claim_by_id(&conn, claim_id)?;
        unacked_for_claim(&conn, &claim)
    }

    // -----------------------------------------------------------------------
    // Claims
    // -----------------------------------------------------------------------

    /// Compare-and-set claim with `BEGIN IMMEDIATE`. An active, unexpired claim wins and
    /// is returned as the conflict holder. Expired unreleased claims are closed as
    /// `expired` in the same transaction before the new claim is inserted.
    pub fn claim_task(&self, new: NewClaim) -> Result<ClaimView, StoreError> {
        let NewClaim {
            task_id,
            holder_kind,
            holder_name,
            thread_ref,
            session_ref,
        } = new;

        let holder_kind = holder_kind.trim();
        let holder_name = holder_name.trim();
        if holder_kind.is_empty() {
            return Err(StoreError::InvalidInput(
                "claim holder_kind is required".into(),
            ));
        }
        if holder_name.is_empty() {
            return Err(StoreError::InvalidInput(
                "claim holder_name is required".into(),
            ));
        }

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = task_by_id(&tx, &task_id)?;
        if task.archived {
            return Err(StoreError::InvalidInput(format!(
                "task {task_id} is archived"
            )));
        }

        let at = now();
        if let Some(holder) = active_claim_for_task(&tx, &task.id, &at)? {
            return Err(StoreError::ClaimConflict {
                task_id: task.id,
                holder: Box::new(holder),
            });
        }

        for stale in expired_claims_for_task(&tx, &task.id, &at)? {
            tx.execute(
                "UPDATE claims SET released_at = ?1, outcome = 'expired' WHERE id = ?2",
                params![at, stale.id],
            )?;
            insert_event(
                &tx,
                &stale.task_id,
                "claim_expired",
                Some(stale.holder_name.as_str()),
                Some(stale.holder_kind.as_str()),
                None,
                &serde_json::json!({"claimId": stale.id}),
            )?;
        }

        let claim = Claim {
            id: new_id(),
            task_id: task.id.clone(),
            holder_kind: holder_kind.to_string(),
            holder_name: holder_name.to_string(),
            thread_ref: none_if_blank(thread_ref),
            session_ref: none_if_blank(session_ref),
            lease_expires_at: now_plus_minutes(CLAIM_LEASE_MINUTES),
            released_at: None,
            outcome: None,
            created_at: at,
        };
        tx.execute(
            "INSERT INTO claims
                 (id, task_id, holder_kind, holder_name, thread_ref, session_ref,
                  lease_expires_at, released_at, outcome, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                claim.id,
                claim.task_id,
                claim.holder_kind,
                claim.holder_name,
                claim.thread_ref,
                claim.session_ref,
                claim.lease_expires_at,
                claim.released_at,
                claim.outcome,
                claim.created_at,
            ],
        )?;
        insert_event(
            &tx,
            &claim.task_id,
            "claimed",
            Some(claim.holder_name.as_str()),
            Some(claim.holder_kind.as_str()),
            None,
            &serde_json::json!({
                "claimId": claim.id,
                "threadRef": claim.thread_ref,
                "sessionRef": claim.session_ref,
                "leaseExpiresAt": claim.lease_expires_at,
            }),
        )?;

        let unacked_changes = unacked_for_claim(&tx, &claim)?;
        tx.commit()?;
        Ok(ClaimView {
            claim,
            unacked_changes,
        })
    }

    pub fn get_claim(&self, claim_id: &str) -> Result<Claim, StoreError> {
        let conn = self.conn();
        claim_by_id(&conn, claim_id)
    }

    /// Extends a live claim's lease by another 30 minutes. A released or expired claim
    /// cannot heartbeat.
    pub fn heartbeat(&self, claim_id: &str) -> Result<Claim, StoreError> {
        let conn = self.conn();
        let claim = claim_by_id(&conn, claim_id)?;
        ensure_claim_active(&claim)?;
        let lease_expires_at = now_plus_minutes(CLAIM_LEASE_MINUTES);
        conn.execute(
            "UPDATE claims SET lease_expires_at = ?1 WHERE id = ?2",
            params![lease_expires_at, claim.id],
        )?;
        Ok(Claim {
            lease_expires_at,
            ..claim
        })
    }

    /// Releases a claim as `blocked` or `abandoned`. Idempotent once released.
    pub fn release_claim(&self, claim_id: &str, outcome: &str) -> Result<Claim, StoreError> {
        let outcome = outcome.trim();
        if !matches!(outcome, "blocked" | "abandoned") {
            return Err(StoreError::InvalidInput(format!(
                "release outcome must be `blocked` or `abandoned`, got `{outcome}`"
            )));
        }

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claim = claim_by_id(&tx, claim_id)?;
        if claim.released_at.is_some() {
            // Only a replay of the same release is a no-op; a claim released as
            // `completed` must not be silently re-released as `blocked`.
            if claim.outcome.as_deref() == Some(outcome) {
                tx.commit()?;
                return Ok(claim);
            }
            return Err(StoreError::ClaimReleased(claim.id));
        }

        let at = now();
        tx.execute(
            "UPDATE claims SET released_at = ?1, outcome = ?2 WHERE id = ?3",
            params![at, outcome, claim.id],
        )?;
        insert_event(
            &tx,
            &claim.task_id,
            "released",
            Some(claim.holder_name.as_str()),
            Some(claim.holder_kind.as_str()),
            Some(outcome),
            &serde_json::json!({"claimId": claim.id, "outcome": outcome}),
        )?;
        tx.commit()?;
        Ok(Claim {
            released_at: Some(at),
            outcome: Some(outcome.to_string()),
            ..claim
        })
    }

    /// Moves the task to the project's `review` status (never `done`), stamps
    /// `completed_at`, releases the claim as `completed` and writes a `completed` event.
    /// Replaying completion for the same claim is idempotent.
    pub fn complete_task(&self, claim_id: &str, summary: &str) -> Result<Task, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claim = claim_by_id(&tx, claim_id)?;

        if claim.released_at.is_some() {
            if claim.outcome.as_deref() == Some("completed") {
                let task = task_by_id(&tx, &claim.task_id)?;
                tx.commit()?;
                return Ok(task);
            }
            return Err(StoreError::ClaimReleased(claim.id));
        }
        ensure_claim_active(&claim)?;

        let task = task_by_id(&tx, &claim.task_id)?;
        let review = review_status_for_project(&tx, &task.project_id)?.ok_or_else(|| {
            StoreError::InvalidInput(format!("project {} has no review status", task.project_id))
        })?;
        let at = now();
        tx.execute(
            "UPDATE tasks
             SET status_id = ?1, rev = rev + 1, updated_at = ?2, completed_at = ?2
             WHERE id = ?3",
            params![review.id, at, task.id],
        )?;
        let body = summary.trim();
        insert_event(
            &tx,
            &task.id,
            "completed",
            Some(claim.holder_name.as_str()),
            Some(claim.holder_kind.as_str()),
            if body.is_empty() { None } else { Some(body) },
            &serde_json::json!({
                "claimId": claim.id,
                "statusId": review.id,
                "outcome": "completed",
            }),
        )?;
        tx.execute(
            "UPDATE claims SET released_at = ?1, outcome = 'completed' WHERE id = ?2",
            params![at, claim.id],
        )?;
        let completed = task_by_id(&tx, &task.id)?;
        tx.commit()?;
        Ok(completed)
    }

    /// Appends a `progress` event, extends the lease and returns the claim's remaining
    /// unacked changes. This is the reliable notification path for a working agent.
    pub fn add_progress(&self, claim_id: &str, note: &str) -> Result<ProgressUpdate, StoreError> {
        let note = note.trim();
        if note.is_empty() {
            return Err(StoreError::InvalidInput("progress note is required".into()));
        }

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claim = claim_by_id(&tx, claim_id)?;
        ensure_claim_active(&claim)?;

        let lease_expires_at = now_plus_minutes(CLAIM_LEASE_MINUTES);
        tx.execute(
            "UPDATE claims SET lease_expires_at = ?1 WHERE id = ?2",
            params![lease_expires_at, claim.id],
        )?;
        let event = insert_event(
            &tx,
            &claim.task_id,
            "progress",
            Some(claim.holder_name.as_str()),
            Some(claim.holder_kind.as_str()),
            Some(note),
            &serde_json::json!({"claimId": claim.id, "leaseExpiresAt": lease_expires_at}),
        )?;
        let claim = Claim {
            lease_expires_at,
            ..claim
        };
        let unacked_changes = unacked_for_claim(&tx, &claim)?;
        tx.commit()?;
        Ok(ProgressUpdate {
            claim,
            event,
            unacked_changes,
        })
    }

    // -----------------------------------------------------------------------
    // Links and events
    // -----------------------------------------------------------------------

    /// Attaches a link, deduping on `(task_id, kind, external_id)` or, when there is no
    /// external id, on `(task_id, kind, url)`. Re-adding refreshes the existing row.
    pub fn add_link(&self, new: NewLink) -> Result<TaskLink, StoreError> {
        let NewLink {
            task_id,
            claim_id,
            kind,
            external_id,
            url,
            title,
            state,
            metadata_json,
            last_synced_at,
        } = new;

        let kind = kind.trim();
        if kind.is_empty() {
            return Err(StoreError::InvalidInput("link kind is required".into()));
        }
        let url = url.trim();
        if url.is_empty() {
            return Err(StoreError::InvalidInput("link url is required".into()));
        }
        let external_id = none_if_blank(external_id);
        if let Some(metadata_json) = &metadata_json {
            validate_json(metadata_json, "metadata_json")?;
        }
        let last_synced_at = match last_synced_at {
            Some(value) => Some(normalize_time_required(&value, "last_synced_at")?),
            None => None,
        };

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = task_by_id(&tx, &task_id)?;

        let claim = match claim_id {
            Some(claim_id) => {
                let claim = claim_by_id(&tx, &claim_id)?;
                if claim.task_id != task.id {
                    return Err(StoreError::ClaimTaskMismatch {
                        claim_id,
                        task_id: task.id.clone(),
                    });
                }
                ensure_claim_active(&claim)?;
                Some(extend_claim(&tx, &claim)?)
            }
            None => None,
        };

        let existing = if let Some(external_id) = &external_id {
            tx.query_row(
                &format!(
                    "SELECT {LINK_COLUMNS} FROM task_links
                     WHERE task_id = ?1 AND kind = ?2 AND external_id = ?3
                     ORDER BY rowid LIMIT 1"
                ),
                params![task.id, kind, external_id],
                link_from_row,
            )
            .optional()?
        } else {
            tx.query_row(
                &format!(
                    "SELECT {LINK_COLUMNS} FROM task_links
                     WHERE task_id = ?1 AND kind = ?2 AND external_id IS NULL AND url = ?3
                     ORDER BY rowid LIMIT 1"
                ),
                params![task.id, kind, url],
                link_from_row,
            )
            .optional()?
        };

        let created = existing.is_none();
        let link = match existing {
            Some(mut link) => {
                link.title = title.or(link.title);
                link.state = state.or(link.state);
                if let Some(metadata_json) = metadata_json {
                    link.metadata_json = metadata_json;
                }
                link.last_synced_at = last_synced_at.or(link.last_synced_at);
                tx.execute(
                    "UPDATE task_links
                     SET title = ?1, state = ?2, metadata_json = ?3, last_synced_at = ?4
                     WHERE id = ?5",
                    params![
                        link.title,
                        link.state,
                        link.metadata_json,
                        link.last_synced_at,
                        link.id,
                    ],
                )?;
                link
            }
            None => {
                let link = TaskLink {
                    id: new_id(),
                    task_id: task.id.clone(),
                    kind: kind.to_string(),
                    external_id,
                    url: url.to_string(),
                    title,
                    state,
                    metadata_json: metadata_json.unwrap_or_else(|| "{}".to_string()),
                    last_synced_at,
                };
                tx.execute(
                    "INSERT INTO task_links
                         (id, task_id, kind, external_id, url, title, state, metadata_json,
                          last_synced_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        link.id,
                        link.task_id,
                        link.kind,
                        link.external_id,
                        link.url,
                        link.title,
                        link.state,
                        link.metadata_json,
                        link.last_synced_at,
                    ],
                )?;
                link
            }
        };

        if created {
            insert_event(
                &tx,
                &link.task_id,
                "link_added",
                claim.as_ref().map(|claim| claim.holder_name.as_str()),
                claim.as_ref().map(|claim| claim.holder_kind.as_str()),
                None,
                &serde_json::json!({"linkId": link.id, "kind": link.kind, "url": link.url}),
            )?;
        }
        tx.commit()?;
        Ok(link)
    }

    pub fn list_links(&self, task_id: &str) -> Result<Vec<TaskLink>, StoreError> {
        let conn = self.conn();
        links_for_task(&conn, task_id)
    }

    /// Every link of one kind on an active task (the PR poller walks `kind = "pr"`).
    pub fn list_links_by_kind(&self, kind: &str) -> Result<Vec<TaskLink>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {LINK_COLUMNS} FROM task_links
             WHERE kind = ?1
               AND task_id IN (SELECT id FROM tasks WHERE archived = 0)
             ORDER BY COALESCE(last_synced_at, '') LIMIT 500"
        ))?;
        let rows = stmt.query_map(params![kind], link_from_row)?;
        let mut links = Vec::new();
        for row in rows {
            links.push(row?);
        }
        Ok(links)
    }

    /// Links on active tasks whose external id or URL equals one of the given values,
    /// compared case-insensitively and ignoring a trailing slash on the URL. Callers pass
    /// lowercase values. Used to tie a transcript's pasted links back to tasks.
    pub fn find_links_matching(
        &self,
        external_ids: &[String],
        urls: &[String],
    ) -> Result<Vec<TaskLink>, StoreError> {
        if external_ids.is_empty() && urls.is_empty() {
            return Ok(Vec::new());
        }
        let marks = |n: usize| vec!["?"; n].join(", ");
        let mut clauses = Vec::new();
        let mut bind: Vec<&String> = Vec::new();
        if !external_ids.is_empty() {
            clauses.push(format!(
                "lower(external_id) IN ({})",
                marks(external_ids.len())
            ));
            bind.extend(external_ids);
        }
        if !urls.is_empty() {
            clauses.push(format!("lower(rtrim(url, '/')) IN ({})", marks(urls.len())));
            bind.extend(urls);
        }
        let sql = format!(
            "SELECT {LINK_COLUMNS} FROM task_links
             WHERE ({}) AND task_id IN (SELECT id FROM tasks WHERE archived = 0)",
            clauses.join(" OR ")
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bind), link_from_row)?;
        let mut links = Vec::new();
        for row in rows {
            links.push(row?);
        }
        Ok(links)
    }

    /// Every link of one kind whose task lives in `project_id`, archived tasks included, with
    /// the task's archived flag. Reconciling an external source needs the archived ones to
    /// tell "reassigned to me" from "never seen".
    pub fn list_links_in_project(
        &self,
        kind: &str,
        project_id: &str,
    ) -> Result<Vec<(TaskLink, bool)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {LINK_COLUMNS},
                    (SELECT archived FROM tasks WHERE tasks.id = task_links.task_id) AS archived
             FROM task_links
             WHERE kind = ?1
               AND task_id IN (SELECT id FROM tasks WHERE project_id = ?2)"
        ))?;
        let rows = stmt.query_map(params![kind, project_id], |row| {
            Ok((link_from_row(row)?, row.get::<_, bool>("archived")?))
        })?;
        let mut links = Vec::new();
        for row in rows {
            links.push(row?);
        }
        Ok(links)
    }

    /// Updates the polled fields of a link. `state`, `title` and metadata are kept when `None`.
    pub fn set_link_sync(
        &self,
        id: &str,
        state: Option<&str>,
        title: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<TaskLink, StoreError> {
        if let Some(metadata_json) = metadata_json {
            validate_json(metadata_json, "metadata_json")?;
        }
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE task_links SET
                 state = COALESCE(?1, state),
                 title = COALESCE(?2, title),
                 metadata_json = COALESCE(?3, metadata_json),
                 last_synced_at = ?4
             WHERE id = ?5",
            params![state, title, metadata_json, now(), id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("task link {id}")));
        }
        let link = conn.query_row(
            &format!("SELECT {LINK_COLUMNS} FROM task_links WHERE id = ?1"),
            params![id],
            link_from_row,
        )?;
        Ok(link)
    }

    /// Finds a link by its external id or URL, for idempotent imports.
    pub fn find_link(
        &self,
        kind: &str,
        external_id: Option<&str>,
        url: &str,
    ) -> Result<Option<TaskLink>, StoreError> {
        let conn = self.conn();
        let link = conn
            .query_row(
                &format!(
                    "SELECT {LINK_COLUMNS} FROM task_links
                     WHERE kind = ?1 AND (external_id = ?2 OR url = ?3) LIMIT 1"
                ),
                params![kind, external_id, url.trim()],
                link_from_row,
            )
            .optional()?;
        Ok(link)
    }

    /// Appends a free-form event (progress lives in `add_progress`; this is for adapters
    /// like git state or a manual note).
    pub fn add_event(&self, new: NewEvent) -> Result<TaskEvent, StoreError> {
        let NewEvent {
            task_id,
            kind,
            actor,
            actor_kind,
            body,
            data_json,
        } = new;

        let kind = kind.trim();
        if kind.is_empty() {
            return Err(StoreError::InvalidInput("event kind is required".into()));
        }
        let data_json = data_json.unwrap_or_else(|| "{}".to_string());
        validate_json(&data_json, "data_json")?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        task_by_id(&tx, &task_id)?;
        let event = TaskEvent {
            id: new_id(),
            task_id,
            kind: kind.to_string(),
            actor: none_if_blank(actor),
            actor_kind: none_if_blank(actor_kind),
            body,
            data_json,
            created_at: now(),
        };
        tx.execute(
            "INSERT INTO task_events
                 (id, task_id, kind, actor, actor_kind, body, data_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.id,
                event.task_id,
                event.kind,
                event.actor,
                event.actor_kind,
                event.body,
                event.data_json,
                event.created_at,
            ],
        )?;
        tx.commit()?;
        Ok(event)
    }

    pub fn list_events(&self, task_id: &str) -> Result<Vec<TaskEvent>, StoreError> {
        let conn = self.conn();
        events_for_task(&conn, task_id)
    }

    // -----------------------------------------------------------------------
    // Proposals
    // -----------------------------------------------------------------------

    pub fn create_proposal(&self, new: NewProposal) -> Result<ProposalView, StoreError> {
        let NewProposal {
            job_id,
            project_id,
            task_id,
            kind,
            base_rev,
            patch,
            rationale,
            jev_json,
            confidence,
        } = new;

        let kind = kind.trim();
        if kind.is_empty() {
            return Err(StoreError::InvalidInput("proposal kind is required".into()));
        }
        if !patch.is_object() {
            return Err(StoreError::InvalidInput(
                "proposal patch must be a JSON object".into(),
            ));
        }
        // Analyst proposals come from untrusted Slack text. They may only propose the
        // fields PLAN 3.3 allows; title, notes and archived stay human-only.
        if job_id.is_some()
            && let Some(object) = patch.as_object()
        {
            for key in object.keys() {
                if !["statusId", "dueAt", "priority"].contains(&key.as_str()) {
                    return Err(StoreError::InvalidInput(format!(
                        "an analyst proposal cannot change {key}; allowed fields are statusId, dueAt, priority"
                    )));
                }
            }
        }
        if let Some(base_rev) = base_rev
            && base_rev < 1
        {
            return Err(StoreError::InvalidInput(
                "proposal base_rev must be positive".into(),
            ));
        }
        if let Some(confidence) = confidence
            && !confidence.is_finite()
        {
            return Err(StoreError::InvalidInput(
                "proposal confidence must be a finite number".into(),
            ));
        }
        if let Some(jev_json) = &jev_json {
            validate_json(jev_json, "jev_json")?;
        }
        let patch_json = serde_json::to_string(&patch).map_err(|error| {
            StoreError::InvalidInput(format!("invalid proposal patch: {error}"))
        })?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        project_by_id(&tx, &project_id)?;
        if let Some(task_id) = &task_id {
            let task = task_by_id(&tx, task_id)?;
            if task.project_id != project_id {
                return Err(StoreError::InvalidInput(format!(
                    "task {task_id} does not belong to project {project_id}"
                )));
            }
        }

        let proposal = Proposal {
            id: new_id(),
            job_id,
            project_id,
            task_id,
            kind: kind.to_string(),
            base_rev,
            patch_json,
            rationale,
            jev_json,
            confidence,
            state: "pending".to_string(),
            decided_by: None,
            decided_at: None,
            applied_event_id: None,
        };
        tx.execute(
            "INSERT INTO proposals
                 (id, job_id, project_id, task_id, kind, base_rev, patch_json, rationale, jev_json,
                  confidence, state, decided_by, decided_at, applied_event_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                proposal.id,
                proposal.job_id,
                proposal.project_id,
                proposal.task_id,
                proposal.kind,
                proposal.base_rev,
                proposal.patch_json,
                proposal.rationale,
                proposal.jev_json,
                proposal.confidence,
                proposal.state,
                proposal.decided_by,
                proposal.decided_at,
                proposal.applied_event_id,
            ],
        )?;
        if let Some(task_id) = &proposal.task_id {
            insert_event(
                &tx,
                task_id,
                "proposal_created",
                None,
                Some("system"),
                Some(&proposal.kind),
                &serde_json::json!({"proposalId": proposal.id}),
            )?;
        }
        tx.commit()?;
        Ok(proposal_view(proposal))
    }

    pub fn get_proposal(&self, id: &str) -> Result<Proposal, StoreError> {
        let conn = self.conn();
        proposal_by_id(&conn, id)
    }

    /// `state` is one of `pending`, `approved` or `rejected`; `None` lists all.
    pub fn list_proposals(&self, state: Option<&str>) -> Result<Vec<ProposalView>, StoreError> {
        if let Some(state) = state
            && !matches!(state, "pending" | "approved" | "rejected")
        {
            return Err(StoreError::InvalidInput(format!(
                "unknown proposal state `{state}`"
            )));
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM proposals
             WHERE (?1 IS NULL OR state = ?1)
             ORDER BY rowid DESC"
        ))?;
        let rows = stmt.query_map(params![state], proposal_from_row)?;
        let mut proposals = Vec::new();
        for row in rows {
            proposals.push(proposal_view(row?));
        }
        Ok(proposals)
    }

    /// Approving applies only known `TaskPatch` fields, with the proposal's `base_rev` as
    /// the expected revision. Unknown patch fields are ignored; a stale revision leaves
    /// the proposal pending. Rejecting only records the decision.
    pub fn decide_proposal(
        &self,
        id: &str,
        decision: ProposalDecision,
        by: &str,
    ) -> Result<ProposalView, StoreError> {
        let by = by.trim();
        if by.is_empty() {
            return Err(StoreError::InvalidInput("decided_by is required".into()));
        }

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let proposal = proposal_by_id(&tx, id)?;
        if proposal.state != "pending" {
            return Err(StoreError::ProposalDecided(proposal.id));
        }
        let at = now();

        match decision {
            ProposalDecision::Reject => {
                tx.execute(
                    "UPDATE proposals
                     SET state = 'rejected', decided_by = ?1, decided_at = ?2
                     WHERE id = ?3",
                    params![by, at, proposal.id],
                )?;
                if let Some(task_id) = &proposal.task_id {
                    insert_event(
                        &tx,
                        task_id,
                        "proposal_rejected",
                        Some(by),
                        Some("user"),
                        Some(&proposal.kind),
                        &serde_json::json!({"proposalId": proposal.id}),
                    )?;
                }
                let proposal = Proposal {
                    state: "rejected".to_string(),
                    decided_by: Some(by.to_string()),
                    decided_at: Some(at),
                    ..proposal
                };
                tx.commit()?;
                Ok(proposal_view(proposal))
            }
            ProposalDecision::Approve => {
                let task_id = proposal.task_id.as_deref().ok_or_else(|| {
                    StoreError::InvalidInput(format!(
                        "proposal {id} has no task to apply a patch to"
                    ))
                })?;
                let base_rev = proposal.base_rev.ok_or_else(|| {
                    StoreError::InvalidInput(format!(
                        "proposal {id} has no base_rev; it cannot be applied safely"
                    ))
                })?;
                let patch: TaskPatch =
                    serde_json::from_str(&proposal.patch_json).map_err(|error| {
                        StoreError::InvalidInput(format!(
                            "proposal {id} patch is not a TaskPatch: {error}"
                        ))
                    })?;
                if patch.is_empty() {
                    return Err(StoreError::InvalidInput(format!(
                        "proposal {id} patch has no applicable fields"
                    )));
                }

                let (_task, event) =
                    apply_task_patch(&tx, task_id, &patch, base_rev, Some(by), Some("user"))?;
                tx.execute(
                    "UPDATE proposals
                     SET state = 'approved', decided_by = ?1, decided_at = ?2, applied_event_id = ?3
                     WHERE id = ?4",
                    params![by, at, event.id, proposal.id],
                )?;
                let proposal = Proposal {
                    state: "approved".to_string(),
                    decided_by: Some(by.to_string()),
                    decided_at: Some(at),
                    applied_event_id: Some(event.id),
                    ..proposal
                };
                tx.commit()?;
                Ok(proposal_view(proposal))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Settings and sync cursors
    // -----------------------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        if key.trim().is_empty() {
            return Err(StoreError::InvalidInput("setting key is required".into()));
        }
        let conn = self.conn();
        let value = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        if key.trim().is_empty() {
            return Err(StoreError::InvalidInput("setting key is required".into()));
        }
        let conn = self.conn();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn list_settings(&self) -> Result<Vec<Setting>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = stmt.query_map([], |row| {
            Ok(Setting {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })?;
        let mut settings = Vec::new();
        for row in rows {
            settings.push(row?);
        }
        Ok(settings)
    }

    pub fn get_sync_cursor(
        &self,
        connector: &str,
        scope: &str,
    ) -> Result<Option<SyncCursor>, StoreError> {
        let conn = self.conn();
        let cursor = conn
            .query_row(
                &format!(
                    "SELECT {CURSOR_COLUMNS} FROM sync_cursors
                     WHERE connector = ?1 AND scope = ?2"
                ),
                params![connector, scope],
                cursor_from_row,
            )
            .optional()?;
        Ok(cursor)
    }

    /// Full-row upsert; the caller owns every field.
    pub fn set_sync_cursor(&self, cursor: &SyncCursor) -> Result<(), StoreError> {
        if cursor.connector.trim().is_empty() || cursor.scope.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "sync cursor connector and scope are required".into(),
            ));
        }
        let last_synced_at = match cursor.last_synced_at.as_deref() {
            Some(value) => Some(normalize_time_required(value, "last_synced_at")?),
            None => None,
        };
        let conn = self.conn();
        conn.execute(
            "INSERT INTO sync_cursors (connector, scope, cursor, last_synced_at, error)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(connector, scope) DO UPDATE SET
                 cursor = excluded.cursor,
                 last_synced_at = excluded.last_synced_at,
                 error = excluded.error",
            params![
                cursor.connector,
                cursor.scope,
                cursor.cursor,
                last_synced_at,
                cursor.error,
            ],
        )?;
        Ok(())
    }

    /// Connector progress: `cursor` `None` keeps the current cursor, `error` `None`
    /// clears it, and `last_synced_at` becomes now. Returns the stored row.
    pub fn upsert_sync_cursor(
        &self,
        connector: &str,
        scope: &str,
        cursor: Option<&str>,
        error: Option<&str>,
    ) -> Result<SyncCursor, StoreError> {
        if connector.trim().is_empty() || scope.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "sync cursor connector and scope are required".into(),
            ));
        }
        let last_synced_at = now();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {CURSOR_COLUMNS} FROM sync_cursors
                     WHERE connector = ?1 AND scope = ?2"
                ),
                params![connector, scope],
                cursor_from_row,
            )
            .optional()?;
        let stored_cursor = cursor
            .map(str::to_string)
            .or_else(|| existing.and_then(|existing| existing.cursor));
        tx.execute(
            "INSERT INTO sync_cursors (connector, scope, cursor, last_synced_at, error)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(connector, scope) DO UPDATE SET
                 cursor = excluded.cursor,
                 last_synced_at = excluded.last_synced_at,
                 error = excluded.error",
            params![connector, scope, stored_cursor, last_synced_at, error],
        )?;
        let stored = tx
            .query_row(
                &format!(
                    "SELECT {CURSOR_COLUMNS} FROM sync_cursors
                     WHERE connector = ?1 AND scope = ?2"
                ),
                params![connector, scope],
                cursor_from_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("sync cursor {connector}/{scope}")))?;
        tx.commit()?;
        Ok(stored)
    }

    // -----------------------------------------------------------------------
    // Slack capture (PLAN 3.1)
    // -----------------------------------------------------------------------

    /// Creates a peer, or updates its name. `user_id` is the Slack `U…` id.
    pub fn upsert_slack_peer(
        &self,
        user_id: &str,
        name: Option<&str>,
    ) -> Result<SlackPeer, StoreError> {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Err(StoreError::InvalidInput("slack user id is required".into()));
        }
        let conn = self.conn();
        conn.execute(
            "INSERT INTO slack_peers (id, user_id, name, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET name = COALESCE(excluded.name, slack_peers.name)",
            params![new_id(), user_id, name, now()],
        )?;
        let peer = conn.query_row(
            &format!("SELECT {SLACK_PEER_COLUMNS} FROM slack_peers WHERE user_id = ?1"),
            params![user_id],
            slack_peer_from_row,
        )?;
        Ok(peer)
    }

    pub fn list_slack_peers(&self) -> Result<Vec<SlackPeer>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SLACK_PEER_COLUMNS} FROM slack_peers ORDER BY name COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map([], slack_peer_from_row)?;
        let mut peers = Vec::new();
        for row in rows {
            peers.push(row?);
        }
        Ok(peers)
    }

    pub fn set_slack_peer_excluded(
        &self,
        peer_id: &str,
        excluded: bool,
    ) -> Result<SlackPeer, StoreError> {
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE slack_peers SET excluded = ?1 WHERE id = ?2",
            params![excluded, peer_id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("slack peer {peer_id}")));
        }
        let peer = conn.query_row(
            &format!("SELECT {SLACK_PEER_COLUMNS} FROM slack_peers WHERE id = ?1"),
            params![peer_id],
            slack_peer_from_row,
        )?;
        Ok(peer)
    }

    /// Stores one DM message and assigns its episode (gap rule and thread parent).
    ///
    /// Returns `None` when the peer is excluded: capture is off and nothing is written.
    /// Replaying the same `(channel_id, ts)` updates the stored text instead of
    /// duplicating the row.
    pub fn capture_slack_message(
        &self,
        peer_user_id: &str,
        peer_name: Option<&str>,
        message: &RawSlackMessage,
        gap_minutes: i64,
    ) -> Result<Option<SlackMessage>, StoreError> {
        let at = model::slack_ts_to_rfc3339(&message.ts).ok_or_else(|| {
            StoreError::InvalidInput(format!(
                "slack ts is not an epoch timestamp: {}",
                message.ts
            ))
        })?;
        let channel_id = message.channel_id.trim();
        if channel_id.is_empty() {
            return Err(StoreError::InvalidInput(
                "slack channel id is required".into(),
            ));
        }
        if peer_user_id.trim().is_empty() {
            return Err(StoreError::InvalidInput("slack user id is required".into()));
        }

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO slack_peers (id, user_id, name, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET name = COALESCE(excluded.name, slack_peers.name)",
            params![new_id(), peer_user_id.trim(), peer_name, now()],
        )?;
        let peer = tx.query_row(
            &format!("SELECT {SLACK_PEER_COLUMNS} FROM slack_peers WHERE user_id = ?1"),
            params![peer_user_id.trim()],
            slack_peer_from_row,
        )?;
        if peer.excluded {
            tx.commit()?;
            return Ok(None);
        }

        let created = now();
        // A Slack ts is unique within a channel: a replay updates the row in place.
        let existing = tx
            .query_row(
                "SELECT m.id, m.conversation_id FROM messages m
                 JOIN conversations c ON c.id = m.conversation_id
                 WHERE c.channel_id = ?1 AND m.ts = ?2",
                params![channel_id, message.ts],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((id, conversation_id)) = existing {
            tx.execute(
                "UPDATE messages SET text = ?1, edited_at = ?2, deleted = ?3,
                     permalink = COALESCE(?4, permalink)
                 WHERE id = ?5",
                params![
                    message.text,
                    message.edited_at,
                    message.deleted,
                    message.permalink,
                    id
                ],
            )?;
            tx.execute(
                "UPDATE conversations SET last_message_at = MAX(last_message_at, ?1) WHERE id = ?2",
                params![at, conversation_id],
            )?;
            let stored = tx.query_row(
                &format!("SELECT {MESSAGE_SELECT} {MESSAGE_FROM} WHERE m.id = ?1"),
                params![id],
                slack_message_from_row,
            )?;
            tx.commit()?;
            return Ok(Some(stored));
        }

        // A thread reply belongs to the episode its parent is in, unless that episode is
        // older than 24h; then it starts a new one.
        let parent = match message.thread_ts.as_deref() {
            Some(thread_ts) if thread_ts != message.ts => tx
                .query_row(
                    &format!(
                        "SELECT {EPISODE_COLUMNS} FROM conversations
                         WHERE id = (SELECT m.conversation_id FROM messages m
                                     JOIN conversations c ON c.id = m.conversation_id
                                     WHERE m.ts = ?1 AND c.channel_id = ?2)"
                    ),
                    params![thread_ts, channel_id],
                    slack_episode_from_row,
                )
                .optional()?,
            _ => None,
        };
        let latest = tx
            .query_row(
                &format!(
                    "SELECT {EPISODE_COLUMNS} FROM conversations
                     WHERE channel_id = ?1 AND state IN ('open', 'waiting')
                     ORDER BY last_message_at DESC LIMIT 1"
                ),
                params![channel_id],
                slack_episode_from_row,
            )
            .optional()?;

        let reuse = match &parent {
            Some(episode) => within_hours(&episode.last_message_at, &at, 24),
            None => latest
                .as_ref()
                .is_some_and(|episode| within_minutes(&episode.last_message_at, &at, gap_minutes)),
        };

        let conversation_id = if reuse {
            let episode = parent.or(latest).expect("reuse implies an episode");
            tx.execute(
                "UPDATE conversations
                 SET last_message_at = MAX(last_message_at, ?1), state = 'open', closed_at = NULL
                 WHERE id = ?2",
                params![at, episode.id],
            )?;
            episode.id
        } else {
            let id = new_id();
            tx.execute(
                "INSERT INTO conversations
                     (id, channel_id, peer_id, peer_name, episode_no, state, opened_at,
                      last_message_at)
                 VALUES (?1, ?2, ?3, ?4,
                         (SELECT COALESCE(MAX(episode_no), 0) + 1 FROM conversations WHERE peer_id = ?3),
                         'open', ?5, ?5)",
                params![id, channel_id, peer.id, peer.name, at],
            )?;
            id
        };

        let direction = if message.user_id.as_deref() == Some(peer_user_id.trim()) {
            "in"
        } else {
            "out"
        };
        let author_name = if direction == "in" {
            peer.name.clone()
        } else {
            None
        };
        tx.execute(
            "INSERT INTO messages
                 (id, conversation_id, ts, author_id, author_name, direction, text, thread_ts,
                  permalink, edited_at, deleted, historical, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                new_id(),
                conversation_id,
                message.ts,
                message.user_id.clone().unwrap_or_default(),
                author_name,
                direction,
                message.text,
                message.thread_ts,
                message.permalink,
                message.edited_at,
                message.deleted,
                message.historical,
                created,
            ],
        )?;
        tx.execute(
            "INSERT INTO sync_cursors (connector, scope, cursor, last_synced_at)
             VALUES ('slack', ?1, ?2, ?3)
             ON CONFLICT(connector, scope) DO UPDATE SET
                 cursor = CASE WHEN sync_cursors.cursor IS NULL
                                 OR sync_cursors.cursor < excluded.cursor
                               THEN excluded.cursor ELSE sync_cursors.cursor END,
                 last_synced_at = excluded.last_synced_at",
            params![channel_id, message.ts, created],
        )?;

        let stored = tx.query_row(
            &format!(
                "SELECT {MESSAGE_SELECT} {MESSAGE_FROM} WHERE m.conversation_id = ?1 AND m.ts = ?2"
            ),
            params![conversation_id, message.ts],
            slack_message_from_row,
        )?;
        tx.commit()?;
        Ok(Some(stored))
    }

    pub fn list_slack_episodes(
        &self,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EpisodeSummary>, StoreError> {
        if let Some(state) = state {
            validate_state(state, &EPISODE_STATES, "episode")?;
        }
        let limit = limit.clamp(1, 200) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.channel_id, c.peer_id, c.state, c.opened_at AS started_at,
                    c.last_message_at, c.closed_at, c.jev_json, c.summary,
                    c.opened_at AS created_at,
                    COALESCE(p.name, c.peer_name) AS peer_name,
                    COALESCE(p.user_id, c.peer_id) AS peer_user_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS message_count,
                    (SELECT m.text FROM messages m
                     WHERE m.conversation_id = c.id AND m.deleted = 0
                     ORDER BY m.ts DESC LIMIT 1) AS preview
             FROM conversations c LEFT JOIN slack_peers p ON p.id = c.peer_id
             WHERE (?1 IS NULL OR c.state = ?1)
             ORDER BY c.last_message_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![state, limit], |row| {
            Ok(EpisodeSummary {
                episode: slack_episode_from_row(row)?,
                peer_name: row.get("peer_name")?,
                peer_user_id: row.get("peer_user_id")?,
                message_count: row.get::<_, i64>("message_count")? as usize,
                preview: row.get("preview")?,
            })
        })?;
        let mut episodes = Vec::new();
        for row in rows {
            episodes.push(row?);
        }
        Ok(episodes)
    }

    pub fn get_slack_episode(&self, id: &str) -> Result<EpisodeDetail, StoreError> {
        let conn = self.conn();
        let episode = conn
            .query_row(
                &format!("SELECT {EPISODE_COLUMNS} FROM conversations WHERE id = ?1"),
                params![id],
                slack_episode_from_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("slack episode {id}")))?;
        let peer = conn
            .query_row(
                &format!("SELECT {SLACK_PEER_COLUMNS} FROM slack_peers WHERE id = ?1"),
                params![episode.peer_id],
                slack_peer_from_row,
            )
            .optional()?
            .unwrap_or_else(|| SlackPeer {
                id: episode.peer_id.clone(),
                user_id: episode.peer_id.clone(),
                name: None,
                is_bot: false,
                excluded: false,
                created_at: episode.created_at.clone(),
            });
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_SELECT} {MESSAGE_FROM}
             WHERE m.conversation_id = ?1 ORDER BY m.ts"
        ))?;
        let rows = stmt.query_map(params![id], slack_message_from_row)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(EpisodeDetail {
            episode,
            peer,
            messages,
        })
    }

    /// Episodes with text whose summary is missing, or stale and quiet since `settled_before`
    /// (so a live conversation is not re-summarized on every message). Newest first.
    pub fn episodes_needing_summary(
        &self,
        settled_before: &str,
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id FROM conversations c
             WHERE EXISTS (SELECT 1 FROM messages m
                           WHERE m.conversation_id = c.id AND m.deleted = 0 AND m.text <> '')
               AND (c.summary IS NULL
                    OR (COALESCE(c.summary_through, '') < c.last_message_at
                        AND c.last_message_at <= ?1))
             ORDER BY c.last_message_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![settled_before, limit as i64], |row| row.get(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Stores an episode summary covering messages up to `through` (its `last_message_at`).
    pub fn set_episode_summary(
        &self,
        id: &str,
        summary: &str,
        through: &str,
    ) -> Result<(), StoreError> {
        let changed = self.conn().execute(
            "UPDATE conversations SET summary = ?1, summary_through = ?2 WHERE id = ?3",
            params![summary, through, id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("slack episode {id}")));
        }
        Ok(())
    }

    pub fn set_slack_episode_state(
        &self,
        id: &str,
        state: &str,
    ) -> Result<SlackEpisode, StoreError> {
        validate_state(state, &EPISODE_STATES, "episode")?;
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE conversations
             SET state = ?1,
                 closed_at = CASE
                     WHEN ?1 = 'closed' THEN COALESCE(closed_at, ?2)
                     WHEN ?1 IN ('open', 'waiting') THEN NULL
                     ELSE closed_at END
             WHERE id = ?3",
            params![state, now(), id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("slack episode {id}")));
        }
        let episode = conn.query_row(
            &format!("SELECT {EPISODE_COLUMNS} FROM conversations WHERE id = ?1"),
            params![id],
            slack_episode_from_row,
        )?;
        Ok(episode)
    }

    /// Closes every open episode whose last message is older than `gap_minutes` and returns
    /// their ids, so the caller can rescreen them at close.
    pub fn close_stale_slack_episodes(&self, gap_minutes: i64) -> Result<Vec<String>, StoreError> {
        let cutoff = model::format_time(
            time::OffsetDateTime::now_utc() - time::Duration::minutes(gap_minutes),
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM conversations
             WHERE state IN ('open', 'waiting') AND last_message_at < ?1",
        )?;
        let rows = stmt.query_map(params![cutoff], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        if ids.is_empty() {
            return Ok(ids);
        }
        conn.execute(
            "UPDATE conversations SET state = 'closed', closed_at = ?1
             WHERE state IN ('open', 'waiting') AND last_message_at < ?2",
            params![now(), cutoff],
        )?;
        Ok(ids)
    }

    /// Retention: deletes captured messages whose Slack ts is before `before_seconds`
    /// (Unix seconds). Using `ts` rather than capture time means a 14-day backfill does
    /// not reset the clock. Episodes stay, and task changes keep their excerpt and
    /// permalink.
    pub fn prune_slack_messages(&self, before_seconds: i64) -> Result<usize, StoreError> {
        let conn = self.conn();
        let deleted = conn.execute(
            "DELETE FROM messages WHERE CAST(ts AS REAL) < ?1",
            params![before_seconds as f64],
        )?;
        Ok(deleted)
    }

    pub fn slack_channel_cursor(&self, channel_id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn();
        let cursor = conn
            .query_row(
                "SELECT cursor FROM sync_cursors WHERE connector = 'slack' AND scope = ?1",
                params![channel_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(cursor.flatten())
    }

    // -----------------------------------------------------------------------
    // Analysis jobs (PLAN 3.2-3.3)
    // -----------------------------------------------------------------------

    /// Creates the job for a conversation watermark and returns it with `true`, or returns
    /// the existing job with `false` (a replay must not restart the reviewer). Any other
    /// live job for the conversation is marked `superseded`.
    pub fn create_job(&self, new: NewJob) -> Result<(AnalysisJob, bool), StoreError> {
        self.insert_job(new, false)
    }

    /// Like `create_job`, but a finished job for the same watermark is rewritten in place
    /// and returned with `true`, so a re-analysis can replace a shadow or ignored result.
    /// A job in flight (`running`, `pending_agent`) or one that routed `change` or
    /// `new_task` is returned untouched with `false`: its action was taken or is under way.
    pub fn replace_job(&self, new: NewJob) -> Result<(AnalysisJob, bool), StoreError> {
        self.insert_job(new, true)
    }

    fn insert_job(&self, new: NewJob, replace: bool) -> Result<(AnalysisJob, bool), StoreError> {
        if new.conversation_id.trim().is_empty() || new.watermark.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "job conversation id and watermark are required".into(),
            ));
        }
        if new.kind != "screen" && new.kind != "classify" {
            return Err(StoreError::InvalidInput(format!(
                "job kind must be screen or classify, got {}",
                new.kind
            )));
        }
        if let Some(route) = new.route.as_deref() {
            validate_state(
                route,
                &["ignore", "review", "rescreen", "change", "new_task"],
                "job route",
            )?;
        }
        let candidates_json = serde_json::to_string(&new.candidates)
            .map_err(|error| StoreError::InvalidInput(format!("job candidates: {error}")))?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {JOB_COLUMNS} FROM analysis_jobs
                     WHERE conversation_id = ?1 AND watermark = ?2"
                ),
                params![new.conversation_id, new.watermark],
                job_from_row,
            )
            .optional()?;
        if let Some(job) = existing {
            let acted = matches!(job.state.as_str(), "running" | "pending_agent")
                || matches!(job.route.as_deref(), Some("change" | "new_task"));
            if !replace || acted {
                tx.commit()?;
                return Ok((job, false));
            }
            let at = now();
            tx.execute(
                "UPDATE analysis_jobs
                 SET state = 'superseded', superseded_by = ?1, updated_at = ?2
                 WHERE conversation_id = ?3 AND id <> ?1
                   AND state IN ('pending', 'running', 'pending_agent', 'shadow')",
                params![job.id, at, new.conversation_id],
            )?;
            tx.execute(
                "UPDATE analysis_jobs
                 SET project_id = ?1, kind = ?2, state = 'pending', attempts = 0, route = ?3,
                     jev_json = ?4, candidates_json = ?5, agent_session = NULL, error = NULL,
                     summary = NULL, superseded_by = NULL, created_at = ?6, updated_at = ?6,
                     started_at = NULL, finished_at = NULL
                 WHERE id = ?7",
                params![
                    new.project_id,
                    new.kind,
                    new.route,
                    new.jev_json,
                    candidates_json,
                    at,
                    job.id,
                ],
            )?;
            tx.execute("DELETE FROM job_ops WHERE job_id = ?1", params![job.id])?;
            let job = tx.query_row(
                &format!("SELECT {JOB_COLUMNS} FROM analysis_jobs WHERE id = ?1"),
                params![job.id],
                job_from_row,
            )?;
            tx.commit()?;
            return Ok((job, true));
        }

        let id = new_id();
        let at = now();
        tx.execute(
            "UPDATE analysis_jobs
             SET state = 'superseded', superseded_by = ?1, updated_at = ?2
             WHERE conversation_id = ?3
               AND state IN ('pending', 'running', 'pending_agent', 'shadow')",
            params![id, at, new.conversation_id],
        )?;
        tx.execute(
            "INSERT INTO analysis_jobs
                 (id, conversation_id, project_id, watermark, kind, state, route, jev_json,
                  candidates_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9, ?9)",
            params![
                id,
                new.conversation_id,
                new.project_id,
                new.watermark,
                new.kind,
                new.route,
                new.jev_json,
                candidates_json,
                at,
            ],
        )?;
        let job = tx.query_row(
            &format!("SELECT {JOB_COLUMNS} FROM analysis_jobs WHERE id = ?1"),
            params![id],
            job_from_row,
        )?;
        tx.commit()?;
        Ok((job, true))
    }

    /// Conversations worth analysing again, newest activity first: those with a message in
    /// the last `days` days whose latest job is a `shadow` backfill, or whose latest job
    /// routed `ignore` or `review` (and drew no proposal) before the newest active task
    /// existed, so a task created since could now be a candidate. A conversation with a
    /// job in flight, or none at all, is left out. Re-analysis rewrites the latest job
    /// with the current time, so a second call does not select the same conversation.
    pub fn requeue_recent(&self, days: i64, limit: usize) -> Result<Vec<String>, StoreError> {
        let cutoff =
            model::format_time(time::OffsetDateTime::now_utc() - time::Duration::days(days));
        let limit = limit.clamp(1, 50) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "WITH latest AS (
                 SELECT j.conversation_id, j.id, j.state, j.route, j.created_at
                 FROM analysis_jobs j
                 WHERE j.state <> 'superseded'
                   AND j.id = (SELECT j2.id FROM analysis_jobs j2
                               WHERE j2.conversation_id = j.conversation_id
                                 AND j2.state <> 'superseded'
                               ORDER BY j2.created_at DESC, j2.rowid DESC LIMIT 1)
             )
             SELECT c.id FROM conversations c JOIN latest l ON l.conversation_id = c.id
             WHERE c.last_message_at >= ?1
               AND NOT EXISTS (
                   SELECT 1 FROM analysis_jobs f
                   WHERE f.conversation_id = c.id
                     AND (f.state IN ('running', 'pending_agent')
                          OR (f.state = 'pending' AND f.route IS NULL)))
               AND (l.state = 'shadow'
                    OR (l.route IN ('ignore', 'review')
                        AND l.state IN ('done', 'pending')
                        AND NOT EXISTS (SELECT 1 FROM proposals p WHERE p.job_id = l.id)
                        AND l.created_at < COALESCE(
                            (SELECT MAX(t.created_at) FROM tasks t WHERE t.archived = 0), '')))
             ORDER BY c.last_message_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![cutoff, limit], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub fn get_job(&self, id: &str) -> Result<AnalysisJob, StoreError> {
        let conn = self.conn();
        let job = conn
            .query_row(
                &format!("SELECT {JOB_COLUMNS} FROM analysis_jobs WHERE id = ?1"),
                params![id],
                job_from_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("analysis job {id}")))?;
        Ok(job)
    }

    pub fn list_jobs(
        &self,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AnalysisJob>, StoreError> {
        if let Some(state) = state {
            validate_state(state, &JOB_STATES, "job")?;
        }
        let limit = limit.clamp(1, 200) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM analysis_jobs
             WHERE (?1 IS NULL OR state = ?1)
             ORDER BY created_at DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![state, limit], job_from_row)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row?);
        }
        Ok(jobs)
    }

    /// Moves a job to `state`; `summary` and `error` replace the stored value when given.
    pub fn set_job_status(
        &self,
        id: &str,
        state: &str,
        summary: Option<&str>,
        error: Option<&str>,
    ) -> Result<AnalysisJob, StoreError> {
        validate_state(state, &JOB_STATES, "job")?;
        let at = now();
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE analysis_jobs SET
                 state = ?1,
                 summary = COALESCE(?2, summary),
                 error = ?3,
                 updated_at = ?4,
                 attempts = CASE WHEN ?1 = 'running' THEN attempts + 1 ELSE attempts END,
                 started_at = CASE WHEN ?1 = 'running' THEN COALESCE(started_at, ?4)
                                   ELSE started_at END,
                 finished_at = CASE WHEN ?1 IN ('done', 'failed', 'superseded')
                                     THEN COALESCE(finished_at, ?4) ELSE finished_at END
             WHERE id = ?5",
            params![state, summary, error, at, id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("analysis job {id}")));
        }
        drop(conn);
        self.get_job(id)
    }

    /// Settles a job waiting in Review. `send` hands it to the reviewer as a `change`;
    /// otherwise it is `dismissed`. Either route takes it out of the Review queue and out of
    /// `requeue_recent`.
    pub fn resolve_review_job(&self, id: &str, send: bool) -> Result<AnalysisJob, StoreError> {
        let (route, state, summary) = if send {
            (
                "change",
                "pending_agent",
                "sent to the reviewer from Review",
            )
        } else {
            ("dismissed", "done", "dismissed in Review")
        };
        let at = now();
        let changed = self.conn().execute(
            "UPDATE analysis_jobs SET
                 route = ?1, state = ?2, summary = ?3, error = NULL, updated_at = ?4,
                 finished_at = CASE WHEN ?2 = 'done' THEN ?4 ELSE finished_at END
             WHERE id = ?5 AND state = 'pending' AND route = 'review'",
            params![route, state, summary, at, id],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidInput(format!(
                "analysis job {id} is not waiting in Review"
            )));
        }
        self.get_job(id)
    }

    /// Idempotent analyst write: the first result for `(job_id, op_index)` wins, and a
    /// replay returns it with `false` as the second tuple element.
    pub fn record_job_op(
        &self,
        job_id: &str,
        op_index: i64,
        tool: &str,
        result_json: &str,
    ) -> Result<(JobOp, bool), StoreError> {
        if op_index < 0 {
            return Err(StoreError::InvalidInput(
                "job op index must be zero or positive".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {JOB_OP_COLUMNS} FROM job_ops WHERE job_id = ?1 AND op_index = ?2"
                ),
                params![job_id, op_index],
                job_op_from_row,
            )
            .optional()?;
        if let Some(op) = existing {
            tx.commit()?;
            return Ok((op, false));
        }
        tx.execute(
            "INSERT INTO job_ops (id, job_id, op_index, tool, result_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![new_id(), job_id, op_index, tool, result_json, now()],
        )?;
        let op = tx.query_row(
            &format!("SELECT {JOB_OP_COLUMNS} FROM job_ops WHERE job_id = ?1 AND op_index = ?2"),
            params![job_id, op_index],
            job_op_from_row,
        )?;
        tx.commit()?;
        Ok((op, true))
    }

    pub fn job_op(&self, job_id: &str, op_index: i64) -> Result<Option<JobOp>, StoreError> {
        let conn = self.conn();
        let op = conn
            .query_row(
                &format!(
                    "SELECT {JOB_OP_COLUMNS} FROM job_ops WHERE job_id = ?1 AND op_index = ?2"
                ),
                params![job_id, op_index],
                job_op_from_row,
            )
            .optional()?;
        Ok(op)
    }

    /// True when the job already recorded a write; a retry must resume it, not repeat it.
    pub fn job_has_ops(&self, job_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM job_ops WHERE job_id = ?1",
            params![job_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Adds a task the reviewer created for this job to its candidate list, so a later
    /// `proposals_submit` for that task is in scope.
    pub fn add_job_candidate(
        &self,
        job_id: &str,
        task_id: &str,
    ) -> Result<AnalysisJob, StoreError> {
        let job = self.get_job(job_id)?;
        if job.candidates.iter().any(|id| id == task_id) {
            return Ok(job);
        }
        let mut candidates = job.candidates;
        candidates.push(task_id.to_string());
        let json = serde_json::to_string(&candidates)
            .map_err(|error| StoreError::InvalidInput(format!("job candidates: {error}")))?;
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE analysis_jobs SET candidates_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![json, now(), job_id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("analysis job {job_id}")));
        }
        drop(conn);
        self.get_job(job_id)
    }

    /// Deletes every captured DM, episode and analysis job. Tasks are untouched: a task
    /// created from a conversation is the user's work, not capture history.
    pub fn clear_slack_capture(&self) -> Result<usize, StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let messages = tx.execute("DELETE FROM messages", [])?;
        // analysis_jobs and messages cascade from conversations.
        let conversations = tx.execute("DELETE FROM conversations", [])?;
        let peers = tx.execute("DELETE FROM slack_peers", [])?;
        tx.execute("DELETE FROM sync_cursors WHERE connector = 'slack'", [])?;
        tx.commit()?;
        Ok(messages + conversations + peers)
    }
}

// ---------------------------------------------------------------------------
// Migrations
// ---------------------------------------------------------------------------

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let next = index as i64 + 1;
        if next <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", next)?;
        tx.commit()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        slug: row.get("slug")?,
        repo_path: row.get("repo_path")?,
        color: row.get("color")?,
        integrations_json: row.get("integrations_json")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn status_from_row(row: &Row<'_>) -> rusqlite::Result<ProjectStatus> {
    let raw: String = row.get("category")?;
    let category =
        StatusCategory::parse(&raw).ok_or_else(|| conversion_error("status category", &raw))?;
    Ok(ProjectStatus {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        name: row.get("name")?,
        category,
        position: row.get("position")?,
        color: row.get("color")?,
    })
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        status_id: row.get("status_id")?,
        title: row.get("title")?,
        notes: row.get("notes")?,
        priority: row.get("priority")?,
        due_at: row.get("due_at")?,
        position: row.get("position")?,
        source: row.get("source")?,
        rev: row.get("rev")?,
        archived: row.get("archived")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        completed_at: row.get("completed_at")?,
        metadata_json: row.get("metadata_json")?,
    })
}

fn claim_from_row(row: &Row<'_>) -> rusqlite::Result<Claim> {
    Ok(Claim {
        id: row.get("id")?,
        task_id: row.get("task_id")?,
        holder_kind: row.get("holder_kind")?,
        holder_name: row.get("holder_name")?,
        thread_ref: row.get("thread_ref")?,
        session_ref: row.get("session_ref")?,
        lease_expires_at: row.get("lease_expires_at")?,
        released_at: row.get("released_at")?,
        outcome: row.get("outcome")?,
        created_at: row.get("created_at")?,
    })
}

fn change_from_row(row: &Row<'_>) -> rusqlite::Result<TaskChange> {
    let kind_raw: String = row.get("kind")?;
    let kind =
        ChangeKind::parse(&kind_raw).ok_or_else(|| conversion_error("change kind", &kind_raw))?;
    let state_raw: String = row.get("state")?;
    let state = ChangeState::parse(&state_raw)
        .ok_or_else(|| conversion_error("change state", &state_raw))?;
    Ok(TaskChange {
        id: row.get("id")?,
        task_id: row.get("task_id")?,
        claim_id: row.get("claim_id")?,
        kind,
        body: row.get("body")?,
        excerpt: row.get("excerpt")?,
        permalink: row.get("permalink")?,
        source_kind: row.get("source_kind")?,
        source_id: row.get("source_id")?,
        confidence: row.get("confidence")?,
        state,
        created_at: row.get("created_at")?,
        acked_at: row.get("acked_at")?,
        superseded_by: row.get("superseded_by")?,
    })
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<TaskEvent> {
    Ok(TaskEvent {
        id: row.get("id")?,
        task_id: row.get("task_id")?,
        kind: row.get("kind")?,
        actor: row.get("actor")?,
        actor_kind: row.get("actor_kind")?,
        body: row.get("body")?,
        data_json: row.get("data_json")?,
        created_at: row.get("created_at")?,
    })
}

fn link_from_row(row: &Row<'_>) -> rusqlite::Result<TaskLink> {
    Ok(TaskLink {
        id: row.get("id")?,
        task_id: row.get("task_id")?,
        kind: row.get("kind")?,
        external_id: row.get("external_id")?,
        url: row.get("url")?,
        title: row.get("title")?,
        state: row.get("state")?,
        metadata_json: row.get("metadata_json")?,
        last_synced_at: row.get("last_synced_at")?,
    })
}

fn proposal_from_row(row: &Row<'_>) -> rusqlite::Result<Proposal> {
    Ok(Proposal {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        project_id: row.get("project_id")?,
        task_id: row.get("task_id")?,
        kind: row.get("kind")?,
        base_rev: row.get("base_rev")?,
        patch_json: row.get("patch_json")?,
        rationale: row.get("rationale")?,
        jev_json: row.get("jev_json")?,
        confidence: row.get("confidence")?,
        state: row.get("state")?,
        decided_by: row.get("decided_by")?,
        decided_at: row.get("decided_at")?,
        applied_event_id: row.get("applied_event_id")?,
    })
}

fn cursor_from_row(row: &Row<'_>) -> rusqlite::Result<SyncCursor> {
    Ok(SyncCursor {
        connector: row.get("connector")?,
        scope: row.get("scope")?,
        cursor: row.get("cursor")?,
        last_synced_at: row.get("last_synced_at")?,
        error: row.get("error")?,
    })
}

fn slack_peer_from_row(row: &Row<'_>) -> rusqlite::Result<SlackPeer> {
    Ok(SlackPeer {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        name: row.get("name")?,
        is_bot: row.get("is_bot")?,
        excluded: row.get("excluded")?,
        created_at: row.get("created_at")?,
    })
}

fn slack_episode_from_row(row: &Row<'_>) -> rusqlite::Result<SlackEpisode> {
    Ok(SlackEpisode {
        id: row.get("id")?,
        channel_id: row.get("channel_id")?,
        peer_id: row.get("peer_id")?,
        state: row.get("state")?,
        started_at: row.get("started_at")?,
        last_message_at: row.get("last_message_at")?,
        closed_at: row.get("closed_at")?,
        summary: row.get("summary")?,
        jev_json: row.get("jev_json")?,
        created_at: row.get("created_at")?,
    })
}

fn slack_message_from_row(row: &Row<'_>) -> rusqlite::Result<SlackMessage> {
    Ok(SlackMessage {
        id: row.get("id")?,
        episode_id: row.get("conversation_id")?,
        peer_id: row.get("peer_id")?,
        channel_id: row.get("channel_id")?,
        ts: row.get("ts")?,
        thread_ts: row.get("thread_ts")?,
        direction: row.get("direction")?,
        user_id: row.get("user_id")?,
        text: row.get("text")?,
        permalink: row.get("permalink")?,
        edited_at: row.get("edited_at")?,
        deleted: row.get("deleted")?,
        historical: row.get("historical")?,
        created_at: row.get("created_at")?,
    })
}

fn job_from_row(row: &Row<'_>) -> rusqlite::Result<AnalysisJob> {
    let candidates_json: String = row.get("candidates_json")?;
    let candidates = serde_json::from_str::<Vec<String>>(&candidates_json).unwrap_or_default();
    Ok(AnalysisJob {
        id: row.get("id")?,
        conversation_id: row.get("conversation_id")?,
        project_id: row.get("project_id")?,
        watermark: row.get("watermark")?,
        kind: row.get("kind")?,
        state: row.get("state")?,
        attempts: row.get("attempts")?,
        route: row.get("route")?,
        jev_json: row.get("jev_json")?,
        candidates,
        summary: row.get("summary")?,
        error: row.get("error")?,
        superseded_by: row.get("superseded_by")?,
        agent_session: row.get("agent_session")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
    })
}

fn job_op_from_row(row: &Row<'_>) -> rusqlite::Result<JobOp> {
    Ok(JobOp {
        id: row.get("id")?,
        job_id: row.get("job_id")?,
        op_index: row.get("op_index")?,
        tool: row.get("tool")?,
        result_json: row.get("result_json")?,
        created_at: row.get("created_at")?,
    })
}

// ---------------------------------------------------------------------------
// Internal reads and writes (all take an existing connection/transaction)
// ---------------------------------------------------------------------------

fn project_by_id(conn: &Connection, id: &str) -> Result<Project, StoreError> {
    conn.query_row(
        &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"),
        params![id],
        project_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("project {id}")))
}

fn status_by_id(conn: &Connection, id: &str) -> Result<ProjectStatus, StoreError> {
    conn.query_row(
        &format!("SELECT {STATUS_COLUMNS} FROM project_statuses WHERE id = ?1"),
        params![id],
        status_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("status {id}")))
}

fn statuses_by_project(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<ProjectStatus>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {STATUS_COLUMNS} FROM project_statuses
         WHERE project_id = ?1
         ORDER BY position, name"
    ))?;
    let rows = stmt.query_map(params![project_id], status_from_row)?;
    let mut statuses = Vec::new();
    for row in rows {
        statuses.push(row?);
    }
    Ok(statuses)
}

fn review_status_for_project(
    conn: &Connection,
    project_id: &str,
) -> Result<Option<ProjectStatus>, StoreError> {
    let status = conn
        .query_row(
            &format!(
                "SELECT {STATUS_COLUMNS} FROM project_statuses
                 WHERE project_id = ?1 AND category = 'review'
                 ORDER BY position, name LIMIT 1"
            ),
            params![project_id],
            status_from_row,
        )
        .optional()?;
    Ok(status)
}

fn task_by_id(conn: &Connection, id: &str) -> Result<Task, StoreError> {
    conn.query_row(
        &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
        params![id],
        task_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("task {id}")))
}

fn claim_by_id(conn: &Connection, id: &str) -> Result<Claim, StoreError> {
    conn.query_row(
        &format!("SELECT {CLAIM_COLUMNS} FROM claims WHERE id = ?1"),
        params![id],
        claim_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("claim {id}")))
}

fn change_by_id(conn: &Connection, id: &str) -> Result<TaskChange, StoreError> {
    conn.query_row(
        &format!("SELECT {CHANGE_COLUMNS} FROM task_changes WHERE id = ?1"),
        params![id],
        change_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("change {id}")))
}

fn proposal_by_id(conn: &Connection, id: &str) -> Result<Proposal, StoreError> {
    conn.query_row(
        &format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id = ?1"),
        params![id],
        proposal_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("proposal {id}")))
}

fn changes_for_task(conn: &Connection, task_id: &str) -> Result<Vec<TaskChange>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHANGE_COLUMNS} FROM task_changes
         WHERE task_id = ?1
         ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map(params![task_id], change_from_row)?;
    let mut changes = Vec::new();
    for row in rows {
        changes.push(row?);
    }
    Ok(changes)
}

fn links_for_task(conn: &Connection, task_id: &str) -> Result<Vec<TaskLink>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {LINK_COLUMNS} FROM task_links WHERE task_id = ?1 ORDER BY kind, rowid"
    ))?;
    let rows = stmt.query_map(params![task_id], link_from_row)?;
    let mut links = Vec::new();
    for row in rows {
        links.push(row?);
    }
    Ok(links)
}

fn events_for_task(conn: &Connection, task_id: &str) -> Result<Vec<TaskEvent>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {EVENT_COLUMNS} FROM task_events
         WHERE task_id = ?1
         ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map(params![task_id], event_from_row)?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

fn active_claim_for_task(
    conn: &Connection,
    task_id: &str,
    at: &str,
) -> Result<Option<Claim>, StoreError> {
    let claim = conn
        .query_row(
            &format!(
                "SELECT {CLAIM_COLUMNS} FROM claims
                 WHERE task_id = ?1 AND released_at IS NULL AND lease_expires_at > ?2
                 ORDER BY created_at DESC, rowid DESC LIMIT 1"
            ),
            params![task_id, at],
            claim_from_row,
        )
        .optional()?;
    Ok(claim)
}

fn expired_claims_for_task(
    conn: &Connection,
    task_id: &str,
    at: &str,
) -> Result<Vec<Claim>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CLAIM_COLUMNS} FROM claims
         WHERE task_id = ?1 AND released_at IS NULL AND lease_expires_at <= ?2
         ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map(params![task_id, at], claim_from_row)?;
    let mut claims = Vec::new();
    for row in rows {
        claims.push(row?);
    }
    Ok(claims)
}

fn ensure_claim_active(claim: &Claim) -> Result<(), StoreError> {
    if claim.released_at.is_some() {
        return Err(StoreError::ClaimReleased(claim.id.clone()));
    }
    if model::is_past(&claim.lease_expires_at) {
        return Err(StoreError::ClaimExpired(claim.id.clone()));
    }
    Ok(())
}

/// Extends the lease to now + 30 minutes and returns the in-memory row.
fn extend_claim(conn: &Connection, claim: &Claim) -> Result<Claim, StoreError> {
    let lease_expires_at = now_plus_minutes(CLAIM_LEASE_MINUTES);
    conn.execute(
        "UPDATE claims SET lease_expires_at = ?1 WHERE id = ?2",
        params![lease_expires_at, claim.id],
    )?;
    Ok(Claim {
        lease_expires_at,
        ..claim.clone()
    })
}

fn unacked_for_claim(conn: &Connection, claim: &Claim) -> Result<Vec<ChangeView>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.task_id, c.claim_id, c.kind, c.body, c.excerpt, c.permalink,
                c.source_kind, c.source_id, c.confidence, c.state, c.created_at,
                c.acked_at, c.superseded_by,
                EXISTS (
                    SELECT 1 FROM change_acks other
                    WHERE other.change_id = c.id
                      AND (other.claim_id IS NULL OR other.claim_id <> ?2)
                ) AS acked_elsewhere
         FROM task_changes c
         WHERE c.task_id = ?1 AND c.state = 'active'
           AND NOT EXISTS (
               SELECT 1 FROM change_acks mine
               WHERE mine.change_id = c.id AND mine.claim_id = ?2
           )
         ORDER BY c.created_at, c.rowid",
    )?;
    let rows = stmt.query_map(params![claim.task_id, claim.id], |row| {
        Ok(ChangeView {
            change: change_from_row(row)?,
            claim_id: Some(claim.id.clone()),
            acked: false,
            acked_elsewhere: row.get("acked_elsewhere")?,
        })
    })?;
    let mut changes = Vec::new();
    for row in rows {
        changes.push(row?);
    }
    Ok(changes)
}

/// The human queue: active changes nobody has acked yet.
fn unacked_for_human(conn: &Connection, task_id: &str) -> Result<Vec<ChangeView>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.task_id, c.claim_id, c.kind, c.body, c.excerpt, c.permalink,
                c.source_kind, c.source_id, c.confidence, c.state, c.created_at,
                c.acked_at, c.superseded_by, 0 AS acked_elsewhere
         FROM task_changes c
         WHERE c.task_id = ?1 AND c.state = 'active'
           AND NOT EXISTS (
               SELECT 1 FROM change_acks any_ack WHERE any_ack.change_id = c.id
           )
         ORDER BY c.created_at, c.rowid",
    )?;
    let rows = stmt.query_map(params![task_id], |row| {
        Ok(ChangeView {
            change: change_from_row(row)?,
            claim_id: None,
            acked: false,
            acked_elsewhere: row.get("acked_elsewhere")?,
        })
    })?;
    let mut changes = Vec::new();
    for row in rows {
        changes.push(row?);
    }
    Ok(changes)
}

fn insert_event(
    conn: &Connection,
    task_id: &str,
    kind: &str,
    actor: Option<&str>,
    actor_kind: Option<&str>,
    body: Option<&str>,
    data: &serde_json::Value,
) -> Result<TaskEvent, StoreError> {
    let event = TaskEvent {
        id: new_id(),
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        actor: actor.map(str::to_string),
        actor_kind: actor_kind.map(str::to_string),
        body: body.map(str::to_string),
        data_json: serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string()),
        created_at: now(),
    };
    conn.execute(
        "INSERT INTO task_events
             (id, task_id, kind, actor, actor_kind, body, data_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event.id,
            event.task_id,
            event.kind,
            event.actor,
            event.actor_kind,
            event.body,
            event.data_json,
            event.created_at,
        ],
    )?;
    Ok(event)
}

/// The single write path for task field changes. Used by `update_task` and by applying
/// an approved proposal; callers own the transaction.
fn apply_task_patch(
    conn: &Connection,
    task_id: &str,
    patch: &TaskPatch,
    expected_rev: i64,
    actor: Option<&str>,
    actor_kind: Option<&str>,
) -> Result<(Task, TaskEvent), StoreError> {
    let task = task_by_id(conn, task_id)?;
    if task.rev != expected_rev {
        return Err(StoreError::StaleRevision {
            id: task_id.to_string(),
            expected: expected_rev,
            actual: task.rev,
        });
    }
    if patch.is_empty() {
        return Err(StoreError::InvalidInput("patch has no changes".into()));
    }

    let mut sets = vec!["rev = rev + 1".to_string(), "updated_at = ?1".to_string()];
    let mut values = vec![SqlValue::Text(now())];
    let mut changed: Vec<&'static str> = Vec::new();

    if let Some(title) = &patch.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(StoreError::InvalidInput(
                "task title cannot be empty".into(),
            ));
        }
        push_set(
            &mut sets,
            &mut values,
            "title",
            SqlValue::Text(title.to_string()),
        );
        changed.push("title");
    }
    if let Some(notes) = &patch.notes {
        push_set(
            &mut sets,
            &mut values,
            "notes",
            SqlValue::Text(notes.clone()),
        );
        changed.push("notes");
    }
    if let Some(priority) = patch.priority {
        let value = match priority {
            Some(priority) => SqlValue::Integer(priority),
            None => SqlValue::Null,
        };
        push_set(&mut sets, &mut values, "priority", value);
        changed.push("priority");
    }
    if let Some(due_at) = &patch.due_at {
        let value = match due_at {
            Some(value) => SqlValue::Text(normalize_time_required(value, "due_at")?),
            None => SqlValue::Null,
        };
        push_set(&mut sets, &mut values, "due_at", value);
        changed.push("due_at");
    }
    if let Some(status_id) = &patch.status_id {
        let status = status_by_id(conn, status_id)?;
        if status.project_id != task.project_id {
            return Err(StoreError::InvalidInput(format!(
                "status {status_id} does not belong to project {}",
                task.project_id
            )));
        }
        push_set(
            &mut sets,
            &mut values,
            "status_id",
            SqlValue::Text(status_id.clone()),
        );
        changed.push("status_id");
    }
    if let Some(position) = patch.position {
        push_set(
            &mut sets,
            &mut values,
            "position",
            SqlValue::Integer(position),
        );
        changed.push("position");
    }
    if let Some(archived) = patch.archived {
        push_set(
            &mut sets,
            &mut values,
            "archived",
            SqlValue::Integer(archived as i64),
        );
        changed.push("archived");
    }
    if let Some(metadata_json) = &patch.metadata_json {
        validate_json(metadata_json, "metadata_json")?;
        push_set(
            &mut sets,
            &mut values,
            "metadata_json",
            SqlValue::Text(metadata_json.clone()),
        );
        changed.push("metadata_json");
    }

    let id_index = values.len() + 1;
    let sql = format!(
        "UPDATE tasks SET {} WHERE id = ?{id_index}",
        sets.join(", ")
    );
    values.push(SqlValue::Text(task_id.to_string()));
    conn.execute(&sql, params_from_iter(values))?;

    let updated = task_by_id(conn, task_id)?;
    let event = insert_event(
        conn,
        task_id,
        "updated",
        actor,
        actor_kind,
        None,
        &serde_json::json!({
            "rev": updated.rev,
            "expectedRev": expected_rev,
            "fields": changed,
        }),
    )?;
    Ok((updated, event))
}

fn push_set(sets: &mut Vec<String>, values: &mut Vec<SqlValue>, column: &str, value: SqlValue) {
    values.push(value);
    sets.push(format!("{column} = ?{}", values.len()));
}

fn next_task_position(conn: &Connection, project_id: &str) -> Result<i64, StoreError> {
    let position = conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM tasks WHERE project_id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    Ok(position)
}

fn proposal_view(proposal: Proposal) -> ProposalView {
    let patch = serde_json::from_str(&proposal.patch_json).unwrap_or(serde_json::Value::Null);
    ProposalView { proposal, patch }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn validate_slug(slug: &str) -> Result<(), StoreError> {
    if slug.is_empty() {
        return Err(StoreError::InvalidInput("project slug is required".into()));
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(StoreError::InvalidInput(format!(
            "project slug must be lowercase letters, digits, '-' or '_': {slug}"
        )));
    }
    Ok(())
}

fn validate_json(value: &str, field: &str) -> Result<(), StoreError> {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(|error| StoreError::InvalidInput(format!("{field} must be valid JSON: {error}")))
}

fn normalize_time_required(value: &str, field: &str) -> Result<String, StoreError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(StoreError::InvalidInput(format!("{field} cannot be empty")));
    }
    let parsed = model::parse_time(value).ok_or_else(|| {
        StoreError::InvalidInput(format!(
            "{field} must be an RFC 3339 timestamp, got `{value}`"
        ))
    })?;
    Ok(model::format_time(parsed))
}

fn none_if_blank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Turns arbitrary user text into literal FTS5 phrase terms. FTS5 allows a
/// double quote inside a quoted string by doubling it; whitespace joins the
/// phrases with the default AND.
fn fts_query(text: &str) -> String {
    text.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Escapes LIKE wildcards so a user's `%` or `_` is literal.
fn like_pattern(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len() + 2);
    pattern.push('%');
    for ch in text.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn conversion_error(column: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("unsupported {column}: {value}").into(),
    )
}

fn validate_state(value: &str, allowed: &[&str], what: &str) -> Result<(), StoreError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(StoreError::InvalidInput(format!(
            "{what} state must be one of {}, got `{value}`",
            allowed.join(", ")
        )))
    }
}

fn parse_rfc3339(value: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

fn within_minutes(last: &str, at: &str, minutes: i64) -> bool {
    match (parse_rfc3339(last), parse_rfc3339(at)) {
        (Some(last), Some(at)) => at - last <= time::Duration::minutes(minutes),
        _ => false,
    }
}

fn within_hours(last: &str, at: &str, hours: i64) -> bool {
    match (parse_rfc3339(last), parse_rfc3339(at)) {
        (Some(last), Some(at)) => at - last <= time::Duration::hours(hours),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use super::*;

    fn store_with_project() -> (Store, Project, Vec<ProjectStatus>) {
        let store = Store::open_in_memory().unwrap();
        let created = store
            .create_project_with_default_workflow(NewProject::new("Relay", "relay", None))
            .unwrap();
        (store, created.project, created.statuses)
    }

    fn status(statuses: &[ProjectStatus], category: StatusCategory) -> &ProjectStatus {
        statuses
            .iter()
            .find(|status| status.category == category)
            .expect("default workflow has every category")
    }

    fn create_task(store: &Store, project: &Project, title: &str) -> Task {
        store
            .create_task(NewTask::new(&project.id, title))
            .expect("task is created")
    }

    fn expire_claim(store: &Store, claim_id: &str) {
        store
            .conn()
            .execute(
                "UPDATE claims SET lease_expires_at = ?1 WHERE id = ?2",
                params!["2000-01-01T00:00:00.000Z", claim_id],
            )
            .unwrap();
    }

    /// A unique file database under the temp directory, removed with its WAL files.
    struct TempDatabase {
        path: PathBuf,
    }

    impl TempDatabase {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir().join(format!("relay-test-{}.db", new_id())),
            }
        }
    }

    impl Drop for TempDatabase {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }

    #[test]
    fn default_workflow_matches_the_plan() {
        let (_store, project, statuses) = store_with_project();
        let actual: Vec<(&str, &str)> = statuses
            .iter()
            .map(|status| (status.name.as_str(), status.category.as_str()))
            .collect();
        assert_eq!(
            actual,
            vec![
                ("Backlog", "backlog"),
                ("Todo", "todo"),
                ("In Progress", "active"),
                ("In Review", "review"),
                ("Merged", "merged"),
                ("Waiting for deploy", "deploying"),
                ("Deployed to RC", "deploying"),
                ("Done", "done"),
                ("Blocked", "blocked"),
            ]
        );
        let positions: Vec<i64> = statuses.iter().map(|status| status.position).collect();
        assert_eq!(positions, (0..9).collect::<Vec<i64>>());
        assert!(
            statuses
                .iter()
                .all(|status| status.project_id == project.id)
        );
    }

    #[test]
    fn project_creation_rejects_a_duplicate_slug() {
        let (store, _project, _statuses) = store_with_project();
        let error = store
            .create_project_with_default_workflow(NewProject::new("Again", "relay", None))
            .unwrap_err();
        assert!(matches!(
            error,
            StoreError::Duplicate {
                entity: "project",
                ..
            }
        ));
        assert_eq!(store.list_projects().unwrap().len(), 1);
    }

    #[test]
    fn task_round_trip_and_stale_rev() {
        let (store, project, statuses) = store_with_project();
        let task = create_task(&store, &project, "Deploy the RC build");

        assert_eq!(task.rev, 1);
        assert_eq!(task.source, "local");
        assert_eq!(task.status_id, status(&statuses, StatusCategory::Todo).id);
        assert_eq!(task.metadata_json, "{}");
        assert_eq!(store.get_task(&task.id).unwrap(), task);

        let detail = store.get_task_detail(&task.id, None).unwrap();
        assert_eq!(detail.task, task);
        assert_eq!(detail.status.category, StatusCategory::Todo);
        assert!(detail.events.iter().any(|event| event.kind == "created"));
        assert!(detail.active_claim.is_none());
        assert!(detail.changes.is_empty());

        let patch = TaskPatch {
            title: Some("Deploy the RC build twice".to_string()),
            priority: Some(Some(1)),
            ..TaskPatch::default()
        };
        let updated = store.update_task(&task.id, patch, 1).unwrap();
        assert_eq!(updated.rev, 2);
        assert_eq!(updated.title, "Deploy the RC build twice");
        assert_eq!(updated.priority, Some(1));
        assert!(updated.updated_at >= task.updated_at);

        let stale = TaskPatch {
            notes: Some("must not land".to_string()),
            ..TaskPatch::default()
        };
        let error = store.update_task(&task.id, stale, 1).unwrap_err();
        assert!(matches!(
            error,
            StoreError::StaleRevision {
                expected: 1,
                actual: 2,
                ..
            }
        ));
        assert_eq!(store.get_task(&task.id).unwrap().notes, task.notes);

        let detail = store.get_task_detail(&task.id, None).unwrap();
        assert_eq!(detail.task.rev, 2);
        assert!(detail.events.iter().any(|event| event.kind == "updated"));
    }

    #[test]
    fn second_claim_is_rejected_with_the_holder() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Claim me");

        let first = store
            .claim_task(NewClaim::new(&task.id, "agent", "T3 Code"))
            .unwrap();
        assert_eq!(first.claim.task_id, task.id);
        assert!(first.claim.released_at.is_none());
        assert!(first.unacked_changes.is_empty());

        let error = store
            .claim_task(NewClaim::new(&task.id, "agent", "OpenCode"))
            .unwrap_err();
        match error {
            StoreError::ClaimConflict { holder, .. } => {
                assert_eq!(holder.id, first.claim.id);
                assert_eq!(holder.holder_name, "T3 Code");
            }
            other => panic!("expected a claim conflict, got {other:?}"),
        }

        // A claim from another task cannot read this task's detail or ack its changes.
        let other_task = create_task(&store, &project, "Another task");
        let other_claim = store
            .claim_task(NewClaim::new(&other_task.id, "agent", "OpenCode"))
            .unwrap();
        let error = store
            .get_task_detail(&task.id, Some(&other_claim.claim.id))
            .unwrap_err();
        assert!(matches!(error, StoreError::ClaimTaskMismatch { .. }));
    }

    #[test]
    fn expired_lease_can_be_reclaimed() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Reclaim me");

        let first = store
            .claim_task(NewClaim::new(&task.id, "agent", "T3 Code"))
            .unwrap();
        expire_claim(&store, &first.claim.id);

        let error = store.heartbeat(&first.claim.id).unwrap_err();
        assert!(matches!(error, StoreError::ClaimExpired(_)));

        let mut late = NewChange::requirement(&task.id, "Late requirement", "slack");
        late.claim_id = Some(first.claim.id.clone());
        let error = store.add_change(late).unwrap_err();
        assert!(
            matches!(error, StoreError::ClaimExpired(_)),
            "an expired claim cannot write"
        );

        let second = store
            .claim_task(NewClaim::new(&task.id, "agent", "OpenCode"))
            .unwrap();
        assert_ne!(second.claim.id, first.claim.id);

        let expired = store.get_claim(&first.claim.id).unwrap();
        assert!(expired.released_at.is_some());
        assert_eq!(expired.outcome.as_deref(), Some("expired"));

        let error = store.add_progress(&first.claim.id, "too late").unwrap_err();
        assert!(matches!(error, StoreError::ClaimReleased(_)));

        // The new claim is the active one.
        let detail = store.get_task_detail(&task.id, None).unwrap();
        assert_eq!(detail.active_claim.unwrap().id, second.claim.id);
    }

    #[test]
    fn acks_are_isolated_between_claims() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Ack me");

        let first = store
            .claim_task(NewClaim::new(&task.id, "agent", "T3 Code"))
            .unwrap();
        let change = store
            .add_change(NewChange::requirement(
                &task.id,
                "Also the RC build",
                "slack",
            ))
            .unwrap();

        let unacked = store.unacked_changes(&first.claim.id).unwrap();
        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].change.id, change.id);
        assert!(!unacked[0].acked);

        let detail = store
            .get_task_detail(&task.id, Some(&first.claim.id))
            .unwrap();
        assert_eq!(detail.active_claim.as_ref().unwrap().id, first.claim.id);
        assert_eq!(detail.unacked_changes.len(), 1);

        let acked = store.ack_change(Some(&first.claim.id), &change.id).unwrap();
        assert!(acked.change.acked);
        assert!(acked.unacked_changes.is_empty());
        assert!(store.unacked_changes(&first.claim.id).unwrap().is_empty());

        store.release_claim(&first.claim.id, "blocked").unwrap();
        let second = store
            .claim_task(NewClaim::new(&task.id, "agent", "OpenCode"))
            .unwrap();
        let unacked = store.unacked_changes(&second.claim.id).unwrap();
        assert_eq!(unacked.len(), 1, "a new claimant does not inherit acks");
        assert!(unacked[0].acked_elsewhere);

        // A human ack is its own scope: it clears the human queue but not the agent's.
        let human = store.ack_change(None, &change.id).unwrap();
        assert!(human.unacked_changes.is_empty());
        // Replaying the human ack is a no-op, not a unique-index error.
        assert!(
            store
                .ack_change(None, &change.id)
                .unwrap()
                .unacked_changes
                .is_empty()
        );
        assert!(
            store
                .get_task_detail(&task.id, None)
                .unwrap()
                .unacked_changes
                .is_empty()
        );
        assert_eq!(store.unacked_changes(&second.claim.id).unwrap().len(), 1);

        let acked = store
            .ack_change(Some(&second.claim.id), &change.id)
            .unwrap();
        assert!(acked.unacked_changes.is_empty());
        assert!(store.unacked_changes(&second.claim.id).unwrap().is_empty());
    }

    #[test]
    fn complete_moves_to_review_and_releases_the_claim() {
        let (store, project, statuses) = store_with_project();
        let task = create_task(&store, &project, "Finish me");
        let claim = store
            .claim_task(NewClaim::new(&task.id, "agent", "T3 Code"))
            .unwrap();

        let completed = store
            .complete_task(&claim.claim.id, "Opened the PR")
            .unwrap();
        assert_eq!(
            completed.status_id,
            status(&statuses, StatusCategory::Review).id
        );
        assert_eq!(completed.rev, 2);
        assert!(completed.completed_at.is_some());

        let stored = store.get_claim(&claim.claim.id).unwrap();
        assert_eq!(stored.outcome.as_deref(), Some("completed"));
        assert!(stored.released_at.is_some());

        let detail = store.get_task_detail(&task.id, None).unwrap();
        assert!(detail.active_claim.is_none());
        let completed_events: Vec<&TaskEvent> = detail
            .events
            .iter()
            .filter(|event| event.kind == "completed")
            .collect();
        assert_eq!(completed_events.len(), 1);
        assert_eq!(completed_events[0].body.as_deref(), Some("Opened the PR"));

        // Replaying completion is harmless; other writes on a released claim are not.
        let replayed = store
            .complete_task(&claim.claim.id, "Opened the PR")
            .unwrap();
        assert_eq!(replayed.rev, 2);
        let error = store
            .add_progress(&claim.claim.id, "still here")
            .unwrap_err();
        assert!(matches!(error, StoreError::ClaimReleased(_)));
        let error = store
            .release_claim(&claim.claim.id, "abandoned")
            .unwrap_err();
        assert!(matches!(error, StoreError::ClaimReleased(_)));
    }

    #[test]
    fn filters_and_substring_search() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .create_project_with_default_workflow(NewProject::new("Relay", "relay", None))
            .unwrap();
        let second = store
            .create_project_with_default_workflow(NewProject::new("Other", "other", None))
            .unwrap();

        let deploy = store
            .create_task(NewTask::new(&first.project.id, "Deploy the RC build"))
            .unwrap();
        let mut docs = NewTask::new(&first.project.id, "Write docs");
        docs.notes = Some("README and the upgrade guide".to_string());
        docs.status_id = Some(status(&first.statuses, StatusCategory::Active).id.clone());
        let docs = store.create_task(docs).unwrap();
        let elsewhere = store
            .create_task(NewTask::new(&second.project.id, "Deploy elsewhere"))
            .unwrap();

        let all = store.list_tasks(TaskFilter::default()).unwrap();
        assert_eq!(all.len(), 3);

        let in_relay = store
            .list_tasks(TaskFilter {
                project_id: Some(first.project.id.clone()),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(in_relay.len(), 2);

        let active = store
            .list_tasks(TaskFilter {
                category: Some(StatusCategory::Active),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, docs.id);
        assert_eq!(
            docs.status_id,
            status(&first.statuses, StatusCategory::Active).id,
            "explicit status wins over the todo default"
        );

        let text = store
            .list_tasks(TaskFilter {
                text: Some("deploy".to_string()),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(text.len(), 2);
        assert!(text.iter().any(|task| task.id == deploy.id));
        assert!(text.iter().any(|task| task.id == elsewhere.id));

        let text_in_relay = store
            .list_tasks(TaskFilter {
                project_id: Some(first.project.id.clone()),
                text: Some("DEPLOY".to_string()),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(text_in_relay.len(), 1);

        let notes = store
            .list_tasks(TaskFilter {
                text: Some("upgrade".to_string()),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, docs.id);

        let since = store
            .list_tasks(TaskFilter {
                updated_since: Some("2000-01-01T00:00:00Z".to_string()),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(since.len(), 3);
        let future = store
            .list_tasks(TaskFilter {
                updated_since: Some("2999-01-01T00:00:00Z".to_string()),
                ..TaskFilter::default()
            })
            .unwrap();
        assert!(future.is_empty());

        let limited = store
            .list_tasks(TaskFilter {
                limit: Some(1),
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(limited.len(), 1);

        // Archive through the patch path, then confirm the default filter hides it.
        store
            .update_task(
                &docs.id,
                TaskPatch {
                    archived: Some(true),
                    ..TaskPatch::default()
                },
                1,
            )
            .unwrap();
        assert_eq!(store.list_tasks(TaskFilter::default()).unwrap().len(), 2);
        let with_archived = store
            .list_tasks(TaskFilter {
                include_archived: true,
                ..TaskFilter::default()
            })
            .unwrap();
        assert_eq!(with_archived.len(), 3);
    }

    #[test]
    fn links_dedupe_by_external_id_or_url() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Link me");

        let mut first = NewLink::new(&task.id, "pr", "https://github.com/uFleck/relay/pull/1");
        first.external_id = Some("1".to_string());
        first.title = Some("first".to_string());
        first.metadata_json = Some("{\"checks\":\"green\"}".to_string());
        let first = store.add_link(first).unwrap();

        let mut again = NewLink::new(&task.id, "pr", "https://github.com/uFleck/relay/pull/1");
        again.external_id = Some("1".to_string());
        again.title = Some("second".to_string());
        let again = store.add_link(again).unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.title.as_deref(), Some("second"));
        assert_eq!(
            again.metadata_json, "{\"checks\":\"green\"}",
            "re-adding without metadata keeps the stored metadata"
        );
        assert_eq!(store.list_links(&task.id).unwrap().len(), 1);

        let url_only = NewLink::new(&task.id, "url", "https://example.com/spec");
        let url_first = store.add_link(url_only).unwrap();
        let url_again = store
            .add_link(NewLink::new(&task.id, "url", "https://example.com/spec"))
            .unwrap();
        assert_eq!(url_first.id, url_again.id);
        assert_eq!(store.list_links(&task.id).unwrap().len(), 2);

        let branch = store
            .add_link(NewLink::new(&task.id, "branch", "relay-claim"))
            .unwrap();
        assert_eq!(branch.kind, "branch");
        assert_eq!(store.list_links(&task.id).unwrap().len(), 3);
    }

    #[test]
    fn changes_retract_and_supersede_without_deleting() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Track changes");

        let old = store
            .add_change(NewChange::requirement(&task.id, "Ship v1", "slack"))
            .unwrap();
        let new = store
            .add_change(NewChange::requirement(&task.id, "Ship v2", "slack"))
            .unwrap();

        let superseded = store.supersede_change(&old.id, &new.id).unwrap();
        assert_eq!(superseded.state, ChangeState::Superseded);
        assert_eq!(superseded.superseded_by.as_deref(), Some(new.id.as_str()));

        let retracted = store.retract_change(&new.id).unwrap();
        assert_eq!(retracted.state, ChangeState::Retracted);

        let changes = store.list_changes(&task.id).unwrap();
        assert_eq!(changes.len(), 2, "history is kept");
        assert!(
            changes
                .iter()
                .all(|change| change.state != ChangeState::Active)
        );

        // Retracting twice is idempotent; a superseded change cannot be retracted.
        assert_eq!(
            store.retract_change(&new.id).unwrap().state,
            ChangeState::Retracted
        );
        let error = store.retract_change(&old.id).unwrap_err();
        assert!(matches!(error, StoreError::InvalidInput(_)));
    }

    #[test]
    fn progress_extends_the_lease_and_returns_unacked_changes() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Work in progress");
        let claim = store
            .claim_task(NewClaim::new(&task.id, "agent", "T3 Code"))
            .unwrap();
        let change = store
            .add_change(NewChange::requirement(
                &task.id,
                "Handle the RC build",
                "slack",
            ))
            .unwrap();

        let near = now_plus_minutes(1);
        store
            .conn()
            .execute(
                "UPDATE claims SET lease_expires_at = ?1 WHERE id = ?2",
                params![near, claim.claim.id],
            )
            .unwrap();

        let update = store
            .add_progress(&claim.claim.id, "Halfway through")
            .unwrap();
        assert!(update.claim.lease_expires_at > near);
        assert_eq!(update.event.kind, "progress");
        assert_eq!(update.event.body.as_deref(), Some("Halfway through"));
        assert_eq!(update.unacked_changes.len(), 1);
        assert_eq!(update.unacked_changes[0].change.id, change.id);

        let heartbeat = store.heartbeat(&claim.claim.id).unwrap();
        assert!(heartbeat.lease_expires_at >= update.claim.lease_expires_at);
    }

    #[test]
    fn proposals_apply_only_known_fields() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Ship it");

        let created = store
            .create_proposal(NewProposal {
                job_id: None,
                project_id: project.id.clone(),
                task_id: Some(task.id.clone()),
                kind: "field_change".to_string(),
                base_rev: Some(1),
                patch: serde_json::json!({
                    "title": "Ship it properly",
                    "unknownField": true
                }),
                rationale: Some("The DM asked for it".to_string()),
                jev_json: None,
                confidence: Some(0.91),
            })
            .unwrap();
        assert_eq!(created.proposal.state, "pending");
        assert_eq!(created.patch["title"], "Ship it properly");

        let approved = store
            .decide_proposal(&created.proposal.id, ProposalDecision::Approve, "bruno")
            .unwrap();
        assert_eq!(approved.proposal.state, "approved");
        assert_eq!(approved.proposal.decided_by.as_deref(), Some("bruno"));
        assert!(approved.proposal.applied_event_id.is_some());

        let task_after = store.get_task(&task.id).unwrap();
        assert_eq!(task_after.title, "Ship it properly");
        assert_eq!(task_after.rev, 2);

        let stale = store
            .create_proposal(NewProposal {
                job_id: None,
                project_id: project.id.clone(),
                task_id: Some(task.id.clone()),
                kind: "field_change".to_string(),
                base_rev: Some(1),
                patch: serde_json::json!({"title": "Too late"}),
                rationale: None,
                jev_json: None,
                confidence: None,
            })
            .unwrap();
        let error = store
            .decide_proposal(&stale.proposal.id, ProposalDecision::Approve, "bruno")
            .unwrap_err();
        assert!(matches!(error, StoreError::StaleRevision { .. }));
        assert_eq!(store.get_task(&task.id).unwrap().title, "Ship it properly");
        assert!(
            store
                .list_proposals(Some("pending"))
                .unwrap()
                .iter()
                .any(|proposal| proposal.proposal.id == stale.proposal.id)
        );

        let rejected = store
            .create_proposal(NewProposal {
                job_id: None,
                project_id: project.id.clone(),
                task_id: Some(task.id.clone()),
                kind: "field_change".to_string(),
                base_rev: Some(2),
                patch: serde_json::json!({"title": "Never"}),
                rationale: None,
                jev_json: None,
                confidence: None,
            })
            .unwrap();
        let decided = store
            .decide_proposal(&rejected.proposal.id, ProposalDecision::Reject, "bruno")
            .unwrap();
        assert_eq!(decided.proposal.state, "rejected");
        assert_eq!(store.get_task(&task.id).unwrap().title, "Ship it properly");

        let error = store
            .decide_proposal(&rejected.proposal.id, ProposalDecision::Approve, "bruno")
            .unwrap_err();
        assert!(matches!(error, StoreError::ProposalDecided(_)));
    }

    #[test]
    fn settings_and_sync_cursors_round_trip() {
        let store = Store::open_in_memory().unwrap();

        assert_eq!(store.get_setting("theme").unwrap(), None);
        store.set_setting("theme", "dark").unwrap();
        assert_eq!(store.get_setting("theme").unwrap().as_deref(), Some("dark"));
        store.set_setting("theme", "light").unwrap();
        assert_eq!(
            store.get_setting("theme").unwrap().as_deref(),
            Some("light")
        );
        assert_eq!(store.list_settings().unwrap().len(), 1);

        let cursor = SyncCursor {
            connector: "slack".to_string(),
            scope: "dm".to_string(),
            cursor: Some("c1".to_string()),
            last_synced_at: Some("2026-11-17T12:00:00Z".to_string()),
            error: None,
        };
        store.set_sync_cursor(&cursor).unwrap();
        let stored = store.get_sync_cursor("slack", "dm").unwrap().unwrap();
        assert_eq!(stored.cursor.as_deref(), Some("c1"));
        assert_eq!(
            stored.last_synced_at.as_deref(),
            Some("2026-11-17T12:00:00.000Z")
        );

        let updated = store
            .upsert_sync_cursor("slack", "dm", Some("c2"), None)
            .unwrap();
        assert_eq!(updated.cursor.as_deref(), Some("c2"));
        assert!(updated.last_synced_at.is_some());

        let kept = store
            .upsert_sync_cursor("slack", "dm", None, Some("rate limited"))
            .unwrap();
        assert_eq!(kept.cursor.as_deref(), Some("c2"));
        assert_eq!(kept.error.as_deref(), Some("rate limited"));

        assert!(store.get_sync_cursor("slack", "missing").unwrap().is_none());
    }

    #[test]
    fn add_event_and_list_events() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Eventful");

        let event = store
            .add_event(NewEvent {
                task_id: task.id.clone(),
                kind: "git_branch".to_string(),
                actor: Some("git".to_string()),
                actor_kind: Some("system".to_string()),
                body: Some("relay-claim".to_string()),
                data_json: Some("{\"branch\":\"relay-claim\"}".to_string()),
            })
            .unwrap();
        assert_eq!(event.kind, "git_branch");

        let events = store.list_events(&task.id).unwrap();
        assert_eq!(events.len(), 2, "created plus the manual event");
        assert!(events.iter().any(|candidate| candidate.id == event.id));

        let error = store
            .add_event(NewEvent {
                task_id: "missing".to_string(),
                kind: "note".to_string(),
                actor: None,
                actor_kind: None,
                body: None,
                data_json: None,
            })
            .unwrap_err();
        assert!(matches!(error, StoreError::NotFound(_)));
    }

    #[test]
    fn file_database_is_wal_and_migrated() {
        let path = std::env::temp_dir().join(format!("relay-{}.db", new_id()));
        let store = Store::open(&path).unwrap();
        let mode: String = store
            .conn()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let version: i64 = store
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn fts_index_tracks_tasks() {
        let (store, project, _statuses) = store_with_project();

        let mut new_task = NewTask::new(&project.id, "Deploy the RC build");
        new_task.notes = Some("Release notes and rollback steps".to_string());
        let task = store.create_task(new_task).unwrap();

        // Insert: title and notes are both indexed, case-insensitively.
        assert_eq!(store.search_tasks("deploy", 10).unwrap()[0].id, task.id);
        assert_eq!(store.search_tasks("DEPLOY", 10).unwrap()[0].id, task.id);
        assert_eq!(store.search_tasks("rollback", 10).unwrap()[0].id, task.id);
        assert!(store.search_tasks("   ", 10).unwrap().is_empty());
        assert!(store.search_tasks("deploy", 0).unwrap().is_empty());

        // Update: the trigger removes the old terms and indexes the new ones.
        let updated = store
            .update_task(
                &task.id,
                TaskPatch {
                    title: Some("Ship the canary".to_string()),
                    notes: Some("Smoke test the new build".to_string()),
                    ..TaskPatch::default()
                },
                1,
            )
            .unwrap();
        assert!(store.search_tasks("deploy", 10).unwrap().is_empty());
        assert!(store.search_tasks("rollback", 10).unwrap().is_empty());
        assert_eq!(store.search_tasks("canary", 10).unwrap()[0].id, task.id);
        assert_eq!(store.search_tasks("smoke", 10).unwrap()[0].id, task.id);

        // Archive: hidden, exactly like list_tasks' default filter.
        let archived = store
            .update_task(
                &task.id,
                TaskPatch {
                    archived: Some(true),
                    ..TaskPatch::default()
                },
                updated.rev,
            )
            .unwrap();
        assert!(store.search_tasks("canary", 10).unwrap().is_empty());
        assert!(
            store
                .list_tasks(TaskFilter {
                    text: Some("canary".to_string()),
                    ..TaskFilter::default()
                })
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .list_tasks(TaskFilter {
                    text: Some("canary".to_string()),
                    include_archived: true,
                    ..TaskFilter::default()
                })
                .unwrap()
                .len(),
            1,
            "the row is still listed when archived rows are requested"
        );

        // Unarchive: searchable again.
        store
            .update_task(
                &task.id,
                TaskPatch {
                    archived: Some(false),
                    ..TaskPatch::default()
                },
                archived.rev,
            )
            .unwrap();
        assert_eq!(store.search_tasks("canary", 10).unwrap()[0].id, task.id);

        // Delete: the FTS trigger removes the index row. There is no delete API.
        store
            .conn()
            .execute("DELETE FROM tasks WHERE id = ?1", params![task.id])
            .unwrap();
        assert!(store.search_tasks("canary", 10).unwrap().is_empty());
    }

    #[test]
    fn fts_search_escapes_operators_and_wildcards_and_honors_the_limit() {
        let (store, project, _statuses) = store_with_project();

        let deploy = create_task(&store, &project, "Deploy relay");
        let deployment = create_task(&store, &project, "Deployment rollback");
        let quoted = create_task(&store, &project, "Handle \"quoted\" input");
        let percent = create_task(&store, &project, "Reach 100% coverage");

        // `*` is literal, not a prefix operator; quotes are escaped, not syntax.
        let literal = store.search_tasks("deploy*", 10).unwrap();
        assert_eq!(
            literal.len(),
            1,
            "a trailing * must not become a prefix query"
        );
        assert_eq!(literal[0].id, deploy.id);
        assert_ne!(literal[0].id, deployment.id);

        let unbalanced = store.search_tasks("\"deploy", 10).unwrap();
        assert_eq!(unbalanced.len(), 1);
        assert_eq!(unbalanced[0].id, deploy.id);

        assert!(
            store
                .search_tasks("deploy\" OR \"missing", 10)
                .unwrap()
                .is_empty(),
            "FTS operators in quotes stay literal terms"
        );
        assert!(
            store
                .search_tasks("deploy OR missing", 10)
                .unwrap()
                .is_empty(),
            "OR is a term here, so all three terms must match"
        );
        assert!(
            store.search_tasks("title:deploy", 10).unwrap().is_empty(),
            "a column filter is not interpreted"
        );
        assert_eq!(
            store.search_tasks("\"quoted\"", 10).unwrap()[0].id,
            quoted.id
        );
        assert_eq!(store.search_tasks("100%", 10).unwrap()[0].id, percent.id);

        // Limit: exact, capped by the candidate count, and 0 is empty.
        for index in 0..3 {
            create_task(&store, &project, &format!("Release batch {index}"));
        }
        assert_eq!(store.search_tasks("release", 2).unwrap().len(), 2);
        assert_eq!(store.search_tasks("release", 10).unwrap().len(), 3);
        assert!(store.search_tasks("release", 0).unwrap().is_empty());
    }

    #[test]
    fn upgrade_from_0003_keeps_episodes_and_queues_them_for_a_summary() {
        let db = TempDatabase::new();
        {
            let conn = Connection::open(&db.path).unwrap();
            for sql in &MIGRATIONS[..3] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 3).unwrap();
            conn.execute(
                "INSERT INTO conversations (id, channel_id, peer_id, opened_at, last_message_at)
                 VALUES ('E1', 'D1', 'P1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages (id, conversation_id, ts, author_id, direction, text)
                 VALUES ('M1', 'E1', '1767225660.000100', 'U1', 'in', 'faz sentido')",
                [],
            )
            .unwrap();
        }

        let store = Store::open(&db.path).unwrap();
        assert_eq!(
            store.episodes_needing_summary(&now(), 10).unwrap(),
            vec!["E1".to_string()]
        );
        store
            .set_episode_summary("E1", "Deploy do bill", "2026-01-01T00:01:00Z")
            .unwrap();
        assert!(
            store
                .episodes_needing_summary(&now(), 10)
                .unwrap()
                .is_empty()
        );
        let listed = store.list_slack_episodes(None, 10).unwrap();
        assert_eq!(listed[0].episode.summary.as_deref(), Some("Deploy do bill"));
        assert_eq!(listed[0].preview.as_deref(), Some("faz sentido"));
    }

    #[test]
    fn upgrade_from_0004_keeps_tasks_and_adds_searchable_notes() {
        let db = TempDatabase::new();
        let task_id = {
            let store = Store::open(&db.path).unwrap();
            let project = store
                .create_project_with_default_workflow(NewProject::new("Relay", "relay", None))
                .unwrap()
                .project;
            let task = create_task(&store, &project, "Ship notes");
            let conn = store.conn();
            conn.execute_batch("DROP TABLE notes_fts; DROP TABLE notes;")
                .unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
            task.id
        };

        let store = Store::open(&db.path).unwrap();
        let version: i64 = store
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        assert_eq!(store.get_task(&task_id).unwrap().title, "Ship notes");
        let note = store.create_note("Kept", "after the upgrade").unwrap();
        assert_eq!(store.list_notes(Some("upgr"), None).unwrap(), vec![note]);
    }

    #[test]
    fn upgrade_from_0001_keeps_rows_and_allows_acks() {
        let db = TempDatabase::new();

        let project_id = new_id();
        let status_id = new_id();
        let task_id = new_id();
        let claim_id = new_id();
        let change_id = new_id();
        let at = now();
        let lease_expires_at = now_plus_minutes(10);

        // Build a v1 database by hand: only 0001, then user_version = 1.
        {
            let conn = Connection::open(&db.path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO projects (id, name, slug, created_at, updated_at)
                 VALUES (?1, 'Relay', 'relay', ?2, ?2)",
                params![project_id, at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project_statuses (id, project_id, name, category, position)
                 VALUES (?1, ?2, 'Todo', 'todo', 0)",
                params![status_id, project_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, project_id, status_id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'Upgrade me', ?4, ?4)",
                params![task_id, project_id, status_id, at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO claims
                     (id, task_id, holder_kind, holder_name, lease_expires_at, created_at)
                 VALUES (?1, ?2, 'agent', 'T3 Code', ?3, ?4)",
                params![claim_id, task_id, lease_expires_at, at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO task_changes
                     (id, task_id, claim_id, kind, body, source_kind, created_at)
                 VALUES (?1, ?2, ?3, 'requirement', 'Survive the upgrade', 'slack', ?4)",
                params![change_id, task_id, claim_id, at],
            )
            .unwrap();
        }

        let store = Store::open(&db.path).unwrap();
        let version: i64 = store
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);

        let change_acks_table: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'change_acks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(change_acks_table, 1, "0002 created change_acks");

        assert_eq!(store.get_task(&task_id).unwrap().title, "Upgrade me");
        assert_eq!(store.get_claim(&claim_id).unwrap().holder_name, "T3 Code");
        let changes = store.list_changes(&task_id).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].id, change_id);
        assert!(changes[0].acked_at.is_none());

        // The new ack path works on the migrated rows.
        let acked = store.ack_change(Some(&claim_id), &change_id).unwrap();
        assert!(acked.change.acked);
        assert!(acked.unacked_changes.is_empty());
        let changes = store.list_changes(&task_id).unwrap();
        assert!(changes[0].acked_at.is_some());

        // Reopening skips 0002 instead of trying to create the table again.
        drop(store);
        let reopened = Store::open(&db.path).unwrap();
        let version: i64 = reopened
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let ack_rows: i64 = reopened
            .conn()
            .query_row("SELECT COUNT(*) FROM change_acks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(ack_rows, 1);
    }

    #[test]
    fn two_connections_race_for_the_same_claim() {
        let db = TempDatabase::new();
        let setup = Store::open(&db.path).unwrap();
        let created = setup
            .create_project_with_default_workflow(NewProject::new("Relay", "relay", None))
            .unwrap();
        let task = setup
            .create_task(NewTask::new(&created.project.id, "Race me"))
            .unwrap();
        drop(setup);

        let barrier = Arc::new(Barrier::new(2));
        let spawn = |holder: &'static str| {
            let store = Store::open(&db.path).unwrap();
            let barrier = Arc::clone(&barrier);
            let task_id = task.id.clone();
            thread::spawn(move || {
                barrier.wait();
                store.claim_task(NewClaim::new(&task_id, "agent", holder))
            })
        };
        let results: Vec<_> = [spawn("T3 Code"), spawn("OpenCode")]
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        let mut winners = results.iter().filter_map(|result| result.as_ref().ok());
        let winner = winners
            .next()
            .unwrap_or_else(|| panic!("no claimant won: {results:?}"));
        assert!(winners.next().is_none(), "exactly one claimant wins");
        let error = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .unwrap_or_else(|| panic!("expected one loser: {results:?}"));
        match error {
            StoreError::ClaimConflict { task_id, holder } => {
                assert_eq!(task_id, &task.id);
                assert_eq!(holder.id, winner.claim.id, "the loser sees the winner");
                assert_eq!(holder.holder_name, winner.claim.holder_name);
            }
            other => panic!("expected a typed claim conflict, got {other:?}"),
        }

        // One claim row, and it is the winner's.
        let verify = Store::open(&db.path).unwrap();
        let claims: i64 = verify
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM claims WHERE task_id = ?1",
                params![task.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(claims, 1);
        assert_eq!(
            verify
                .get_task_detail(&task.id, None)
                .unwrap()
                .active_claim
                .unwrap()
                .id,
            winner.claim.id
        );
    }

    #[test]
    fn concurrent_updates_with_the_same_expected_rev_advance_once() {
        let db = TempDatabase::new();
        let setup = Store::open(&db.path).unwrap();
        let created = setup
            .create_project_with_default_workflow(NewProject::new("Relay", "relay", None))
            .unwrap();
        let task = setup
            .create_task(NewTask::new(&created.project.id, "Update me"))
            .unwrap();
        drop(setup);

        let barrier = Arc::new(Barrier::new(2));
        let spawn = |title: &'static str| {
            let store = Store::open(&db.path).unwrap();
            let barrier = Arc::clone(&barrier);
            let task_id = task.id.clone();
            thread::spawn(move || {
                barrier.wait();
                store.update_task(
                    &task_id,
                    TaskPatch {
                        title: Some(title.to_string()),
                        ..TaskPatch::default()
                    },
                    1,
                )
            })
        };
        let results: Vec<_> = [spawn("First patch"), spawn("Second patch")]
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "exactly one update wins: {results:?}"
        );
        let error = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .unwrap_or_else(|| panic!("expected one stale writer: {results:?}"));
        assert!(
            matches!(
                error,
                StoreError::StaleRevision {
                    expected: 1,
                    actual: 2,
                    ..
                }
            ),
            "the loser sees the committed rev, got {error:?}"
        );

        let verify = Store::open(&db.path).unwrap();
        let stored = verify.get_task(&task.id).unwrap();
        assert_eq!(stored.rev, 2, "rev advances exactly once");
        assert!(
            stored.title == "First patch" || stored.title == "Second patch",
            "the winning patch landed: {}",
            stored.title
        );
        let updates = verify
            .list_events(&task.id)
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "updated")
            .count();
        assert_eq!(updates, 1, "the losing transaction wrote no event");
    }

    #[test]
    fn human_ack_does_not_clear_an_agent_queue_and_stamps_acked_at() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "Two ack scopes");
        let claim = store
            .claim_task(NewClaim::new(&task.id, "agent", "T3 Code"))
            .unwrap();
        let change = store
            .add_change(NewChange::requirement(
                &task.id,
                "Keep the API stable",
                "slack",
            ))
            .unwrap();

        // Current, intentional semantics: a human ack is `claim_id = NULL`.
        let human = store.ack_change(None, &change.id).unwrap();
        assert!(human.change.acked);
        assert_eq!(human.change.claim_id, None);
        assert!(
            human.unacked_changes.is_empty(),
            "the human queue is cleared"
        );

        let changes = store.list_changes(&task.id).unwrap();
        assert_eq!(changes.len(), 1);
        let first_ack = changes[0]
            .acked_at
            .clone()
            .expect("the first ack stamps task_changes.acked_at");

        // The agent's queue is a separate scope: still one unacked change.
        let agent_queue = store.unacked_changes(&claim.claim.id).unwrap();
        assert_eq!(
            agent_queue.len(),
            1,
            "a human ack must not clear an agent's queue"
        );
        assert!(agent_queue[0].acked_elsewhere);
        assert!(!agent_queue[0].acked);
        assert_eq!(
            store
                .get_task_detail(&task.id, Some(&claim.claim.id))
                .unwrap()
                .unacked_changes
                .len(),
            1
        );

        // The claim's own ack drains it and leaves the first-ack timeline value alone.
        let agent = store.ack_change(Some(&claim.claim.id), &change.id).unwrap();
        assert!(agent.unacked_changes.is_empty());
        assert!(store.unacked_changes(&claim.claim.id).unwrap().is_empty());
        assert!(
            store
                .get_task_detail(&task.id, None)
                .unwrap()
                .unacked_changes
                .is_empty()
        );
        assert_eq!(
            store.list_changes(&task.id).unwrap()[0].acked_at.as_deref(),
            Some(first_ack.as_str()),
            "the first ack time is not overwritten"
        );
    }

    fn raw_message(ts: &str, user: &str, text: &str) -> RawSlackMessage {
        RawSlackMessage {
            channel_id: "D01".into(),
            ts: ts.into(),
            thread_ts: None,
            user_id: Some(user.into()),
            text: text.into(),
            permalink: Some(format!("https://slack.test/archives/D01/p{ts}")),
            edited_at: None,
            deleted: false,
            historical: false,
        }
    }

    #[test]
    fn a_project_repo_path_can_be_set_and_cleared() {
        let (store, project, _statuses) = store_with_project();
        let updated = store
            .set_project_repo(&project.id, Some("C:\\Projects\\relay"))
            .unwrap();
        assert_eq!(updated.repo_path.as_deref(), Some("C:\\Projects\\relay"));
        let cleared = store.set_project_repo(&project.id, None).unwrap();
        assert!(cleared.repo_path.is_none());
        assert!(matches!(
            store.set_project_repo("missing", Some("x")),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn links_can_be_found_for_idempotent_imports() {
        let (store, project, _statuses) = store_with_project();
        let task = create_task(&store, &project, "From ClickUp");
        store
            .add_link(NewLink {
                task_id: task.id.clone(),
                claim_id: None,
                kind: "clickup".into(),
                external_id: Some("abc".into()),
                url: "https://app.clickup.com/t/abc".into(),
                title: None,
                state: Some("todo".into()),
                metadata_json: None,
                last_synced_at: None,
            })
            .unwrap();
        let by_id = store
            .find_link("clickup", Some("abc"), "https://example.invalid")
            .unwrap()
            .expect("external id matches");
        assert_eq!(by_id.task_id, task.id);
        let by_url = store
            .find_link("clickup", None, "https://app.clickup.com/t/abc")
            .unwrap()
            .expect("url matches");
        assert_eq!(by_url.id, by_id.id);
        assert!(
            store
                .find_link("github", Some("abc"), "https://example.invalid")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn slack_capture_segments_by_gap_and_thread() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700000000.000100", "U1", "one"),
                90,
            )
            .unwrap()
            .unwrap();
        let second = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700000600.000100", "U1", "two"),
                90,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            first.episode_id, second.episode_id,
            "10 minutes is one episode"
        );
        let gap = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700007800.000100", "U1", "three"),
                90,
            )
            .unwrap()
            .unwrap();
        assert_ne!(
            gap.episode_id, first.episode_id,
            "2 hours starts a new episode"
        );

        // A reply in the first message's thread joins that episode, not the newest one.
        let mut reply = raw_message("1700007900.000100", "U1", "thread reply");
        reply.thread_ts = Some(first.ts.clone());
        let reply = store
            .capture_slack_message("U1", Some("Alice"), &reply, 90)
            .unwrap()
            .unwrap();
        assert_eq!(reply.episode_id, first.episode_id);

        // Replaying the same ts updates the text in place; it does not duplicate.
        let replay = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700000000.000100", "U1", "one (edited)"),
                90,
            )
            .unwrap()
            .unwrap();
        assert_eq!(replay.episode_id, first.episode_id);
        let detail = store.get_slack_episode(&first.episode_id).unwrap();
        assert_eq!(
            detail.messages.len(),
            3,
            "first, second and the thread reply"
        );
        assert_eq!(detail.messages[0].text, "one (edited)");
        assert_eq!(detail.messages[0].direction, "in");
        assert_eq!(detail.episode.channel_id, "D01");
        assert_eq!(
            store.slack_channel_cursor("D01").unwrap().as_deref(),
            Some("1700007900.000100")
        );

        let episodes = store.list_slack_episodes(None, 10).unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].peer_name.as_deref(), Some("Alice"));

        // Our own message is outbound.
        let out = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700008000.000100", "U0", "mine"),
                90,
            )
            .unwrap()
            .unwrap();
        assert_eq!(out.direction, "out");
    }

    #[test]
    fn a_deleted_message_is_marked_not_removed() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700000000.000100", "U1", "please do it"),
                90,
            )
            .unwrap()
            .unwrap();
        let mut deleted = raw_message("1700000000.000100", "U1", "");
        deleted.deleted = true;
        store
            .capture_slack_message("U1", Some("Alice"), &deleted, 90)
            .unwrap()
            .unwrap();
        let detail = store.get_slack_episode(&first.episode_id).unwrap();
        assert_eq!(
            detail.messages.len(),
            1,
            "a deletion is an update, not a new row"
        );
        assert!(detail.messages[0].deleted);
        assert!(detail.messages[0].text.is_empty());
    }

    #[test]
    fn slack_excluded_peer_is_not_stored() {
        let store = Store::open_in_memory().unwrap();
        let peer = store.upsert_slack_peer("U1", Some("Alice")).unwrap();
        store.set_slack_peer_excluded(&peer.id, true).unwrap();
        let stored = store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700000000.000100", "U1", "secret"),
                90,
            )
            .unwrap();
        assert!(stored.is_none());
        assert!(store.list_slack_episodes(None, 10).unwrap().is_empty());
    }

    fn new_job(conversation_id: &str, watermark: &str) -> NewJob {
        NewJob {
            conversation_id: conversation_id.into(),
            watermark: watermark.into(),
            kind: "classify".into(),
            project_id: None,
            route: Some("change".into()),
            jev_json: Some("{}".into()),
            candidates: vec!["T1".into(), "T2".into()],
        }
    }

    /// A conversation row for the job tests, whose FK needs a real one.
    fn conversation(store: &Store) -> String {
        store
            .capture_slack_message(
                "U1",
                Some("Alice"),
                &raw_message("1700000000.000100", "U1", "hi"),
                90,
            )
            .unwrap()
            .unwrap()
            .episode_id
    }

    #[test]
    fn jobs_are_one_per_watermark_and_supersede_the_older_live_job() {
        let store = Store::open_in_memory().unwrap();
        let conversation = conversation(&store);
        let (first, inserted) = store
            .create_job(new_job(&conversation, "1700000000.000100"))
            .unwrap();
        assert!(inserted);
        assert_eq!(first.state, "pending");
        assert_eq!(first.candidates, vec!["T1", "T2"]);

        let (replay, inserted) = store
            .create_job(new_job(&conversation, "1700000000.000100"))
            .unwrap();
        assert!(!inserted, "a replay must not count as a new job");
        assert_eq!(replay.id, first.id, "a replay returns the same job");

        let (second, inserted) = store
            .create_job(new_job(&conversation, "1700000600.000100"))
            .unwrap();
        assert!(inserted);
        let superseded = store.get_job(&first.id).unwrap();
        assert_eq!(superseded.state, "superseded");
        assert_eq!(
            superseded.superseded_by.as_deref(),
            Some(second.id.as_str())
        );

        let running = store
            .set_job_status(&second.id, "running", None, None)
            .unwrap();
        assert_eq!(running.attempts, 1);
        assert!(running.started_at.is_some());

        let done = store
            .set_job_status(&second.id, "done", Some("created one task"), None)
            .unwrap();
        assert_eq!(done.summary.as_deref(), Some("created one task"));
        assert!(done.finished_at.is_some());
        assert!(
            store
                .list_jobs(Some("done"), 10)
                .unwrap()
                .iter()
                .any(|j| j.id == second.id)
        );
    }

    fn recent_conversation(store: &Store, user: &str, seconds_ago: i64) -> String {
        let ts = time::OffsetDateTime::now_utc().unix_timestamp() - seconds_ago;
        let mut message = raw_message(&format!("{ts}.000100"), user, "need a report");
        message.channel_id = format!("D{user}");
        store
            .capture_slack_message(user, Some(user), &message, 90)
            .unwrap()
            .unwrap()
            .episode_id
    }

    fn job_in(store: &Store, conversation: &str, state: &str, route: &str) -> String {
        let mut new = new_job(conversation, "1.000000");
        new.route = Some(route.into());
        let (job, _) = store.create_job(new).unwrap();
        store.set_job_status(&job.id, state, None, None).unwrap();
        store
            .conn()
            .execute(
                "UPDATE analysis_jobs SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                params![job.id],
            )
            .unwrap();
        job.id
    }

    fn some_task(store: &Store) {
        let project = store
            .create_project_with_default_workflow(NewProject::new("Inbox", "inbox", None))
            .unwrap();
        store
            .create_task(NewTask::new(&project.project.id, "fresh"))
            .unwrap();
    }

    #[test]
    fn requeue_selects_shadow_and_stale_ignored_jobs_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let shadow = recent_conversation(&store, "U1", 3_600);
        let ignored = recent_conversation(&store, "U2", 600);
        let acted = recent_conversation(&store, "U3", 300);
        let old = recent_conversation(&store, "U4", 5 * 86_400);
        let none = recent_conversation(&store, "U5", 100);
        job_in(&store, &shadow, "shadow", "review");
        job_in(&store, &ignored, "done", "ignore");
        job_in(&store, &acted, "done", "new_task");
        job_in(&store, &old, "shadow", "ignore");
        let _ = none;
        some_task(&store);

        assert_eq!(
            store.requeue_recent(3, 10).unwrap(),
            vec![ignored.clone(), shadow.clone()]
        );
        assert_eq!(store.requeue_recent(3, 1).unwrap(), vec![ignored]);
    }

    #[test]
    fn a_review_job_resolves_once_and_then_leaves_the_requeue() {
        let store = Store::open_in_memory().unwrap();
        let sent = recent_conversation(&store, "U1", 600);
        let dismissed = recent_conversation(&store, "U2", 500);
        let sent_job = job_in(&store, &sent, "pending", "review");
        let dismissed_job = job_in(&store, &dismissed, "pending", "review");
        some_task(&store);
        assert_eq!(store.requeue_recent(3, 10).unwrap().len(), 2);

        let job = store.resolve_review_job(&sent_job, true).unwrap();
        assert_eq!(
            (job.state.as_str(), job.route.as_deref()),
            ("pending_agent", Some("change"))
        );
        let job = store.resolve_review_job(&dismissed_job, false).unwrap();
        assert_eq!(
            (job.state.as_str(), job.route.as_deref()),
            ("done", Some("dismissed"))
        );
        assert!(store.requeue_recent(3, 10).unwrap().is_empty());
        assert!(
            store.resolve_review_job(&dismissed_job, true).is_err(),
            "a settled job cannot be resolved again"
        );
    }

    #[test]
    fn requeue_skips_ignored_jobs_newer_than_every_task_and_jobs_in_flight() {
        let store = Store::open_in_memory().unwrap();
        let ignored = recent_conversation(&store, "U1", 600);
        let busy = recent_conversation(&store, "U2", 500);
        job_in(&store, &ignored, "done", "ignore");
        job_in(&store, &busy, "shadow", "review");
        let mut second = new_job(&busy, "2.000000");
        second.route = Some("change".into());
        let (running, _) = store.create_job(second).unwrap();
        store
            .set_job_status(&running.id, "running", None, None)
            .unwrap();

        assert!(
            store.requeue_recent(3, 10).unwrap().is_empty(),
            "no task exists to change the outcome, and the other job is running"
        );
        some_task(&store);
        assert_eq!(store.requeue_recent(3, 10).unwrap(), vec![ignored]);
    }

    #[test]
    fn a_replaced_job_leaves_the_requeue_and_a_second_run_selects_nothing() {
        let store = Store::open_in_memory().unwrap();
        let conversation = recent_conversation(&store, "U1", 600);
        let shadow = job_in(&store, &conversation, "shadow", "ignore");
        some_task(&store);
        assert_eq!(
            store.requeue_recent(3, 10).unwrap(),
            vec![conversation.clone()]
        );

        let mut live = new_job(&conversation, "1.000000");
        live.route = Some("ignore".into());
        let (replay, inserted) = store.create_job(live.clone()).unwrap();
        assert!(
            !inserted && replay.id == shadow,
            "a plain replay keeps the shadow job"
        );
        let (job, inserted) = store.replace_job(live).unwrap();
        assert!(inserted);
        assert_eq!(job.id, shadow, "the row is rewritten in place");
        assert_eq!(job.state, "pending");
        store.set_job_status(&job.id, "done", None, None).unwrap();

        assert!(
            store.requeue_recent(3, 10).unwrap().is_empty(),
            "re-analysed ignore result is newer than every task"
        );
    }

    #[test]
    fn replace_job_leaves_acted_jobs_alone() {
        let store = Store::open_in_memory().unwrap();
        let conversation = recent_conversation(&store, "U1", 600);
        let acted = job_in(&store, &conversation, "done", "new_task");
        let mut again = new_job(&conversation, "1.000000");
        again.route = Some("ignore".into());
        let (job, inserted) = store.replace_job(again).unwrap();
        assert!(!inserted);
        assert_eq!(job.id, acted);
        assert_eq!(job.route.as_deref(), Some("new_task"));
    }

    #[test]
    fn find_links_matching_ignores_case_slashes_and_archived_tasks() {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project_with_default_workflow(NewProject::new("Inbox", "inbox", None))
            .unwrap();
        let task = store
            .create_task(NewTask::new(&project.project.id, "linked"))
            .unwrap();
        store
            .add_link(NewLink {
                task_id: task.id.clone(),
                claim_id: None,
                kind: "clickup".into(),
                external_id: Some("AbC123".into()),
                url: "https://app.clickup.com/t/AbC123/".into(),
                title: None,
                state: None,
                metadata_json: None,
                last_synced_at: None,
            })
            .unwrap();
        let by_id = store.find_links_matching(&["abc123".into()], &[]).unwrap();
        let by_url = store
            .find_links_matching(&[], &["https://app.clickup.com/t/abc123".into()])
            .unwrap();
        assert_eq!((by_id.len(), by_url.len()), (1, 1));
        assert!(store.find_links_matching(&[], &[]).unwrap().is_empty());
        assert!(
            store
                .find_links_matching(&["nope".into()], &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn job_ops_replay_the_first_result() {
        let store = Store::open_in_memory().unwrap();
        let conversation = conversation(&store);
        let (job, inserted) = store
            .create_job(new_job(&conversation, "1700000000.000100"))
            .unwrap();
        assert!(inserted);
        let (first, fresh) = store
            .record_job_op(&job.id, 0, "tasks_create", r#"{"id":"T1"}"#)
            .unwrap();
        assert!(fresh);
        let (replay, fresh) = store
            .record_job_op(&job.id, 0, "tasks_create", r#"{"id":"T2"}"#)
            .unwrap();
        assert!(!fresh);
        assert_eq!(replay.id, first.id);
        assert_eq!(replay.tool, "tasks_create");
        assert_eq!(replay.result_json, r#"{"id":"T1"}"#);
        assert_eq!(store.job_op(&job.id, 0).unwrap().unwrap().id, first.id);
    }
}
