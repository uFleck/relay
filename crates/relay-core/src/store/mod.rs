//! SQLite store: one writer behind a mutex, migrations through `PRAGMA user_version`.
//!
//! The database is the local source of truth. Everything else (Slack, JEV, GitHub,
//! ClickUp, MCP) reads and writes through these services; adapters stay thin.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::model::{Project, ProjectStatus, Task};

const MIGRATIONS: &[&str] = &[include_str!("../../migrations/0001_init.sql")];

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
    #[error("task not found: {0}")]
    NotFound(String),
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

    pub fn insert_project(&self, project: &Project) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO projects
                 (id, name, slug, repo_path, color, integrations_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project.id.as_str(),
                project.name.as_str(),
                project.slug.as_str(),
                project.repo_path.as_deref(),
                project.color.as_deref(),
                project.integrations_json.as_str(),
                project.created_at.as_str(),
                project.updated_at.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn insert_status(&self, status: &ProjectStatus) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO project_statuses (id, project_id, name, category, position, color)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                status.id.as_str(),
                status.project_id.as_str(),
                status.name.as_str(),
                status.category.as_str(),
                status.position,
                status.color.as_deref(),
            ],
        )?;
        Ok(())
    }

    pub fn insert_task(&self, task: &Task) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO tasks
                 (id, project_id, status_id, title, notes, priority, due_at, position, source, rev,
                  archived, created_at, updated_at, completed_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                task.id.as_str(),
                task.project_id.as_str(),
                task.status_id.as_str(),
                task.title.as_str(),
                task.notes.as_str(),
                task.priority,
                task.due_at.as_deref(),
                task.position,
                task.source.as_str(),
                task.rev,
                task.archived,
                task.created_at.as_str(),
                task.updated_at.as_str(),
                task.completed_at.as_deref(),
                task.metadata_json.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Task, StoreError> {
        self.conn()
            .query_row(
                "SELECT id, project_id, status_id, title, notes, priority, due_at, position,
                        source, rev, archived, created_at, updated_at, completed_at, metadata_json
                 FROM tasks WHERE id = ?1",
                params![id],
                task_from_row,
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    /// Active tasks, newest workflow position first. `None` lists every project.
    pub fn list_tasks(&self, project_id: Option<&str>) -> Result<Vec<Task>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, status_id, title, notes, priority, due_at, position,
                    source, rev, archived, created_at, updated_at, completed_at, metadata_json
             FROM tasks
             WHERE archived = 0 AND (?1 IS NULL OR project_id = ?1)
             ORDER BY position, created_at",
        )?;
        let rows = stmt.query_map(params![project_id], task_from_row)?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }

    /// FTS5 search over title and notes, for task search and JEV candidates.
    pub fn search_tasks(&self, text: &str, limit: usize) -> Result<Vec<Task>, StoreError> {
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
        let rows = stmt.query_map(params![text, limit as i64], task_from_row)?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        Ok(tasks)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{new_id, StatusCategory};

    fn project() -> Project {
        let now = "2026-11-17T12:00:00Z".to_string();
        Project {
            id: new_id(),
            name: "Relay".to_string(),
            slug: "relay".to_string(),
            repo_path: Some("C:\\Projects\\relay".to_string()),
            color: None,
            integrations_json: "{}".to_string(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    fn status(project_id: &str) -> ProjectStatus {
        ProjectStatus {
            id: new_id(),
            project_id: project_id.to_string(),
            name: "Todo".to_string(),
            category: StatusCategory::Todo,
            position: 1,
            color: None,
        }
    }

    fn task(project_id: &str, status_id: &str) -> Task {
        let now = "2026-11-17T12:00:00Z".to_string();
        Task {
            id: new_id(),
            project_id: project_id.to_string(),
            status_id: status_id.to_string(),
            title: "Deploy the RC build".to_string(),
            notes: "Wire the updater endpoint".to_string(),
            priority: Some(2),
            due_at: None,
            position: 1,
            source: "local".to_string(),
            rev: 1,
            archived: false,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
            metadata_json: "{}".to_string(),
        }
    }

    #[test]
    fn migrations_and_task_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let foreign_keys: i64 = store
            .conn()
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);

        let project = project();
        store.insert_project(&project).unwrap();
        let status = status(&project.id);
        store.insert_status(&status).unwrap();
        let task = task(&project.id, &status.id);
        store.insert_task(&task).unwrap();

        let fetched = store.get_task(&task.id).unwrap();
        assert_eq!(fetched, task);

        let tasks = store.list_tasks(Some(&project.id)).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task.id);

        assert!(store.get_task("missing").is_err());
    }

    #[test]
    fn fts_index_tracks_tasks() {
        let store = Store::open_in_memory().unwrap();
        let project = project();
        store.insert_project(&project).unwrap();
        let status = status(&project.id);
        store.insert_status(&status).unwrap();
        let task = task(&project.id, &status.id);
        store.insert_task(&task).unwrap();

        let found = store.search_tasks("deploy", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, task.id);

        assert!(store.search_tasks("kubernetes", 10).unwrap().is_empty());
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
}
