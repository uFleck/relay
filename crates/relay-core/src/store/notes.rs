//! Notes: local text with FTS5 search and pinning. A note never syncs anywhere; the only ways out
//! are a task (`note_to_task`, which agents then claim over MCP) and the UI's copy-to-T3 handoff.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Store, StoreError};
use crate::model::{NewTask, Note, Task, new_id, now};

const NOTE_COLUMNS: &str = "id, title, body, pinned, created_at, updated_at";
const DEFAULT_NOTE_LIMIT: usize = 200;
const MAX_NOTE_LIMIT: usize = 1000;
/// A task title cut from a note body stays one readable line.
const TASK_TITLE_CHARS: usize = 120;

impl Store {
    pub fn create_note(&self, title: &str, body: &str) -> Result<Note, StoreError> {
        let at = now();
        let note = Note {
            id: new_id(),
            title: title.to_string(),
            body: body.to_string(),
            pinned: false,
            created_at: at.clone(),
            updated_at: at,
        };
        self.conn().execute(
            "INSERT INTO notes (id, title, body, pinned, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                note.id,
                note.title,
                note.body,
                note.created_at,
                note.updated_at
            ],
        )?;
        Ok(note)
    }

    pub fn get_note(&self, id: &str) -> Result<Note, StoreError> {
        note_by_id(&self.conn(), id)
    }

    /// Replaces the title and body. Last write wins: only the main window edits notes.
    pub fn update_note(&self, id: &str, title: &str, body: &str) -> Result<Note, StoreError> {
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE notes SET title = ?2, body = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, title, body, now()],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("note {id}")));
        }
        note_by_id(&conn, id)
    }

    /// Pinning does not touch `updated_at`, so a note keeps its place among its peers.
    pub fn set_note_pinned(&self, id: &str, pinned: bool) -> Result<Note, StoreError> {
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE notes SET pinned = ?2 WHERE id = ?1",
            params![id, pinned],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("note {id}")));
        }
        note_by_id(&conn, id)
    }

    pub fn delete_note(&self, id: &str) -> Result<(), StoreError> {
        let changed = self
            .conn()
            .execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("note {id}")));
        }
        Ok(())
    }

    /// Pinned first, then the most recently edited. A non-empty `text` filters through FTS5:
    /// every token is a literal prefix, so search-as-you-type matches `rel` in `relay`.
    pub fn list_notes(
        &self,
        text: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Note>, StoreError> {
        let limit = limit.unwrap_or(DEFAULT_NOTE_LIMIT).clamp(1, MAX_NOTE_LIMIT) as i64;
        let text = text.map(str::trim).filter(|text| !text.is_empty());
        let conn = self.conn();
        let mut notes = Vec::new();
        match text {
            None => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {NOTE_COLUMNS} FROM notes
                     ORDER BY pinned DESC, updated_at DESC, seq DESC
                     LIMIT ?1"
                ))?;
                for row in stmt.query_map(params![limit], note_from_row)? {
                    notes.push(row?);
                }
            }
            Some(text) => {
                let mut stmt = conn.prepare(
                    "SELECT n.id, n.title, n.body, n.pinned, n.created_at, n.updated_at
                     FROM notes_fts JOIN notes n ON n.seq = notes_fts.rowid
                     WHERE notes_fts MATCH ?1
                     ORDER BY n.pinned DESC, rank, n.updated_at DESC
                     LIMIT ?2",
                )?;
                for row in stmt.query_map(params![prefix_query(text), limit], note_from_row)? {
                    notes.push(row?);
                }
            }
        }
        Ok(notes)
    }

    /// Creates a task from a note: the title (or the body's first line) becomes the task title,
    /// the body its notes, and `metadata.noteId` points back. The note stays.
    pub fn note_to_task(&self, note_id: &str, project_id: &str) -> Result<Task, StoreError> {
        let note = self.get_note(note_id)?;
        let title = task_title(&note).ok_or_else(|| {
            StoreError::InvalidInput("the note needs a title or some text first".into())
        })?;
        let mut new = NewTask::new(project_id, &title);
        new.notes = Some(note.body.trim().to_string()).filter(|body| !body.is_empty());
        new.source = Some("note".into());
        new.metadata_json = Some(serde_json::json!({ "noteId": note.id }).to_string());
        self.create_task(new)
    }
}

