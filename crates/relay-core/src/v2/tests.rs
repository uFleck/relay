//! File-backed acceptance tests for the fresh v2 store. Every fixture lives in a
//! uniquely named temp directory and is removed on drop. No live profile is touched.

use std::{fs, path::PathBuf};

use super::store::{APPLICATION_ID, USER_VERSION};
use super::*;
use crate::model::NewProject as V1NewProject;
use crate::model::new_id;
use crate::store::Store as V1Store;
use rusqlite::params;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("relay-v2-{tag}-{}", new_id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn reference(kind: &str, target: &str) -> PreparedReference {
    PreparedReference {
        kind: kind.to_string(),
        target_id: target.to_string(),
        canonical_url: format!("https://example.invalid/{kind}/{target}"),
        snapshot_json: Some(format!("{{\"title\":\"{target}\"}}")),
    }
}

fn plain(text: &str, references: Vec<PreparedReference>) -> PreparedLine {
    PreparedLine {
        line_id: None,
        text: text.to_string(),
        references,
        task_intent: None,
    }
}

fn directive(text: &str, agent_mode: AgentMode, project_id: Option<&str>) -> PreparedLine {
    PreparedLine {
        line_id: None,
        text: text.to_string(),
        references: vec![reference("github", "sample-app#142")],
        task_intent: Some(PreparedTaskIntent {
            agent_mode,
            project_id: project_id.map(str::to_string),
        }),
    }
}

fn create(key: &str, lines: Vec<PreparedLine>) -> PreparedNoteWrite {
    PreparedNoteWrite {
        write: NoteWrite::Create {
            submission_key: key.to_string(),
        },
        source: "composer".to_string(),
        lines,
    }
}

fn edit(id: &str, revision: i64, lines: Vec<PreparedLine>) -> PreparedNoteWrite {
    PreparedNoteWrite {
        write: NoteWrite::Edit {
            note_id: id.to_string(),
            expected_revision: revision,
        },
        source: "composer".to_string(),
        lines,
    }
}

#[test]
fn fresh_store_has_defaults_wal_foreign_keys_and_only_v2_tables() {
    let dir = TempDir::new("fresh");
    let path = dir.file("relay-v2.db");
    let store = V2Store::open(&path).unwrap();

    let snapshot = store.notebook_snapshot().unwrap();
    assert!(snapshot.notes.is_empty());
    assert!(snapshot.projects.is_empty());
    assert!(!snapshot.settings.import_clickup_enabled);
    assert!(!snapshot.settings.import_github_enabled);
    assert_eq!(snapshot.settings.import_scope, None);

    let conn = store.conn();
    let journal: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    let foreign_keys: i64 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    let application_id: i64 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    let user_version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    assert_eq!(foreign_keys, 1);
    assert_eq!(application_id, APPLICATION_ID);
    assert_eq!(user_version, USER_VERSION);

    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    for required in [
        "notes",
        "note_lines",
        "tasks",
        "ref_targets",
        "line_refs",
        "task_refs",
        "submission_keys",
        "import_ledger",
        "settings",
        "projects",
    ] {
        assert!(
            tables.iter().any(|name| name == required),
            "missing {required}"
        );
    }
    for retired in [
        "analysis_jobs",
        "claims",
        "change_acks",
        "conversations",
        "messages",
        "proposals",
        "project_statuses",
        "slack_peers",
        "task_changes",
        "task_events",
        "task_links",
        "sync_cursors",
    ] {
        assert!(
            !tables.iter().any(|name| name == retired),
            "v1 table {retired} survived"
        );
    }
    drop(conn);
    drop(store);

    let reopened = V2Store::open(&path).unwrap();
    assert_eq!(reopened.notebook_snapshot().unwrap(), snapshot);
}

#[test]
fn round_trip_persists_lines_tasks_references_settings_project_and_ledger() {
    let dir = TempDir::new("roundtrip");
    let path = dir.file("relay-v2.db");
    let store = V2Store::open(&path).unwrap();

    let project = store
        .register_project(NewV2Project {
            name: "Sample App".to_string(),
            slug: "SAMPLE-App".to_string(),
            repo_path: Some("C:/sample".to_string()),
            owner_repo: Some("uFleck/Sample-App".to_string()),
            t3_target: Some("sample".to_string()),
        })
        .unwrap();
    assert_eq!(project.slug, "sample-app");
    assert_eq!(project.owner_repo.as_deref(), Some("ufleck/sample-app"));

    let saved = store
        .save_note(create(
            "submission-1",
            vec![
                plain("a plain thought", vec![reference("clickup", "CU-8x1k2")]),
                directive("!ship the fix", AgentMode::Act, Some(&project.id)),
                directive("??think it through", AgentMode::Grill, None),
            ],
        ))
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.source, "composer");
    // Only plain lines are Note prose; directive lines stay as raw editable lines and Tasks.
    assert_eq!(saved.body, "a plain thought");
    assert_eq!(saved.lines.len(), 3);
    assert_eq!(saved.tasks.len(), 2);
    assert_eq!(saved.tasks[0].status, TaskStatus::Briefing);
    assert_eq!(saved.tasks[0].agent_mode, AgentMode::Act);
    assert_eq!(
        saved.tasks[0].project_id.as_deref(),
        Some(project.id.as_str())
    );
    assert_eq!(saved.tasks[1].status, TaskStatus::Grilling);
    assert_eq!(saved.tasks[1].project_id, None);
    for task in &saved.tasks {
        assert_eq!(task.note_id, saved.id);
        assert_eq!(task.references.len(), 1);
        assert_eq!(task.references[0].target_id, "sample-app#142");
    }

    store
        .update_settings(V2SettingsUpdate {
            import_clickup_enabled: true,
            import_github_enabled: true,
            import_scope: Some("[\"uFleck/sample-app\"]".to_string()),
        })
        .unwrap();
    store
        .record_import(NewImportRecord {
            source_kind: "github".to_string(),
            target_id: "sample-app#142".to_string(),
            note_id: Some(saved.id.clone()),
        })
        .unwrap();

    drop(store);
    let reopened = V2Store::open(&path).unwrap();
    let snapshot = reopened.notebook_snapshot().unwrap();
    assert_eq!(snapshot.notes, vec![saved.clone()]);
    assert_eq!(snapshot.projects, vec![project]);
    assert!(snapshot.settings.import_clickup_enabled);
    assert!(snapshot.settings.import_github_enabled);
    assert_eq!(
        snapshot.settings.import_scope.as_deref(),
        Some("[\"uFleck/sample-app\"]")
    );
    assert!(
        reopened
            .import_recorded("github", "sample-app#142")
            .unwrap()
    );

    assert_eq!(reopened.search_notes("plain thought", 10).unwrap().len(), 1);
    assert_eq!(reopened.search_tasks("ship", 10).unwrap().len(), 1);

    // Deleting the Note must not release the Import ledger key.
    reopened.delete_note(&saved.id).unwrap();
    assert!(reopened.notebook_snapshot().unwrap().notes.is_empty());
    assert!(
        reopened
            .import_recorded("github", "sample-app#142")
            .unwrap()
    );
    assert_eq!(reopened.list_imports().unwrap().len(), 1);
}

