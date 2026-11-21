//! The v2 SQLite store. One file (`relay-v2.db`), one writer behind a mutex, its own
//! `application_id` and `user_version`, and no shared table with the v1 store.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use super::import::{ImportCandidate, ImportOutcome, ImportRefreshTarget, RefreshUpdate};
use super::model::{
    AgentMode, GrillTurn, ImportRecord, NewImportRecord, NewV2Project, NoteWrite, PreparedLine,
    PreparedNoteWrite, PreparedReference, T3Target, TaskProgressUpdate, TaskStatus, V2Note,
    V2NoteLine, V2NotebookSnapshot, V2Project, V2Reference, V2Settings, V2SettingsUpdate, V2Task,
    V2WorkspaceSnapshot, WorkspaceSearchResult,
};
use super::recovery::{
    NewRecovery, RecoveryReason, RecoveryTarget, RecoveryUpdate, STALE_SNAPSHOT_MS,
};
use super::reference::{
    ReferenceAvailability, ReferenceKey, ReferenceSnapshot, V2PickerGroup, V2PickerRow,
    snapshot_needs_recovery,
};
use crate::model::{from_unix_millis, new_id, now};

/// One v2 migration and the SQL predicate that is true once its change is on disk.
/// `applied` is a marker, not a version number: #27 and #28 both shipped a `0002`
/// that reached `user_version = 2` with a different schema. Detecting each change by
/// its own marker lets either on-disk layout upgrade to the single coherent sequence
/// without discarding data.
struct Migration {
    sql: &'static str,
    applied: &'static str,
}

/// v2 migrations run in order. `user_version` is the count of applied files, so an
/// existing fresh-start database can pick up later additions without a v1 migration.
const MIGRATIONS: &[Migration] = &[
    Migration {
        sql: include_str!("../../migrations/v2/0001_init.sql"),
        applied: "EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'projects')",
    },
    Migration {
        sql: include_str!("../../migrations/v2/0002_task_progress.sql"),
        applied: "EXISTS (SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'grill_turn')",
    },
    Migration {
        sql: include_str!("../../migrations/v2/0003_provider_settings.sql"),
        applied: "EXISTS (SELECT 1 FROM pragma_table_info('settings') WHERE name = 'opencode_model')",
    },
    Migration {
        sql: include_str!("../../migrations/v2/0004_reference_recovery.sql"),
        applied: "EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ref_recovery')",
    },
    Migration {
        sql: include_str!("../../migrations/v2/0005_import_refresh.sql"),
        applied: "EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'import_refresh')",
    },
];
/// 'RELY' as an integer. A non-zero application id marks a file as ours.
pub(super) const APPLICATION_ID: i64 = 0x5245_4C59;
pub(super) const USER_VERSION: i64 = MIGRATIONS.len() as i64;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// Column order shared by the two queries that feed `task_from_row`.
const TASK_COLUMNS: &str = "t.id, t.source_line_id, n.id, t.original_line, t.agent_mode, t.status, \
     t.project_id, t.brief, t.grill_turn, t.pending_action, t.failure_stage, t.failure_reason, \
     t.thread_url, t.created_at, t.updated_at";

#[derive(Debug, thiserror::Error)]
pub enum V2Error {
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
    #[error("note {id} is at revision {actual}, expected {expected}")]
    StaleRevision {
        id: String,
        expected: i64,
        actual: i64,
    },
    #[error("line {0} belongs to another note")]
    ForeignLine(String),
    #[error("task {0} already has a project")]
    TaskAlreadyAssigned(String),
    #[error("{entity} already exists: {value}")]
    Duplicate { entity: &'static str, value: String },
    #[error("not a Relay v2 database: {0}")]
    UnexpectedDatabase(String),
}

impl V2Error {
    /// Stable code for IPC; the message is for humans.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io { .. } => "io",
            Self::Sql(_) => "sql",
            Self::NotFound(_) => "not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::StaleRevision { .. } => "stale_rev",
            Self::ForeignLine(_) => "foreign_line",
            Self::TaskAlreadyAssigned(_) => "task_already_assigned",
            Self::Duplicate { .. } => "duplicate",
            Self::UnexpectedDatabase(_) => "unexpected_database",
        }
    }
}

/// A connection to the v2 database. The mutex is the write lock; all writes use
/// `BEGIN IMMEDIATE` so a failed write rolls the whole change back.
pub struct V2Store {
    conn: Mutex<Connection>,
}