fn task_title(note: &Note) -> Option<String> {
    let line = Some(note.title.trim())
        .filter(|title| !title.is_empty())
        .or_else(|| {
            note.body
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
        })?;
    Some(line.chars().take(TASK_TITLE_CHARS).collect())
}

/// Like the task search's `fts_query`, each token is a quoted literal, so FTS operators in user
/// input stay text; the trailing `*` makes it a prefix.
fn prefix_query(text: &str) -> String {
    text.split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn note_by_id(conn: &Connection, id: &str) -> Result<Note, StoreError> {
    conn.query_row(
        &format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1"),
        params![id],
        note_from_row,
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("note {id}")))
}

fn note_from_row(row: &Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get("id")?,
        title: row.get("title")?,
        body: row.get("body")?,
        pinned: row.get("pinned")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewProject;

    fn titles(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|note| note.title.as_str()).collect()
    }

    #[test]
    fn notes_round_trip_and_list_pinned_first() {
        let store = Store::open_in_memory().unwrap();
        let first = store.create_note("First", "one").unwrap();
        let second = store.create_note("Second", "two").unwrap();
        store
            .update_note(&first.id, "First", "one, edited")
            .unwrap();
        // Two writes can share a millisecond; pin the order the edit implies.
        store
            .conn()
            .execute(
                "UPDATE notes SET updated_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
                params![second.id],
            )
            .unwrap();
        let second = store.get_note(&second.id).unwrap();

        // The edit makes `First` the most recent.
        assert_eq!(
            titles(&store.list_notes(None, None).unwrap()),
            ["First", "Second"]
        );

        let pinned = store.set_note_pinned(&second.id, true).unwrap();
        assert!(pinned.pinned);
        assert_eq!(pinned.updated_at, second.updated_at);
        assert_eq!(
            titles(&store.list_notes(None, None).unwrap()),
            ["Second", "First"]
        );

        assert_eq!(store.get_note(&first.id).unwrap().body, "one, edited");
        store.delete_note(&first.id).unwrap();
        assert!(matches!(
            store.get_note(&first.id),
            Err(StoreError::NotFound(_))
        ));
        assert!(matches!(
            store.delete_note(&first.id),
            Err(StoreError::NotFound(_))
        ));
        assert!(matches!(
            store.update_note("missing", "", ""),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn search_matches_prefixes_follows_edits_and_keeps_operators_literal() {
        let store = Store::open_in_memory().unwrap();
        let deploy = store
            .create_note("Deploy bill", "check the RC first")
            .unwrap();
        store.create_note("Groceries", "milk OR eggs").unwrap();

        let search = |text: &str| titles(&store.list_notes(Some(text), None).unwrap()).join(",");
        assert_eq!(search("depl"), "Deploy bill");
        assert_eq!(search("rc"), "Deploy bill");
        assert_eq!(store.list_notes(Some("  "), None).unwrap().len(), 2);
        assert_eq!(search("OR"), "Groceries");
        assert_eq!(search("NOT \"milk"), "");

        store
            .update_note(&deploy.id, "Ship relay", "notes view")
            .unwrap();
        assert_eq!(search("depl"), "");
        assert_eq!(search("view"), "Ship relay");

        store.delete_note(&deploy.id).unwrap();
        assert_eq!(search("view"), "");
    }

    #[test]
    fn a_note_becomes_a_task_with_a_link_back() {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project_with_default_workflow(NewProject::new("Relay", "relay", None))
            .unwrap()
            .project;

        let titled = store
            .create_note("Notes view", "  list + editor  ")
            .unwrap();
        let task = store.note_to_task(&titled.id, &project.id).unwrap();
        assert_eq!(task.title, "Notes view");
        assert_eq!(task.notes, "list + editor");
        assert_eq!(task.source, "note");
        let metadata: serde_json::Value = serde_json::from_str(&task.metadata_json).unwrap();
        assert_eq!(metadata["noteId"], titled.id.as_str());
        assert!(store.get_note(&titled.id).is_ok());

        let untitled = store.create_note("", "\n  fix the hover\nmore").unwrap();
        let task = store.note_to_task(&untitled.id, &project.id).unwrap();
        assert_eq!(task.title, "fix the hover");

        let empty = store.create_note(" ", " \n ").unwrap();
        assert!(matches!(
            store.note_to_task(&empty.id, &project.id),
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.note_to_task(&titled.id, "missing"),
            Err(StoreError::NotFound(_))
        ));
    }
}