#[test]
fn v1_file_is_unmodified_and_rejected() {
    let dir = TempDir::new("isolation");
    let legacy_path = dir.file("relay.db");
    let artifact = dir.file("credential.json");

    let legacy = V1Store::open(&legacy_path).unwrap();
    let sentinel = legacy.create_note("Sentinel", "keep me").unwrap();
    legacy
        .create_project_with_default_workflow(V1NewProject::new("Inbox", "inbox", None))
        .unwrap();
    fs::write(&artifact, b"{ \"synthetic\": true }").unwrap();
    // Close v1 first: closing the last connection checkpoints its WAL into the main
    // file. The baseline must be the settled file, not the pre-checkpoint bytes.
    drop(legacy);
    let legacy_bytes = fs::read(&legacy_path).unwrap();
    let artifact_bytes = fs::read(&artifact).unwrap();

    let v2 = V2Store::open(dir.file("relay-v2.db")).unwrap();
    let saved = v2
        .save_note(create("side-by-side", vec![plain("v2 only", vec![])]))
        .unwrap();
    assert!(matches!(
        V2Store::open(&legacy_path),
        Err(V2Error::UnexpectedDatabase(_))
    ));

    assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);
    assert_eq!(fs::read(&artifact).unwrap(), artifact_bytes);

    let legacy = V1Store::open(&legacy_path).unwrap();
    assert_eq!(legacy.get_note(&sentinel.id).unwrap().body, "keep me");
    drop(legacy);

    assert_eq!(v2.notebook_snapshot().unwrap().notes, vec![saved]);
}