impl V2Store {
    /// Opens `relay-v2.db` beside the existing v1 file. The file is inspected before
    /// its journal mode is touched, so a v1 or foreign database is never modified.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, V2Error> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|source| V2Error::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path)?;
        inspect_database(&conn)?;
        Self::from_connection(conn)
    }

    /// Private in-memory store for core tests.
    pub fn open_in_memory() -> Result<Self, V2Error> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, V2Error> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(super) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("relay v2 store mutex poisoned")
    }

    // -----------------------------------------------------------------------
    // Settings and projects
    // -----------------------------------------------------------------------

    pub fn settings(&self) -> Result<V2Settings, V2Error> {
        settings_from_row(&self.conn())
    }

    pub fn update_settings(&self, update: V2SettingsUpdate) -> Result<V2Settings, V2Error> {
        let conn = self.conn();
        let at = now();
        conn.execute(
            "UPDATE settings SET import_clickup_enabled = ?1, import_github_enabled = ?2,
                 import_scope = ?3, updated_at = ?4 WHERE id = 1",
            params![
                update.import_clickup_enabled as i64,
                update.import_github_enabled as i64,
                none_if_blank(update.import_scope),
                at,
            ],
        )?;
        settings_from_row(&conn)
    }

    pub fn list_projects(&self) -> Result<Vec<V2Project>, V2Error> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, slug, repo_path, owner_repo, t3_target, created_at, updated_at
             FROM projects ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([], project_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Registers a project with a normalized, unique slug and optional owner/repo.
    pub fn register_project(&self, new: NewV2Project) -> Result<V2Project, V2Error> {
        let name = new.name.trim();
        if name.is_empty() {
            return Err(V2Error::InvalidInput("project name is required".into()));
        }
        let slug = new.slug.trim().to_lowercase();
        if slug.is_empty()
            || !slug
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
        {
            return Err(V2Error::InvalidInput(format!(
                "project slug must be lowercase letters, digits, '-' or '_': {slug}"
            )));
        }
        let owner_repo = none_if_blank(new.owner_repo).map(|value| value.to_lowercase());
        if let Some(owner_repo) = &owner_repo
            && owner_repo.split_once('/').is_none()
        {
            return Err(V2Error::InvalidInput(format!(
                "owner/repo must look like `owner/repo`, got `{owner_repo}`"
            )));
        }

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
            return Err(V2Error::Duplicate {
                entity: "project",
                value: slug,
            });
        }
        if let Some(owner_repo) = &owner_repo {
            let exists = tx
                .query_row(
                    "SELECT 1 FROM projects WHERE owner_repo = ?1",
                    params![owner_repo],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                return Err(V2Error::Duplicate {
                    entity: "owner/repo",
                    value: owner_repo.clone(),
                });
            }
        }

        let at = now();
        let project = V2Project {
            id: new_id(),
            name: name.to_string(),
            slug,
            repo_path: none_if_blank(new.repo_path),
            owner_repo,
            t3_target: none_if_blank(new.t3_target),
            created_at: at.clone(),
            updated_at: at,
        };
        tx.execute(
            "INSERT INTO projects
                 (id, name, slug, repo_path, owner_repo, t3_target, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project.id,
                project.name,
                project.slug,
                project.repo_path,
                project.owner_repo,
                project.t3_target,
                project.created_at,
                project.updated_at,
            ],
        )?;
        tx.commit()?;
        Ok(project)
    }

    /// Saves the non-secret OpenCode `provider/model` selection. Passing `None`
    /// clears it. The key itself lives only in the v2 keyring.
    pub fn set_opencode_model(&self, model: Option<String>) -> Result<V2Settings, V2Error> {
        let conn = self.conn();
        let model = model.and_then(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        });
        conn.execute(
            "UPDATE settings SET opencode_model = ?1, updated_at = ?2 WHERE id = 1",
            params![model, now()],
        )?;
        settings_from_row(&conn)
    }

    /// Saves or clears a project's T3 mapping. The mapping is independent of the
    /// checkout path and is valid while T3 is stopped, so no CLI call happens here.
    pub fn set_t3_target(
        &self,
        project_id: &str,
        target: Option<T3Target>,
    ) -> Result<V2Project, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                params![project_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(V2Error::NotFound(format!("project {project_id}")));
        }
        let stored = target.map(|target| target.to_storage());
        tx.execute(
            "UPDATE projects SET t3_target = ?1, updated_at = ?2 WHERE id = ?3",
            params![stored, now(), project_id],
        )?;
        let project = tx.query_row(
            "SELECT id, name, slug, repo_path, owner_repo, t3_target, created_at, updated_at
             FROM projects WHERE id = ?1",
            params![project_id],
            project_from_row,
        )?;
        tx.commit()?;
        Ok(project)
    }

    /// The typed mapping read #36 consumes. `None` means no mapping was saved.
    pub fn t3_target(&self, project_id: &str) -> Result<Option<T3Target>, V2Error> {
        let found: Option<(String, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT id, t3_target FROM projects WHERE id = ?1",
                params![project_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((_, raw)) = found else {
            return Err(V2Error::NotFound(format!("project {project_id}")));
        };
        match raw {
            Some(raw) => T3Target::from_storage(&raw).map(Some).ok_or_else(|| {
                V2Error::InvalidInput(format!("project {project_id} has an invalid T3 target"))
            }),
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // Notes, lines, Tasks and References
    // -----------------------------------------------------------------------

    /// The write contract #25 consumes. Create is idempotent on its submission key,
    /// Edit is guarded by the revision and by line ownership. Any failure rolls back.
    pub fn save_note(&self, write: PreparedNoteWrite) -> Result<V2Note, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let at = now();

        let note_id = match &write.write {
            NoteWrite::Create { submission_key } => {
                if let Some(existing) = tx
                    .query_row(
                        "SELECT note_id FROM submission_keys WHERE submission_key = ?1",
                        params![submission_key],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                {
                    let replay = note_by_id(&tx, &existing)?;
                    tx.commit()?;
                    return Ok(replay);
                }
                let id = new_id();
                tx.execute(
                    "INSERT INTO notes (id, rev, source, body, created_at, updated_at)
                     VALUES (?1, 0, ?2, '', ?3, ?3)",
                    params![id, write.source, at],
                )?;
                tx.execute(
                    "INSERT INTO submission_keys (submission_key, note_id, created_at)
                     VALUES (?1, ?2, ?3)",
                    params![submission_key, id, at],
                )?;
                id
            }
            NoteWrite::Edit {
                note_id,
                expected_revision,
            } => {
                let actual: Option<i64> = tx
                    .query_row(
                        "SELECT rev FROM notes WHERE id = ?1",
                        params![note_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let actual = actual.ok_or_else(|| V2Error::NotFound(format!("note {note_id}")))?;
                if actual != *expected_revision {
                    return Err(V2Error::StaleRevision {
                        id: note_id.clone(),
                        expected: *expected_revision,
                        actual,
                    });
                }
                note_id.clone()
            }
        };

        validate_lines(&tx, &note_id, &write.lines)?;

        // Clear every active position first, so an insertion or reorder cannot collide
        // with the (note_id, position) uniqueness constraint. Removed lines stay with
        // a null position and keep their Tasks addressable.
        tx.execute(
            "UPDATE note_lines SET position = NULL, updated_at = ?1 WHERE note_id = ?2",
            params![at, note_id],
        )?;

        for (position, line) in write.lines.iter().enumerate() {
            let line_id = upsert_line(&tx, &note_id, line, position as i64, &at)?;
            replace_line_refs(&tx, &line_id, &line.references, &at)?;

            if let Some(intent) = &line.task_intent {
                ensure_project(&tx, intent.project_id.as_deref())?;
                let has_task = tx
                    .query_row(
                        "SELECT 1 FROM tasks WHERE source_line_id = ?1",
                        params![line_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !has_task {
                    let status = match intent.agent_mode {
                        AgentMode::Act | AgentMode::Wait => TaskStatus::Briefing,
                        AgentMode::Grill => TaskStatus::Grilling,
                    };
                    let task_id = new_id();
                    let grill_turn =
                        (status == TaskStatus::Grilling).then(|| GrillTurn::Agent.as_str());
                    tx.execute(
                        "INSERT INTO tasks
                             (id, source_line_id, original_line, agent_mode, status, project_id,
                              brief, grill_turn, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?8)",
                        params![
                            task_id,
                            line_id,
                            line.text,
                            intent.agent_mode.as_str(),
                            status.as_str(),
                            intent.project_id,
                            grill_turn,
                            at,
                        ],
                    )?;
                    // Snapshot the line's membership now; later Note edits cannot remove it.
                    tx.execute(
                        "INSERT INTO task_refs (task_id, ref_id, position)
                         SELECT ?1, ref_id, position FROM line_refs WHERE line_id = ?2",
                        params![task_id, line_id],
                    )?;
                }
            }
        }

        // The Note body is the plain-line projection: directive lines create Tasks and keep
        // their own frozen `original_line`, so they do not become Note prose. Every raw line
        // (including its sigil and URL text) stays in `note_lines` for in-place editing.
        let body = write
            .lines
            .iter()
            .filter(|line| line.task_intent.is_none())
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        tx.execute(
            "UPDATE notes SET rev = rev + 1, body = ?1, updated_at = ?2 WHERE id = ?3",
            params![body, at, note_id],
        )?;

        let saved = note_by_id(&tx, &note_id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Deletes a Note and its lines/Tasks. The Import ledger has no foreign key to a
    /// Note, so its source keys survive the deletion.
    pub fn delete_note(&self, id: &str) -> Result<(), V2Error> {
        let changed = self
            .conn()
            .execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(V2Error::NotFound(format!("note {id}")));
        }
        Ok(())
    }

    /// Assigns a registered project to an unassigned Task. Repeating the same choice
    /// is a no-op; a different project is a conflict.
    pub fn assign_task_project(&self, task_id: &str, project_id: &str) -> Result<V2Task, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = task_by_id(&tx, task_id)?;
        if let Some(existing) = &task.project_id {
            if existing == project_id {
                tx.commit()?;
                return Ok(task);
            }
            return Err(V2Error::TaskAlreadyAssigned(task_id.to_string()));
        }
        ensure_project(&tx, Some(project_id))?;
        tx.execute(
            "UPDATE tasks SET project_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![project_id, now(), task_id],
        )?;
        let updated = task_by_id(&tx, task_id)?;
        tx.commit()?;
        Ok(updated)
    }

    /// Writes the progress fields the v2 views read. #35-#37 own the producers; this is
    /// also the fixture path used to exercise a stored Grilling turn through SQLite. Every
    /// call bumps `updated_at`, so the Task becomes the newest change in its group.
    pub fn update_task_progress(
        &self,
        task_id: &str,
        update: TaskProgressUpdate,
    ) -> Result<V2Task, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        task_by_id(&tx, task_id)?;
        tx.execute(
            "UPDATE tasks SET grill_turn = ?2, pending_action = ?3, failure_stage = ?4,
                 failure_reason = ?5, thread_url = ?6, updated_at = ?7
             WHERE id = ?1",
            params![
                task_id,
                update.grill_turn.map(GrillTurn::as_str),
                update.pending_action,
                update.failure_stage,
                update.failure_reason,
                update.thread_url,
                now(),
            ],
        )?;
        let updated = task_by_id(&tx, task_id)?;
        tx.commit()?;
        Ok(updated)
    }

    // -----------------------------------------------------------------------
    // Import ledger
    // -----------------------------------------------------------------------

    /// Reserves a source key once, independently of the Note's lifetime.
    pub fn record_import(&self, new: NewImportRecord) -> Result<(), V2Error> {
        self.conn().execute(
            "INSERT INTO import_ledger (source_kind, target_id, note_id, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(source_kind, target_id) DO UPDATE SET
                 note_id = COALESCE(import_ledger.note_id, excluded.note_id)",
            params![new.source_kind, new.target_id, new.note_id, now()],
        )?;
        Ok(())
    }

    pub fn import_recorded(&self, source_kind: &str, target_id: &str) -> Result<bool, V2Error> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM import_ledger WHERE source_kind = ?1 AND target_id = ?2",
                params![source_kind, target_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn list_imports(&self) -> Result<Vec<ImportRecord>, V2Error> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT source_kind, target_id, note_id, created_at FROM import_ledger
             ORDER BY created_at, source_kind, target_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ImportRecord {
                source_kind: row.get(0)?,
                target_id: row.get(1)?,
                note_id: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Creates one imported Note for a canonical target. The whole change is one transaction:
    /// the ledger key is reserved first with conflict handling scoped to that key, so a racing
    /// run can commit at most one Note. Any later failure rolls the reservation back. The Note
    /// text is the provider's literal title, bypassing Composer directive parsing entirely, and
    /// carries exactly one source Reference and one seeded snapshot.
    pub fn import_note(&self, candidate: &ImportCandidate) -> Result<ImportOutcome, V2Error> {
        let (source_kind, target_id) = candidate.ledger_key();
        if !matches!(source_kind.as_str(), "clickup" | "github") {
            return Err(V2Error::InvalidInput("unsupported Import source".into()));
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let at = now();

        let changed = tx.execute(
            "INSERT INTO import_ledger (source_kind, target_id, note_id, created_at)
             VALUES (?1, ?2, NULL, ?3)
             ON CONFLICT(source_kind, target_id) DO NOTHING",
            params![source_kind, target_id, at],
        )?;
        if changed == 0 {
            tx.commit()?;
            return Ok(ImportOutcome::AlreadyImported);
        }

        // Past the reservation point. Any error from here unwinds the whole transaction,
        // including the ledger row, so a retry can create the Note.
        let title = candidate.title.trim();
        if title.is_empty() {
            return Err(V2Error::InvalidInput(
                "an imported Note needs a title".into(),
            ));
        }
        let canonical_url = candidate.canonical_url.trim();
        if canonical_url.is_empty() {
            return Err(V2Error::InvalidInput(
                "an imported Reference needs a canonical URL".into(),
            ));
        }
        let identity = candidate.identity();
        let (kind, stored_target) = storage_identity(&identity)
            .ok_or_else(|| V2Error::InvalidInput("unsupported Import Reference".into()))?;

        let note_id = new_id();
        tx.execute(
            "INSERT INTO notes (id, rev, source, body, created_at, updated_at)
             VALUES (?1, 0, 'import', ?2, ?3, ?3)",
            params![note_id, title, at],
        )?;
        let line_id = new_id();
        tx.execute(
            "INSERT INTO note_lines (id, note_id, text, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)",
            params![line_id, note_id, title, at],
        )?;
        tx.execute(
            "INSERT INTO ref_targets (id, kind, target_id, canonical_url, snapshot_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(kind, target_id) DO UPDATE SET
                 canonical_url = excluded.canonical_url,
                 snapshot_json = COALESCE(ref_targets.snapshot_json, excluded.snapshot_json)",
            params![
                new_id(),
                kind,
                stored_target,
                canonical_url,
                candidate.snapshot_json,
                at,
            ],
        )?;
        let ref_id: String = tx.query_row(
            "SELECT id FROM ref_targets WHERE kind = ?1 AND target_id = ?2",
            params![kind, stored_target],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO line_refs (line_id, ref_id, position) VALUES (?1, ?2, 0)",
            params![line_id, ref_id],
        )?;

        // Enrol the live link for exact reads at the provider cadence. This is independent of
        // discovery and its toggles and survives deletion of the Note it came with.
        let due_ms = rfc3339_ms(&at).saturating_add(candidate.refresh_ms());
        let due_at = from_unix_millis(due_ms).unwrap_or_else(crate::model::now);
        tx.execute(
            "INSERT OR IGNORE INTO import_refresh
                 (key, kind, target_id, canonical_url, provider, account_id, attempts,
                  next_due_at, next_due_ms, reason, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, NULL, ?9)",
            params![
                candidate.identity(),
                kind,
                stored_target,
                canonical_url,
                source_kind,
                candidate.account_id,
                due_at,
                due_ms,
                at,
            ],
        )?;
        tx.execute(
            "UPDATE import_ledger SET note_id = ?1 WHERE source_kind = ?2 AND target_id = ?3",
            params![note_id, source_kind, target_id],
        )?;
        tx.commit()?;
        Ok(ImportOutcome::Created { note_id })
    }

    // -----------------------------------------------------------------------
    // Imported Reference refresh
    // -----------------------------------------------------------------------

    /// The read-only snapshot currently stored for a canonical Reference identity. The refresh
    /// worker uses it to keep title and metadata while updating only live state.
    pub fn reference_snapshot_json(
        &self,
        kind: &str,
        target_id: &str,
    ) -> Result<Option<String>, V2Error> {
        Ok(self
            .conn()
            .query_row(
                "SELECT snapshot_json FROM ref_targets WHERE kind = ?1 AND target_id = ?2",
                params![kind, target_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Due imported links. The connection is released with this call, so the worker can await a
    /// provider without holding the SQLite write lock.
    pub fn due_import_refreshes(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<ImportRefreshTarget>, V2Error> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, kind, target_id, canonical_url, provider, account_id, attempts,
                    next_due_at, next_due_ms, reason, updated_at
             FROM import_refresh WHERE next_due_ms <= ?1
             ORDER BY next_due_ms, key LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now_ms, limit as i64], |row| {
            Ok(ImportRefreshTarget {
                key: row.get(0)?,
                kind: row.get(1)?,
                target_id: row.get(2)?,
                canonical_url: row.get(3)?,
                provider: row.get(4)?,
                account_id: row.get(5)?,
                attempts: row.get(6)?,
                next_due_at: row.get(7)?,
                next_due_ms: row.get(8)?,
                reason: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Applies a worker decision. `expected_updated_at` is what the worker read; a newer
    /// completion discards this one. A successful read only touches the snapshot and the
    /// refresh schedule, never a Note, a Task or the Import ledger.
    pub fn record_import_refresh(
        &self,
        key: &str,
        expected_updated_at: &str,
        snapshot_json: Option<&str>,
        update: &RefreshUpdate,
        now: &str,
    ) -> Result<bool, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT updated_at FROM import_refresh WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        if current.as_deref() != Some(expected_updated_at) {
            return Ok(false);
        }
        if let Some(json) = snapshot_json
            && let Some((kind, target_id)) = storage_identity(key)
        {
            tx.execute(
                "UPDATE ref_targets SET snapshot_json = ?1 WHERE kind = ?2 AND target_id = ?3",
                params![json, kind, target_id],
            )?;
        }
        tx.execute(
            "UPDATE import_refresh SET attempts = ?2, next_due_at = ?3, next_due_ms = ?4,
                 reason = ?5, updated_at = ?6 WHERE key = ?1",
            params![
                key,
                update.attempts,
                update.next_due_at,
                update.next_due_ms,
                update.reason,
                now,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Reference snapshots and persistent recovery
    // -----------------------------------------------------------------------

    /// Persists a picked live result as a read-only snapshot under its canonical identity.
    /// This creates no Note, Task or Import ledger row; it only makes the target pickable from
    /// local results later. A later Note save preserves this snapshot through `COALESCE`.
    pub fn save_reference_snapshot(
        &self,
        key: &ReferenceKey,
        snapshot: &ReferenceSnapshot,
    ) -> Result<V2Reference, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let at = now();
        let kind = key.kind();
        let target_id = key.target_id();
        tx.execute(
            "INSERT INTO ref_targets (id, kind, target_id, canonical_url, snapshot_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(kind, target_id) DO UPDATE SET
                 canonical_url = excluded.canonical_url,
                 snapshot_json = excluded.snapshot_json",
            params![
                new_id(),
                kind,
                target_id,
                key.canonical_url(),
                snapshot.to_json(),
                at,
            ],
        )?;
        let stored = tx.query_row(
            "SELECT id, kind, target_id, canonical_url, snapshot_json
             FROM ref_targets WHERE kind = ?1 AND target_id = ?2",
            params![kind, target_id],
            reference_from_row,
        )?;
        tx.commit()?;
        Ok(stored)
    }

    /// Creates the single recovery row for a shared key. Repeating it across Notes, Tasks or
    /// windows is a no-op, so a key never gets more than one schedule.
    pub fn ensure_recovery(
        &self,
        new: &[NewRecovery],
        now: &str,
        now_ms: i64,
    ) -> Result<usize, V2Error> {
        if new.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut created = 0;
        for target in new {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO ref_recovery
                     (key, kind, target_id, canonical_url, provider, attempts,
                      next_due_at, next_due_ms, reason, paused, paused_config, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, NULL, 0, NULL, ?6)",
                params![
                    target.key,
                    target.kind,
                    target.target_id,
                    target.canonical_url,
                    target.provider,
                    now,
                    now_ms,
                ],
            )?;
            created += changed;
        }
        tx.commit()?;
        Ok(created)
    }

    /// Due, unpaused targets. The connection is dropped with this call, so the worker can await
    /// a provider without holding the SQLite write lock.
    pub fn due_recoveries(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<RecoveryTarget>, V2Error> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, kind, target_id, canonical_url, provider, attempts,
                    next_due_at, next_due_ms, reason, paused, paused_config, updated_at
             FROM ref_recovery
             WHERE paused = 0 AND next_due_ms <= ?1
             ORDER BY next_due_ms, key LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now_ms, limit as i64], recovery_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Paused targets are re-evaluated on startup and on a provider-status change. A change of
    /// `config_version` (a secret-free credential fingerprint) unpauses them; an unchanged
    /// configuration leaves them paused and issues no request.
    pub fn unpause_changed_config(
        &self,
        provider: &str,
        config_version: &str,
        now: &str,
        now_ms: i64,
    ) -> Result<usize, V2Error> {
        Ok(self.conn().execute(
            "UPDATE ref_recovery SET paused = 0, paused_config = NULL, reason = NULL,
                 attempts = 0, next_due_at = ?1, next_due_ms = ?2, updated_at = ?1
             WHERE provider = ?3 AND paused = 1
               AND (paused_config IS NULL OR paused_config <> ?4)",
            params![now, now_ms, provider, config_version],
        )?)
    }

    /// Applies a worker decision. `expected_updated_at` is the value the worker read; if a
    /// newer attempt changed the row while this one was on the network, the stale completion is
    /// discarded and `false` is returned. A successful read only touches the snapshot and the
    /// recovery row, never Notes or the Import ledger.
    pub fn record_recovery(
        &self,
        key: &str,
        expected_updated_at: &str,
        update: &RecoveryUpdate,
        snapshot_json: Option<&str>,
        now: &str,
    ) -> Result<bool, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT updated_at FROM ref_recovery WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        if current.as_deref() != Some(expected_updated_at) {
            // A newer completion or a config change already superseded this one.
            return Ok(false);
        }
        if update.clear {
            if let (Some((kind, target_id)), Some(json)) = (storage_identity(key), snapshot_json) {
                tx.execute(
                    "UPDATE ref_targets SET snapshot_json = ?1 WHERE kind = ?2 AND target_id = ?3",
                    params![json, kind, target_id],
                )?;
            }
            tx.execute("DELETE FROM ref_recovery WHERE key = ?1", params![key])?;
        } else {
            tx.execute(
                "UPDATE ref_recovery SET attempts = ?2, next_due_at = ?3, next_due_ms = ?4,
                     reason = ?5, paused = ?6, paused_config = ?7, updated_at = ?8
                 WHERE key = ?1",
                params![
                    key,
                    update.attempts,
                    update.next_due_at,
                    update.next_due_ms,
                    update.reason.map(RecoveryReason::as_str),
                    update.paused as i64,
                    update.paused_config,
                    now,
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Read boundary and search
    // -----------------------------------------------------------------------

    /// The single read the first v2 IPC command exposes.
    pub fn notebook_snapshot(&self) -> Result<V2NotebookSnapshot, V2Error> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let note_ids = {
            let mut stmt = tx.prepare("SELECT id FROM notes ORDER BY created_at, seq")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut notes = Vec::with_capacity(note_ids.len());
        for id in note_ids {
            notes.push(note_by_id(&tx, &id)?);
        }
        let projects = {
            let mut stmt = tx.prepare(
                "SELECT id, name, slug, repo_path, owner_repo, t3_target, created_at, updated_at
                 FROM projects ORDER BY name COLLATE NOCASE, id",
            )?;
            stmt.query_map([], project_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let settings = settings_from_row(&tx)?;
        tx.commit()?;
        Ok(V2NotebookSnapshot {
            notes,
            projects,
            settings,
        })
    }

    /// The single workspace read shared by Notebook and Tasks. It reuses the snapshot's
    /// transaction so notes, their Tasks, projects and settings are one consistent view.
    pub fn workspace_snapshot(&self) -> Result<V2WorkspaceSnapshot, V2Error> {
        let base = self.notebook_snapshot()?;
        let mut tasks = base
            .notes
            .iter()
            .flat_map(|note| note.tasks.iter().cloned())
            .collect::<Vec<_>>();
        // Creation time then ULID: stable enough for a flat list and the Needs-you badge.
        tasks.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(V2WorkspaceSnapshot {
            notes: base.notes,
            tasks,
            projects: base.projects,
            settings: base.settings,
        })
    }

    /// The shared filter path for both v2 views. Standalone `+project` and `@reference`
    /// tokens resolve to persisted identities; the remaining text is a literal,
    /// case-insensitive SQLite FTS query reusing the v1 quoting pattern. An unknown
    /// qualified token deliberately returns zero matches instead of being treated as text.
    /// No list cap is applied, so a match past the v1 200-note default is never dropped.
    pub fn search_workspace(&self, raw: &str) -> Result<WorkspaceSearchResult, V2Error> {
        let result = WorkspaceSearchResult {
            query: raw.to_string(),
            note_ids: Vec::new(),
            task_ids: Vec::new(),
        };
        let mut project_slugs = Vec::new();
        let mut reference_tokens = Vec::new();
        let mut text = Vec::new();
        for token in raw.split_whitespace() {
            if let Some(slug) = token.strip_prefix('+') {
                project_slugs.push(slug.trim().to_ascii_lowercase());
            } else if let Some(reference) = token.strip_prefix('@') {
                reference_tokens.push(reference.trim().to_ascii_lowercase());
            } else {
                text.push(token);
            }
        }

        let conn = self.conn();
        let mut project_ids = Vec::new();
        for slug in &project_slugs {
            let id: Option<String> = conn
                .query_row(
                    "SELECT id FROM projects WHERE lower(slug) = ?1",
                    params![slug],
                    |row| row.get(0),
                )
                .optional()?;
            match id {
                Some(id) => project_ids.push(id),
                None => return Ok(result),
            }
        }
        let mut reference_ids = Vec::new();
        for token in &reference_tokens {
            // A known but unreadable target still has a row; its snapshot may be null.
            let target: Option<String> = conn
                .query_row(
                    "SELECT target_id FROM ref_targets
                     WHERE lower(target_id) = ?1 OR lower(canonical_url) = ?1 LIMIT 1",
                    params![token],
                    |row| row.get(0),
                )
                .optional()?;
            match target {
                Some(target) => reference_ids.push(target),
                None => return Ok(result),
            }
        }

        let fts = if text.is_empty() {
            None
        } else {
            Some(fts_literal(&text.join(" ")))
        };
        let note_hits = match &fts {
            Some(query) => {
                let mut stmt = conn.prepare(
                    "SELECT n.id FROM notes_fts JOIN notes n ON n.seq = notes_fts.rowid
                     WHERE notes_fts MATCH ?1",
                )?;
                Some(
                    stmt.query_map(params![query], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<HashSet<_>>>()?,
                )
            }
            None => None,
        };
        let task_hits = match &fts {
            Some(query) => {
                let mut stmt = conn.prepare(
                    "SELECT t.id FROM tasks_fts JOIN tasks t ON t.seq = tasks_fts.rowid
                     WHERE tasks_fts MATCH ?1",
                )?;
                Some(
                    stmt.query_map(params![query], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<HashSet<_>>>()?,
                )
            }
            None => None,
        };

        let ids = {
            let mut stmt = conn.prepare("SELECT id FROM notes ORDER BY created_at, seq")?;
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut matched_notes = Vec::new();
        let mut matched_tasks = Vec::new();
        for id in ids {
            let note = note_by_id(&conn, &id)?;
            let note_refs = reference_ids.is_empty()
                || note
                    .lines
                    .iter()
                    .flat_map(|line| line.references.iter())
                    .chain(note.tasks.iter().flat_map(|task| task.references.iter()))
                    .any(|reference| reference_ids.contains(&reference.target_id));
            let note_project =
                project_ids.is_empty() || note_project_hit(&note, &project_ids, &project_slugs);
            let note_text = note_hits
                .as_ref()
                .is_none_or(|hits| hits.contains(&note.id));
            let mut note_matches = note_text && note_refs && note_project;
            for task in &note.tasks {
                let task_refs = reference_ids.is_empty()
                    || task
                        .references
                        .iter()
                        .any(|reference| reference_ids.contains(&reference.target_id));
                let task_project = project_ids.is_empty()
                    || task
                        .project_id
                        .as_ref()
                        .is_some_and(|id| project_ids.contains(id));
                let task_text = task_hits
                    .as_ref()
                    .is_none_or(|hits| hits.contains(&task.id));
                if task_text && task_refs && task_project {
                    matched_tasks.push(task.id.clone());
                    note_matches = true;
                }
            }
            if note_matches {
                matched_notes.push(note.id);
            }
        }
        Ok(WorkspaceSearchResult {
            query: raw.to_string(),
            note_ids: matched_notes,
            task_ids: matched_tasks,
        })
    }

    /// The one local query behind the `@` picker. It reads only the v2 store: picked
    /// snapshots, references already used on Notes and Tasks, imported targets, the Notes
    /// and Tasks themselves and registered project metadata. Provider-backed rows arrive
    /// in later slices; this query stays local and immediate.
    pub fn reference_picker(&self, raw_query: &str) -> Result<Vec<V2PickerRow>, V2Error> {
        let (needle, scope) = picker_scope(raw_query);
        let conn = self.conn();
        let projects = {
            let mut stmt = conn.prepare(
                "SELECT slug, owner_repo FROM projects ORDER BY updated_at DESC, seq DESC",
            )?;
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut repo_counts: HashMap<String, usize> = HashMap::new();
        for (_, owner_repo) in &projects {
            if let Some(owner_repo) = owner_repo {
                let lower = owner_repo.to_ascii_lowercase();
                if let Some((_, repo)) = lower.split_once('/') {
                    *repo_counts.entry(repo.to_string()).or_default() += 1;
                }
            }
        }

        let mut rows: HashMap<String, V2PickerRow> = HashMap::new();
        let insert =
            |row: V2PickerRow, rows: &mut HashMap<String, V2PickerRow>| match rows.get(&row.key) {
                Some(existing) if existing.updated_at >= row.updated_at => {}
                _ => {
                    rows.insert(row.key.clone(), row);
                }
            };

        // Picked snapshots and every Reference already used on a Note or Task.
        {
            let mut stmt = conn.prepare(
                "SELECT r.id, r.kind, r.target_id, r.canonical_url, r.snapshot_json,
                        COALESCE(
                            (SELECT MAX(n.updated_at) FROM line_refs lr
                             JOIN note_lines nl ON nl.id = lr.line_id
                             JOIN notes n ON n.id = nl.note_id WHERE lr.ref_id = r.id),
                            (SELECT MAX(t.updated_at) FROM task_refs tr
                             JOIN tasks t ON t.id = tr.task_id WHERE tr.ref_id = r.id),
                            r.created_at)
                 FROM ref_targets r",
            )?;
            let found = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (_, kind, target_id, canonical_url, snapshot_json, updated_at) in found {
                let Some(key) =
                    ReferenceKey::from_storage(&kind, &target_id, canonical_url.as_deref())
                else {
                    continue;
                };
                let snapshot = snapshot_json.as_deref().map(ReferenceSnapshot::from_json);
                let alias = picker_alias(&key, snapshot.as_ref(), &repo_counts);
                insert(
                    picker_row_from_key(
                        &key,
                        snapshot.as_ref(),
                        alias.as_deref(),
                        &updated_at,
                        &needle,
                    ),
                    &mut rows,
                );
            }
        }

        // Notes and Tasks are local Reference targets in their own right.
        for note in notebook_notes(&conn)? {
            let key = ReferenceKey::Note {
                id: note.id.clone(),
            };
            let title = note
                .lines
                .first()
                .map(|line| line.text.clone())
                .unwrap_or_else(|| note.body.clone());
            insert(
                local_row(key, title, "Note", None, note.updated_at, &needle),
                &mut rows,
            );
            for task in note.tasks {
                let key = ReferenceKey::Task {
                    id: task.id.clone(),
                };
                insert(
                    local_row(
                        key,
                        task.original_line.clone(),
                        "Task",
                        Some(task.status.as_str()),
                        task.updated_at,
                        &needle,
                    ),
                    &mut rows,
                );
            }
        }

        // The Import ledger outlives its Note, so an imported target stays pickable even
        // after the Note was deleted.
        {
            let mut stmt = conn.prepare(
                "SELECT source_kind, target_id, COALESCE(
                    (SELECT n.updated_at FROM notes n WHERE n.id = import_ledger.note_id),
                    import_ledger.created_at)
                 FROM import_ledger",
            )?;
            let found = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (source_kind, target_id, updated_at) in found {
                // Import ledger ids already include their CU- prefix for cards.
                let storage_id = if source_kind == "clickup" && !target_id.starts_with("CU-") {
                    format!("CU-{target_id}")
                } else {
                    target_id.clone()
                };
                let Some(key) = ReferenceKey::from_storage(&source_kind, &storage_id, None) else {
                    continue;
                };
                if rows.contains_key(&key.identity()) {
                    continue;
                }
                let alias = picker_alias(&key, None, &repo_counts);
                insert(
                    picker_row_from_key(&key, None, alias.as_deref(), &updated_at, &needle),
                    &mut rows,
                );
            }
        }

        // A typed URL or id names a known key immediately, even when it has no local row.
        if let Some(key) = super::reference::known_key(raw_query) {
            let identity = key.identity();
            rows.entry(identity.clone())
                .or_insert_with(|| picker_row_from_key(&key, None, None, &now(), &needle));
        }

        let mut result: Vec<V2PickerRow> = rows
            .into_values()
            .filter(|row| scope.allows(row.group))
            .filter(|row| picker_matches(row, &needle))
            .collect();
        result.sort_by(|a, b| {
            a.group
                .rank()
                .cmp(&b.group.rank())
                .then_with(|| b.exact_match.cmp(&a.exact_match))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.key.cmp(&b.key))
        });
        Ok(result)
    }

    /// FTS5 over the plain Note body. Each token is a quoted literal, so FTS operators
    /// and wildcards in user text stay text.
    pub fn search_notes(&self, text: &str, limit: usize) -> Result<Vec<V2Note>, V2Error> {
        let text = text.trim();
        if text.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM notes_fts JOIN notes n ON n.seq = notes_fts.rowid
                 WHERE notes_fts MATCH ?1 ORDER BY rank LIMIT ?2",
            )?;
            stmt.query_map(params![fts_literal(text), limit as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut notes = Vec::with_capacity(ids.len());
        for id in ids {
            notes.push(note_by_id(&tx, &id)?);
        }
        tx.commit()?;
        Ok(notes)
    }

    /// FTS5 over the frozen original directive text.
    pub fn search_tasks(&self, text: &str, limit: usize) -> Result<Vec<V2Task>, V2Error> {
        let text = text.trim();
        if text.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TASK_COLUMNS}
             FROM tasks_fts JOIN tasks t ON t.seq = tasks_fts.rowid
             JOIN note_lines l ON l.id = t.source_line_id
             JOIN notes n ON n.id = l.note_id
             WHERE tasks_fts MATCH ?1 ORDER BY rank LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![fts_literal(text), limit as i64], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// Picker helpers: query scopes, ranking and the shared Lens row contract
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum PickerScope {
    All,
    Cards,
    GitHub,
    Local,
}

impl PickerScope {
    fn allows(self, group: V2PickerGroup) -> bool {
        match self {
            Self::All => true,
            Self::Cards => group == V2PickerGroup::Cards,
            Self::GitHub => group == V2PickerGroup::Prs || group == V2PickerGroup::Issues,
            Self::Local => group == V2PickerGroup::NotesTasks,
        }
    }
}

fn picker_scope(raw: &str) -> (String, PickerScope) {
    let token = raw.strip_prefix('@').unwrap_or(raw).trim();
    if token.len() > 1 && token.starts_with('#') && token[1..].bytes().all(|b| b.is_ascii_digit()) {
        return (token.to_ascii_lowercase(), PickerScope::GitHub);
    }
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("cu-") {
        return (lower, PickerScope::Cards);
    }
    if lower.starts_with("n-") || lower.starts_with("t-") {
        return (lower, PickerScope::Local);
    }
    (lower, PickerScope::All)
}

fn picker_alias(
    key: &ReferenceKey,
    snapshot: Option<&ReferenceSnapshot>,
    repo_counts: &HashMap<String, usize>,
) -> Option<String> {
    match key {
        ReferenceKey::Slack { .. } => snapshot
            .and_then(|snapshot| snapshot.channel_name.clone())
            .filter(|name| !name.is_empty()),
        ReferenceKey::Github { owner_repo, .. } => {
            let (owner, repo) = owner_repo.rsplit_once('/')?;
            (repo_counts.get(repo).copied().unwrap_or(0) > 1).then(|| owner.to_string())
        }
        _ => None,
    }
}

fn picker_row_from_key(
    key: &ReferenceKey,
    snapshot: Option<&ReferenceSnapshot>,
    alias: Option<&str>,
    updated_at: &str,
    needle: &str,
) -> V2PickerRow {
    let title = snapshot
        .and_then(|snapshot| snapshot.title.clone())
        .unwrap_or_else(|| key.target_id());
    let state = snapshot.and_then(|snapshot| snapshot.state.clone());
    let mut metadata = snapshot
        .map(|snapshot| snapshot.metadata.clone())
        .unwrap_or_default();
    metadata.truncate(3);
    let reason = snapshot.and_then(|snapshot| snapshot.reason.clone());
    let availability = if reason.is_some() {
        ReferenceAvailability::Unavailable
    } else {
        ReferenceAvailability::Ready
    };
    let settings_destination = (availability == ReferenceAvailability::Unavailable).then(|| {
        match key.kind() {
            "slack" => "Settings → Slack",
            "github" => "Settings → GitHub",
            "clickup" => "Settings → ClickUp",
            _ => "Settings",
        }
        .to_string()
    });
    let mut row = V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(alias),
        canonical_link: key.canonical_url(),
        availability,
        group: key.group(),
        title,
        state,
        metadata,
        snapshot: snapshot.cloned(),
        unavailable_reason: reason,
        settings_destination,
        updated_at: updated_at.to_string(),
        exact_match: false,
    };
    row.exact_match = picker_exact(&row, needle);
    row
}

fn local_row(
    key: ReferenceKey,
    title: String,
    label: &str,
    state: Option<&str>,
    updated_at: String,
    needle: &str,
) -> V2PickerRow {
    let mut row = V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(None),
        canonical_link: key.canonical_url(),
        availability: ReferenceAvailability::Ready,
        group: key.group(),
        title,
        state: state.map(str::to_string),
        metadata: vec![label.to_string()],
        snapshot: None,
        unavailable_reason: None,
        settings_destination: None,
        updated_at,
        exact_match: false,
    };
    row.exact_match = picker_exact(&row, needle);
    row
}

fn picker_exact(row: &V2PickerRow, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let needle = needle.trim_start_matches('@');
    let target = row
        .key
        .split_once(':')
        .map(|(_, target)| target)
        .unwrap_or(&row.key);
    [
        target,
        row.display_id.trim_start_matches('@'),
        row.title.as_str(),
    ]
    .iter()
    .any(|value| value.eq_ignore_ascii_case(needle))
}

fn picker_matches(row: &V2PickerRow, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    [
        row.key.as_str(),
        row.display_id.as_str(),
        row.title.as_str(),
        row.canonical_link.as_str(),
    ]
    .iter()
    .any(|value| value.to_ascii_lowercase().contains(needle))
        || row
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.content.as_deref())
            .is_some_and(|content| content.to_ascii_lowercase().contains(needle))
}

fn notebook_notes(conn: &Connection) -> Result<Vec<V2Note>, V2Error> {
    let ids = {
        let mut stmt = conn.prepare("SELECT id FROM notes ORDER BY created_at, seq")?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    ids.iter().map(|id| note_by_id(conn, id)).collect()
}

// ---------------------------------------------------------------------------
// Boot: inspect before touching journal mode
// ---------------------------------------------------------------------------

fn inspect_database(conn: &Connection) -> Result<(), V2Error> {
    let application_id: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let user_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let tables: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;

    if application_id == APPLICATION_ID && user_version <= USER_VERSION {
        return Ok(());
    }
    if application_id == 0 && user_version == 0 && tables == 0 {
        return Ok(());
    }
    Err(V2Error::UnexpectedDatabase(format!(
        "application_id={application_id}, user_version={user_version}, tables={tables}"
    )))
}

fn migrate(conn: &Connection) -> Result<(), V2Error> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 0 || version > USER_VERSION {
        return Err(V2Error::UnexpectedDatabase(format!(
            "unsupported v2 user_version {version}"
        )));
    }

    // Apply whichever schema changes are actually missing. A single transaction keeps
    // the two parallel `0002` layouts from leaving a half-upgraded file behind.
    let tx = conn.unchecked_transaction()?;
    for migration in MIGRATIONS {
        let applied: bool =
            tx.query_row(&format!("SELECT ({})", migration.applied), [], |row| {
                row.get(0)
            })?;
        if !applied {
            tx.execute_batch(migration.sql)?;
        }
    }
    if version == 0 {
        // A fresh store gets the typed defaults only when the init migration created
        // the settings table during this upgrade.
        tx.execute(
            "INSERT INTO settings
                 (id, import_clickup_enabled, import_github_enabled, import_scope, updated_at)
             VALUES (1, 0, 0, NULL, ?1)",
            params![now()],
        )?;
    }
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", USER_VERSION)?;
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Write helpers
// ---------------------------------------------------------------------------

fn validate_lines(
    tx: &Transaction<'_>,
    note_id: &str,
    lines: &[PreparedLine],
) -> Result<(), V2Error> {
    let mut seen = HashSet::new();
    for line in lines {
        let Some(id) = &line.line_id else { continue };
        if !seen.insert(id.as_str()) {
            return Err(V2Error::InvalidInput(format!(
                "line {id} appears more than once in the write"
            )));
        }
        let owner: Option<String> = tx
            .query_row(
                "SELECT note_id FROM note_lines WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        match owner {
            None => return Err(V2Error::NotFound(format!("line {id}"))),
            Some(owner) if owner != note_id => return Err(V2Error::ForeignLine(id.clone())),
            Some(_) => {}
        }
    }
    Ok(())
}

fn upsert_line(
    tx: &Transaction<'_>,
    note_id: &str,
    line: &PreparedLine,
    position: i64,
    at: &str,
) -> Result<String, V2Error> {
    if let Some(id) = &line.line_id {
        tx.execute(
            "UPDATE note_lines SET text = ?1, position = ?2, updated_at = ?3
             WHERE id = ?4 AND note_id = ?5",
            params![line.text, position, at, id, note_id],
        )?;
        return Ok(id.clone());
    }
    let id = new_id();
    tx.execute(
        "INSERT INTO note_lines (id, note_id, text, position, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, note_id, line.text, position, at],
    )?;
    Ok(id)
}

fn replace_line_refs(
    tx: &Transaction<'_>,
    line_id: &str,
    references: &[PreparedReference],
    at: &str,
) -> Result<(), V2Error> {
    tx.execute("DELETE FROM line_refs WHERE line_id = ?1", params![line_id])?;
    for (position, reference) in references.iter().enumerate() {
        let kind = reference.kind.trim();
        let target_id = reference.target_id.trim();
        if kind.is_empty() || target_id.is_empty() {
            return Err(V2Error::InvalidInput(
                "a Reference needs a kind and a canonical target id".into(),
            ));
        }
        if reference.canonical_url.trim().is_empty() {
            return Err(V2Error::InvalidInput(
                "a Reference needs a canonical URL".into(),
            ));
        }
        tx.execute(
            "INSERT INTO ref_targets (id, kind, target_id, canonical_url, snapshot_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(kind, target_id) DO UPDATE SET
                 canonical_url = excluded.canonical_url,
                 snapshot_json = COALESCE(excluded.snapshot_json, ref_targets.snapshot_json)",
            params![
                new_id(),
                kind,
                target_id,
                reference.canonical_url,
                reference.snapshot_json,
                at,
            ],
        )?;
        // Snapshot the line's link and, atomically with the Note write, give an unreadable
        // provider target its one durable recovery row. A picked snapshot is preserved by the
        // COALESCE above and is fresh, so picking plus saving creates no recovery work.
        let snapshot_json: Option<String> = tx.query_row(
            "SELECT snapshot_json FROM ref_targets WHERE kind = ?1 AND target_id = ?2",
            params![kind, target_id],
            |row| row.get(0),
        )?;
        if matches!(kind, "github" | "clickup") {
            let snapshot = snapshot_json.as_deref().map(ReferenceSnapshot::from_json);
            if snapshot_needs_recovery(snapshot.as_ref(), rfc3339_ms(at), STALE_SNAPSHOT_MS) {
                if let Some(key) =
                    ReferenceKey::from_storage(kind, target_id, Some(&reference.canonical_url))
                {
                    tx.execute(
                        "INSERT OR IGNORE INTO ref_recovery
                             (key, kind, target_id, canonical_url, provider, attempts,
                              next_due_at, next_due_ms, reason, paused, paused_config, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, NULL, 0, NULL, ?6)",
                        params![
                            key.identity(),
                            kind,
                            target_id,
                            reference.canonical_url,
                            kind,
                            at,
                            rfc3339_ms(at),
                        ],
                    )?;
                }
            }
        }
        let ref_id: String = tx.query_row(
            "SELECT id FROM ref_targets WHERE kind = ?1 AND target_id = ?2",
            params![kind, target_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO line_refs (line_id, ref_id, position) VALUES (?1, ?2, ?3)",
            params![line_id, ref_id, position as i64],
        )?;
    }
    Ok(())
}

fn ensure_project(tx: &Transaction<'_>, project_id: Option<&str>) -> Result<(), V2Error> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    let exists = tx
        .query_row(
            "SELECT 1 FROM projects WHERE id = ?1",
            params![project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Err(V2Error::NotFound(format!("project {project_id}")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn settings_from_row(conn: &Connection) -> Result<V2Settings, V2Error> {
    conn.query_row(
        "SELECT import_clickup_enabled, import_github_enabled, import_scope,
                opencode_model, updated_at
         FROM settings WHERE id = 1",
        [],
        |row| {
            Ok(V2Settings {
                import_clickup_enabled: row.get::<_, i64>(0)? != 0,
                import_github_enabled: row.get::<_, i64>(1)? != 0,
                import_scope: row.get(2)?,
                opencode_model: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .map_err(Into::into)
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<V2Project> {
    Ok(V2Project {
        id: row.get("id")?,
        name: row.get("name")?,
        slug: row.get("slug")?,
        repo_path: row.get("repo_path")?,
        owner_repo: row.get("owner_repo")?,
        t3_target: row.get("t3_target")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn reference_from_row(row: &Row<'_>) -> rusqlite::Result<V2Reference> {
    Ok(V2Reference {
        id: row.get("id")?,
        kind: row.get("kind")?,
        target_id: row.get("target_id")?,
        canonical_url: row.get("canonical_url")?,
        snapshot_json: row.get("snapshot_json")?,
    })
}

fn refs_for(
    conn: &Connection,
    table: &str,
    owner_column: &str,
    owner_id: &str,
) -> Result<Vec<V2Reference>, V2Error> {
    let sql = format!(
        "SELECT r.id, r.kind, r.target_id, r.canonical_url, r.snapshot_json
         FROM {table} m JOIN ref_targets r ON r.id = m.ref_id
         WHERE m.{owner_column} = ?1 ORDER BY m.position"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![owner_id], reference_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn note_by_id(conn: &Connection, id: &str) -> Result<V2Note, V2Error> {
    let (revision, source, body, created_at, updated_at): (i64, String, String, String, String) =
        conn.query_row(
            "SELECT rev, source, body, created_at, updated_at FROM notes WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| V2Error::NotFound(format!("note {id}")))?;

    let line_rows = {
        let mut stmt = conn.prepare(
            "SELECT id, text, position FROM note_lines
             WHERE note_id = ?1 AND position IS NOT NULL ORDER BY position",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut lines = Vec::with_capacity(line_rows.len());
    for (line_id, text, position) in line_rows {
        lines.push(V2NoteLine {
            id: line_id.clone(),
            text,
            position: Some(position),
            references: refs_for(conn, "line_refs", "line_id", &line_id)?,
        });
    }

    let task_rows = {
        let mut stmt = conn.prepare(
            "SELECT id, source_line_id, original_line, agent_mode, status, project_id,
                    brief, grill_turn, pending_action, failure_stage, failure_reason,
                    thread_url, created_at, updated_at
             FROM tasks WHERE source_line_id IN (SELECT id FROM note_lines WHERE note_id = ?1)
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut tasks = Vec::with_capacity(task_rows.len());
    for (
        task_id,
        source_line_id,
        original_line,
        mode,
        status,
        project_id,
        brief,
        grill_turn,
        pending_action,
        failure_stage,
        failure_reason,
        thread_url,
        created_at,
        updated_at,
    ) in task_rows
    {
        let agent_mode = AgentMode::parse(&mode)
            .ok_or_else(|| V2Error::InvalidInput(format!("unsupported agent mode: {mode}")))?;
        let task_status = TaskStatus::parse(&status)
            .ok_or_else(|| V2Error::InvalidInput(format!("unsupported task status: {status}")))?;
        tasks.push(V2Task {
            id: task_id.clone(),
            source_line_id,
            note_id: id.to_string(),
            original_line,
            agent_mode,
            status: task_status,
            project_id,
            brief,
            grill_turn: parse_grill_turn(grill_turn)?,
            pending_action,
            failure_stage,
            failure_reason,
            thread_url,
            references: refs_for(conn, "task_refs", "task_id", &task_id)?,
            created_at,
            updated_at,
        });
    }

    Ok(V2Note {
        id: id.to_string(),
        revision,
        source,
        body,
        created_at,
        updated_at,
        lines,
        tasks,
    })
}

fn task_by_id(conn: &Connection, id: &str) -> Result<V2Task, V2Error> {
    let row = conn
        .query_row(
            &format!(
                "SELECT {TASK_COLUMNS}
             FROM tasks t
             JOIN note_lines l ON l.id = t.source_line_id
             JOIN notes n ON n.id = l.note_id
             WHERE t.id = ?1"
            ),
            params![id],
            task_from_row,
        )
        .optional()?
        .ok_or_else(|| V2Error::NotFound(format!("task {id}")))?;
    Ok(V2Task {
        references: refs_for(conn, "task_refs", "task_id", id)?,
        ..row
    })
}

fn parse_grill_turn(value: Option<String>) -> Result<Option<GrillTurn>, V2Error> {
    match value {
        Some(value) => GrillTurn::parse(&value)
            .map(Some)
            .ok_or_else(|| V2Error::InvalidInput(format!("unsupported grill turn: {value}"))),
        None => Ok(None),
    }
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<V2Task> {
    // Column order is TASK_COLUMNS; task_by_id and search_tasks share this mapper.
    let mode: String = row.get(4)?;
    let status: String = row.get(5)?;
    let agent_mode = AgentMode::parse(&mode).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            format!("unsupported agent mode: {mode}").into(),
        )
    })?;
    let task_status = TaskStatus::parse(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            format!("unsupported task status: {status}").into(),
        )
    })?;
    let grill_turn = parse_grill_turn(row.get(8)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            error.to_string().into(),
        )
    })?;
    Ok(V2Task {
        id: row.get(0)?,
        source_line_id: row.get(1)?,
        note_id: row.get(2)?,
        original_line: row.get(3)?,
        agent_mode,
        status: task_status,
        project_id: row.get(6)?,
        brief: row.get(7)?,
        grill_turn,
        pending_action: row.get(9)?,
        failure_stage: row.get(10)?,
        failure_reason: row.get(11)?,
        thread_url: row.get(12)?,
        references: Vec::new(),
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn none_if_blank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// The `(kind, target_id)` a canonical key maps to in `ref_targets`. The PR/issue distinction
/// is deliberately absent: identity excludes presentation, and the storage key is the same.
fn storage_identity(key: &str) -> Option<(&str, String)> {
    let (kind, rest) = key.split_once(':')?;
    if rest.is_empty() {
        return None;
    }
    let target_id = if kind == "clickup" {
        format!("CU-{rest}")
    } else {
        rest.to_string()
    };
    Some((kind, target_id))
}

fn rfc3339_ms(raw: &str) -> i64 {
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .map(|value| (value.unix_timestamp_nanos() / 1_000_000) as i64)
        .unwrap_or(0)
}

fn recovery_from_row(row: &Row<'_>) -> rusqlite::Result<RecoveryTarget> {
    let reason: Option<String> = row.get(8)?;
    Ok(RecoveryTarget {
        key: row.get(0)?,
        kind: row.get(1)?,
        target_id: row.get(2)?,
        canonical_url: row.get(3)?,
        provider: row.get(4)?,
        attempts: row.get(5)?,
        next_due_at: row.get(6)?,
        next_due_ms: row.get(7)?,
        reason: reason.as_deref().and_then(RecoveryReason::parse),
        paused: row.get::<_, i64>(9)? != 0,
        paused_config: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

/// A Note carries no project column. A project filter matches it through a Task's saved
/// project or through a literal standalone `+slug` left in one of its editable lines.
fn note_project_hit(note: &V2Note, project_ids: &[String], slugs: &[String]) -> bool {
    note.tasks.iter().any(|task| {
        task.project_id
            .as_ref()
            .is_some_and(|id| project_ids.contains(id))
    }) || note.lines.iter().any(|line| {
        line.text.split_whitespace().any(|word| {
            let word = word.trim_end_matches(|ch: char| {
                matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}')
            });
            slugs
                .iter()
                .any(|slug| word.eq_ignore_ascii_case(&format!("+{slug}")))
        })
    })
}

/// Quotes every token as a literal FTS5 phrase. Doubling `"` is the FTS escape.
fn fts_literal(text: &str) -> String {
    text.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}
