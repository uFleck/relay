//! The v2 SQLite store. One file (`relay-v2.db`), one writer behind a mutex, its own
//! `application_id` and `user_version`, and no shared table with the v1 store.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use super::model::{
    AgentMode, ImportRecord, NewImportRecord, NewV2Project, NoteWrite, PreparedLine,
    PreparedNoteWrite, PreparedReference, TaskStatus, V2Note, V2NoteLine, V2NotebookSnapshot,
    V2Project, V2Reference, V2Settings, V2SettingsUpdate, V2Task,
};
use crate::model::{new_id, now};

const MIGRATION: &str = include_str!("../../migrations/v2/0001_init.sql");
/// 'RELY' as an integer. A non-zero application id marks a file as ours.
pub(super) const APPLICATION_ID: i64 = 0x5245_4C59;
pub(super) const USER_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

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
                    tx.execute(
                        "INSERT INTO tasks
                             (id, source_line_id, original_line, agent_mode, status, project_id,
                              brief, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?7)",
                        params![
                            task_id,
                            line_id,
                            line.text,
                            intent.agent_mode.as_str(),
                            status.as_str(),
                            intent.project_id,
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
        let mut stmt = conn.prepare(
            "SELECT t.id, t.source_line_id, n.id, t.original_line, t.agent_mode, t.status,
                    t.project_id, t.brief, t.created_at, t.updated_at
             FROM tasks_fts JOIN tasks t ON t.seq = tasks_fts.rowid
             JOIN note_lines l ON l.id = t.source_line_id
             JOIN notes n ON n.id = l.note_id
             WHERE tasks_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_literal(text), limit as i64], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
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
    if version == USER_VERSION {
        return Ok(());
    }
    if version != 0 {
        return Err(V2Error::UnexpectedDatabase(format!(
            "unsupported v2 user_version {version}"
        )));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(MIGRATION)?;
    tx.execute(
        "INSERT INTO settings
             (id, import_clickup_enabled, import_github_enabled, import_scope, updated_at)
         VALUES (1, 0, 0, NULL, ?1)",
        params![now()],
    )?;
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
        "SELECT import_clickup_enabled, import_github_enabled, import_scope, updated_at
         FROM settings WHERE id = 1",
        [],
        |row| {
            Ok(V2Settings {
                import_clickup_enabled: row.get::<_, i64>(0)? != 0,
                import_github_enabled: row.get::<_, i64>(1)? != 0,
                import_scope: row.get(2)?,
                updated_at: row.get(3)?,
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
                    brief, created_at, updated_at
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
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
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
            "SELECT t.id, t.source_line_id, n.id, t.original_line, t.agent_mode, t.status,
                    t.project_id, t.brief, t.created_at, t.updated_at
             FROM tasks t
             JOIN note_lines l ON l.id = t.source_line_id
             JOIN notes n ON n.id = l.note_id
             WHERE t.id = ?1",
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

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<V2Task> {
    // Column order shared by task_by_id and search_tasks:
    // id, source_line_id, note_id, original_line, agent_mode, status, project_id,
    // brief, created_at, updated_at.
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
    Ok(V2Task {
        id: row.get(0)?,
        source_line_id: row.get(1)?,
        note_id: row.get(2)?,
        original_line: row.get(3)?,
        agent_mode,
        status: task_status,
        project_id: row.get(6)?,
        brief: row.get(7)?,
        references: Vec::new(),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn none_if_blank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Quotes every token as a literal FTS5 phrase. Doubling `"` is the FTS escape.
fn fts_literal(text: &str) -> String {
    text.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}