#[test]
fn write_contract_guards_replay_stale_edits_rollback_and_removed_lines() {
    let dir = TempDir::new("contract");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();

    let first = store
        .save_note(create(
            "key",
            vec![
                plain("alpha", vec![]),
                directive("!do it", AgentMode::Act, None),
            ],
        ))
        .unwrap();
    let replay = store
        .save_note(create("key", vec![plain("different", vec![])]))
        .unwrap();
    assert_eq!(replay, first);

    let other = store
        .save_note(create("other", vec![plain("beta", vec![])]))
        .unwrap();
    assert_ne!(other.id, first.id);

    assert!(matches!(
        store.save_note(edit(&first.id, 99, vec![plain("x", vec![])])),
        Err(V2Error::StaleRevision { .. })
    ));

    let foreign_line = other.lines[0].id.clone();
    assert!(matches!(
        store.save_note(edit(
            &first.id,
            first.revision,
            vec![PreparedLine {
                line_id: Some(foreign_line),
                text: "trespassing".to_string(),
                references: vec![],
                task_intent: None,
            }]
        )),
        Err(V2Error::ForeignLine(_))
    ));

    let same_line_twice = first.lines[0].id.clone();
    let duplicate_line = || PreparedLine {
        line_id: Some(same_line_twice.clone()),
        text: "alpha".to_string(),
        references: vec![],
        task_intent: None,
    };
    assert!(matches!(
        store.save_note(edit(
            &first.id,
            first.revision,
            vec![duplicate_line(), duplicate_line()]
        )),
        Err(V2Error::InvalidInput(_))
    ));
    assert_eq!(
        store.notebook_snapshot().unwrap().notes[0].revision,
        first.revision
    );

    // A late failure (unknown project on a new Task) must roll back the whole edit.
    let bad = store.save_note(edit(
        &first.id,
        first.revision,
        vec![
            plain("alpha", vec![]),
            directive("!new work", AgentMode::Act, Some("missing")),
        ],
    ));
    assert!(matches!(bad, Err(V2Error::NotFound(_))));
    let after_bad = store.notebook_snapshot().unwrap().notes[0].clone();
    assert_eq!(after_bad.revision, first.revision);
    assert_eq!(after_bad.body, first.body);
    assert_eq!(after_bad.lines.len(), 2);

    // Reorder with duplicate text and a kept line id: active positions stay unique.
    let kept_id = first.lines[0].id.clone();
    let reordered = store
        .save_note(edit(
            &first.id,
            first.revision,
            vec![
                plain("duplicate", vec![]),
                plain("duplicate", vec![]),
                PreparedLine {
                    line_id: Some(kept_id.clone()),
                    text: "alpha".to_string(),
                    references: vec![],
                    task_intent: None,
                },
            ],
        ))
        .unwrap();
    assert_eq!(reordered.revision, first.revision + 1);
    assert_eq!(reordered.lines.len(), 3);
    assert_eq!(
        reordered
            .lines
            .iter()
            .map(|line| line.position)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)]
    );
    assert_eq!(reordered.lines[2].id, kept_id);
    // The existing Task survived the reorder and kept its frozen original line.
    assert_eq!(reordered.tasks.len(), 1);
    assert_eq!(reordered.tasks[0].original_line, "!do it");

    // Remove the Task-bearing line. Its Task stays addressable with a null position.
    let removed = store
        .save_note(edit(
            &first.id,
            reordered.revision,
            vec![plain("only plain now", vec![])],
        ))
        .unwrap();
    assert_eq!(removed.lines.len(), 1);
    assert_eq!(removed.tasks.len(), 1);
    assert_eq!(removed.tasks[0].note_id, first.id);
    let (total, inactive): (i64, i64) = store
        .conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(position IS NULL), 0) FROM note_lines
             WHERE note_id = ?1",
            params![first.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(total, 5);
    assert_eq!(inactive, 4);

    // Project assignment is transactional and guarded.
    let project = store
        .register_project(NewV2Project {
            name: "Sample".to_string(),
            slug: "sample".to_string(),
            repo_path: None,
            owner_repo: None,
            t3_target: None,
        })
        .unwrap();
    let task_id = removed.tasks[0].id.clone();
    let assigned = store.assign_task_project(&task_id, &project.id).unwrap();
    assert_eq!(assigned.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(
        store
            .assign_task_project(&task_id, &project.id)
            .unwrap()
            .project_id,
        assigned.project_id
    );
    let second = store
        .register_project(NewV2Project {
            name: "Other".to_string(),
            slug: "other".to_string(),
            repo_path: None,
            owner_repo: None,
            t3_target: None,
        })
        .unwrap();
    assert!(matches!(
        store.assign_task_project(&task_id, &second.id),
        Err(V2Error::TaskAlreadyAssigned(_))
    ));
}

#[test]
fn workspace_snapshot_and_search_keep_punctuation_and_removed_task_lines() {
    let dir = TempDir::new("workspace");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    let project = store
        .register_project(NewV2Project {
            name: "Sample".to_string(),
            slug: "sample".to_string(),
            repo_path: None,
            owner_repo: Some("uFleck/Sample".to_string()),
            t3_target: None,
        })
        .unwrap();

    let saved = store
        .save_note(create(
            "punctuation",
            vec![
                plain("fix: the (v2) bug", vec![]),
                directive("!ship relay-v2", AgentMode::Act, Some(&project.id)),
            ],
        ))
        .unwrap();

    let workspace = store.workspace_snapshot().unwrap();
    assert_eq!(workspace.notes, vec![saved.clone()]);
    assert_eq!(workspace.tasks.len(), 1);
    assert_eq!(workspace.tasks[0].note_id, saved.id);
    assert_eq!(workspace.projects, vec![project]);

    // Punctuation in a query stays literal text and cannot become an FTS operator.
    let search = |query: &str| store.search_workspace(query).unwrap();
    assert!(search("fix: bug").note_ids.contains(&saved.id));
    assert!(search("relay-v2").task_ids.contains(&saved.tasks[0].id));
    assert!(search("relay-v2").note_ids.contains(&saved.id));
    assert!(search("NOT \"milk").note_ids.is_empty());
    // FTS operators are literal text, so `OR` cannot widen the match.
    assert!(search("OR").note_ids.is_empty());

    // Removing the source line keeps the Task, its frozen text and its Note searchable.
    let trimmed = store
        .save_note(edit(
            &saved.id,
            saved.revision,
            vec![plain("only the plain line", vec![])],
        ))
        .unwrap();
    assert!(
        trimmed
            .lines
            .iter()
            .all(|line| line.text != "!ship relay-v2")
    );
    let after = store.workspace_snapshot().unwrap();
    assert_eq!(after.tasks.len(), 1);
    assert_eq!(after.tasks[0].original_line, "!ship relay-v2");
    assert!(search("relay-v2").note_ids.contains(&saved.id));
}

#[test]
fn qualified_filters_resolve_distinct_canonical_reference_identities() {
    let dir = TempDir::new("filters");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    let alpha = store
        .register_project(NewV2Project {
            name: "Alpha".to_string(),
            slug: "alpha".to_string(),
            repo_path: None,
            owner_repo: None,
            t3_target: None,
        })
        .unwrap();
    let beta = store
        .register_project(NewV2Project {
            name: "Beta".to_string(),
            slug: "beta".to_string(),
            repo_path: None,
            owner_repo: None,
            t3_target: None,
        })
        .unwrap();
    assert_eq!(alpha.slug, "alpha");

    // Two persisted kinds share a target id but remain distinct canonical identities.
    let a = store
        .save_note(create(
            "a",
            vec![plain(
                "the alpha note +alpha",
                vec![reference("github", "shared-id")],
            )],
        ))
        .unwrap();
    let b = store
        .save_note(create(
            "b",
            vec![plain(
                "the beta note +beta",
                vec![PreparedReference {
                    snapshot_json: None,
                    ..reference("clickup", "shared-id")
                }],
            )],
        ))
        .unwrap();

    let (rows, kinds): (i64, i64) = store
        .conn()
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT kind) FROM ref_targets WHERE target_id = 'shared-id'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rows, 2);
    assert_eq!(kinds, 2);

    let search = |query: &str| store.search_workspace(query).unwrap();
    let by_ref = search("@shared-id");
    assert!(by_ref.note_ids.contains(&a.id));
    assert!(by_ref.note_ids.contains(&b.id));
    assert!(search("+ALPHA").note_ids.contains(&a.id));
    assert!(!search("+beta").note_ids.contains(&a.id));
    assert!(search("+alpha @shared-id").note_ids.contains(&a.id));
    assert!(!search("+alpha @shared-id").note_ids.contains(&b.id));
    assert!(search("+missing").note_ids.is_empty());
    assert!(search("@missing").note_ids.is_empty());
    assert_eq!(search("").note_ids.len(), 2);

    // A Task in a named project is found by the flat project filter too.
    let task_note = store
        .save_note(create(
            "task",
            vec![directive("!do work", AgentMode::Act, Some(&beta.id))],
        ))
        .unwrap();
    assert!(search("+beta").task_ids.contains(&task_note.tasks[0].id));
}

/// Test-only fixture writer: set the stored lifecycle status directly, the way a later
/// producer would, so every status round-trips through the real SQLite read path.
fn force_status(store: &V2Store, task_id: &str, status: &str) {
    let at = crate::model::now();
    store
        .conn()
        .execute(
            "UPDATE tasks SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![task_id, status, at],
        )
        .unwrap();
}

#[test]
fn stored_progress_round_trips_every_status_and_the_grill_turn_flip() {
    let dir = TempDir::new("progress");
    let path = dir.file("relay-v2.db");
    let store = V2Store::open(&path).unwrap();
    assert_eq!(
        {
            let v: i64 = store
                .conn()
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            v
        },
        USER_VERSION
    );

    let statuses = [
        "briefing",
        "grilling",
        "needs_project",
        "awaiting_approval",
        "dispatched",
        "needs_answer",
        "returned",
        "failed",
        "done",
        "cancelled",
    ];
    let note = store
        .save_note(create(
            "all-statuses",
            (0..statuses.len())
                .map(|i| directive(&format!("!task {i}"), AgentMode::Act, None))
                .collect(),
        ))
        .unwrap();
    let ids: Vec<String> = note.tasks.iter().map(|t| t.id.clone()).collect();
    for (id, status) in ids.iter().zip(statuses) {
        force_status(&store, id, status);
    }

    // A Grill Task is created on the agent's turn and stays that way until a producer
    // writes otherwise. All progress fields are optional and start unset.
    let grill = store
        .save_note(create(
            "grill",
            vec![directive("??clarify", AgentMode::Grill, None)],
        ))
        .unwrap()
        .tasks
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(grill.status, TaskStatus::Grilling);
    assert_eq!(grill.grill_turn, Some(GrillTurn::Agent));
    assert_eq!(grill.pending_action, None);
    assert_eq!(grill.thread_url, None);

    let waiting = store
        .update_task_progress(
            &grill.id,
            TaskProgressUpdate {
                grill_turn: Some(GrillTurn::UserQuestion),
                pending_action: Some("Answer the question in the thread".into()),
                ..TaskProgressUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(waiting.grill_turn, Some(GrillTurn::UserQuestion));
    assert_eq!(waiting.status, TaskStatus::Grilling);
    assert_eq!(
        waiting.pending_action.as_deref(),
        Some("Answer the question in the thread")
    );
    assert!(waiting.updated_at >= grill.updated_at);

    let failed = store
        .update_task_progress(
            &ids[7],
            TaskProgressUpdate {
                failure_stage: Some("dispatch".into()),
                failure_reason: Some("T3 is not running".into()),
                thread_url: Some("http://localhost/thread/1".into()),
                ..TaskProgressUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(failed.failure_stage.as_deref(), Some("dispatch"));
    assert_eq!(failed.failure_reason.as_deref(), Some("T3 is not running"));
    assert_eq!(
        failed.thread_url.as_deref(),
        Some("http://localhost/thread/1")
    );

    // Reopen: every field and every status survives a real file round-trip.
    drop(store);
    let reopened = V2Store::open(&path).unwrap();
    let workspace = reopened.workspace_snapshot().unwrap();
    assert_eq!(workspace.tasks.len(), statuses.len() + 1);
    let by_id: std::collections::HashMap<_, _> = workspace
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();
    for (id, status) in ids.iter().zip(statuses) {
        assert_eq!(by_id[id.as_str()].status.as_str(), status);
    }
    assert_eq!(
        by_id[grill.id.as_str()].grill_turn,
        Some(GrillTurn::UserQuestion)
    );
    assert_eq!(
        by_id[ids[7].as_str()].failure_stage.as_deref(),
        Some("dispatch")
    );

    // The fixture harness flips the turn back to the agent; the status does not change.
    let back = reopened
        .update_task_progress(
            &grill.id,
            TaskProgressUpdate {
                grill_turn: Some(GrillTurn::Agent),
                pending_action: None,
                ..TaskProgressUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(back.grill_turn, Some(GrillTurn::Agent));
    assert_eq!(back.status, TaskStatus::Grilling);
    assert_eq!(back.pending_action, None);

    // IPC JSON keeps the documented camelCase contract.
    let json = serde_json::to_string(&back).unwrap();
    assert!(json.contains("\"grillTurn\":\"agent\""));
    assert!(json.contains("\"threadUrl\""));
    assert!(json.contains("\"status\":\"grilling\""));
}

#[test]
fn a_version_one_v2_file_gains_the_progress_columns() {
    let dir = TempDir::new("migrate");
    let path = dir.file("relay-v2.db");
    // Build the 0001 schema only, using the real migration file.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../../migrations/v2/0001_init.sql"))
        .unwrap();
    conn.execute(
        "INSERT INTO settings (id, import_clickup_enabled, import_github_enabled, import_scope, updated_at)
         VALUES (1, 0, 0, NULL, '2026-11-21T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.pragma_update(None, "application_id", super::store::APPLICATION_ID)
        .unwrap();
    conn.pragma_update(None, "user_version", 1_i64).unwrap();
    drop(conn);

    let store = V2Store::open(&path).unwrap();
    let columns: Vec<String> = store
        .conn()
        .prepare("PRAGMA table_info(tasks)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    for column in [
        "grill_turn",
        "pending_action",
        "failure_stage",
        "failure_reason",
        "thread_url",
    ] {
        assert!(
            columns.iter().any(|name| name == column),
            "missing {column}"
        );
    }
    assert_eq!(
        store.notebook_snapshot().unwrap().settings.updated_at,
        "2026-11-21T00:00:00Z"
    );
}
