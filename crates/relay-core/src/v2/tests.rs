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

#[test]
fn t3_target_accepts_ids_and_absolute_paths_only() {
    assert_eq!(
        T3Target::parse("id", " 01JABCDEFGHJKLMNPQRSTVWXYZ ").unwrap(),
        T3Target::Id("01JABCDEFGHJKLMNPQRSTVWXYZ".into())
    );
    assert_eq!(
        T3Target::parse("path", r"  C:\Relay Smoke\sample-app  ").unwrap(),
        T3Target::Path(r"C:\Relay Smoke\sample-app".into())
    );
    assert_eq!(
        T3Target::parse("path", r"\\server\share\app").unwrap(),
        T3Target::Path(r"\\server\share\app".into())
    );
    assert!(T3Target::parse("path", "relative\\app").is_err());
    assert!(T3Target::parse("id", "has space").is_err());
    assert!(T3Target::parse("branch", "main").is_err());
    assert!(T3Target::parse("id", "  ").is_err());
    for target in [
        T3Target::Id("01JABCDEFGHJKLMNPQRSTVWXYZ".into()),
        T3Target::Path(r"C:\Relay Smoke\sample-app".into()),
    ] {
        assert_eq!(T3Target::from_storage(&target.to_storage()), Some(target));
    }
}

#[test]
fn settings_and_t3_mapping_persist_across_reopen() {
    let dir = TempDir::new("settings");
    let path = dir.file("relay-v2.db");
    let store = V2Store::open(&path).unwrap();
    let defaults = store.settings().unwrap();
    assert!(!defaults.import_clickup_enabled);
    assert!(!defaults.import_github_enabled);
    assert_eq!(defaults.import_scope, None);
    assert_eq!(defaults.opencode_model, None);

    let scope = V2ImportScope {
        workspace_id: "ws-1".into(),
        space_id: Some("space-2".into()),
        project_id: "project-id".into(),
    };
    store
        .update_settings(V2SettingsUpdate {
            import_clickup_enabled: true,
            import_github_enabled: false,
            import_scope: Some(scope.to_json()),
        })
        .unwrap();
    let saved = store
        .set_opencode_model(Some(" opencode/gpt-5-nano ".into()))
        .unwrap();
    assert_eq!(saved.opencode_model.as_deref(), Some("opencode/gpt-5-nano"));
    assert_eq!(
        V2ImportScope::from_json(saved.import_scope.as_deref().unwrap()),
        Some(scope)
    );

    let project = store
        .register_project(NewV2Project {
            name: "Sample App".into(),
            slug: "sample-app".into(),
            repo_path: Some(r"C:\Projects\sample-app".into()),
            owner_repo: Some("sample/sample-app".into()),
            t3_target: None,
        })
        .unwrap();
    assert_eq!(store.t3_target(&project.id).unwrap(), None);

    let target = T3Target::parse("path", r"C:\Relay Smoke\sample-app").unwrap();
    let saved = store
        .set_t3_target(&project.id, Some(target.clone()))
        .unwrap();
    assert_eq!(
        saved.t3_target.as_deref(),
        Some("path:C:\\Relay Smoke\\sample-app")
    );
    assert_ne!(saved.repo_path, saved.t3_target);
    drop(store);

    let reopened = V2Store::open(&path).unwrap();
    assert_eq!(reopened.t3_target(&project.id).unwrap(), Some(target));
    assert_eq!(
        reopened.settings().unwrap().opencode_model.as_deref(),
        Some("opencode/gpt-5-nano")
    );
    // Clearing is a persistable state, distinct from a checkout path.
    reopened.set_t3_target(&project.id, None).unwrap();
    drop(reopened);
    let reopened = V2Store::open(&path).unwrap();
    assert_eq!(reopened.t3_target(&project.id).unwrap(), None);
    assert!(reopened.settings().unwrap().import_clickup_enabled);
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

// --- Prerequisite integration: the two parallel `0002` layouts -----------------

/// Build a real `user_version = 2` file from one branch of the parallel v2 work.
/// `second_migration` is whichever `0002` that branch shipped; data is seeded so an
/// upgrade must preserve it rather than starting over.
fn version_two_fixture(
    tag: &str,
    second_migration: &str,
    with_opencode: bool,
) -> (TempDir, PathBuf) {
    let dir = TempDir::new(tag);
    let path = dir.file("relay-v2.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../../migrations/v2/0001_init.sql"))
        .unwrap();
    conn.execute_batch(second_migration).unwrap();
    let at = "2026-11-21T00:00:00Z";
    if with_opencode {
        conn.execute(
            "INSERT INTO settings (id, import_clickup_enabled, import_github_enabled, import_scope, opencode_model, updated_at)
             VALUES (1, 1, 0, NULL, 'opencode/gpt-5-nano', ?1)",
            params![at],
        )
        .unwrap();
    } else {
        conn.execute(
            "INSERT INTO settings (id, import_clickup_enabled, import_github_enabled, import_scope, updated_at)
             VALUES (1, 0, 1, NULL, ?1)",
            params![at],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO notes (id, rev, source, body, created_at, updated_at)
         VALUES ('note-1', 1, 'composer', '!do work', ?1, ?1)",
        params![at],
    )
    .unwrap();
    for (id, mode) in [("act-task", "act"), ("grill-task", "grill")] {
        let line_id = format!("line-{id}");
        conn.execute(
            "INSERT INTO note_lines (id, note_id, text, position, created_at, updated_at)
             VALUES (?1, 'note-1', '!do work', NULL, ?2, ?2)",
            params![line_id, at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, source_line_id, original_line, agent_mode, status, created_at, updated_at)
             VALUES (?1, ?2, '!do work', ?3, 'briefing', ?4, ?4)",
            params![id, line_id, mode, at],
        )
        .unwrap();
    }
    if !with_opencode {
        // #27's store sets grill_turn at task creation; mimic that for its on-disk shape.
        conn.execute(
            "UPDATE tasks SET grill_turn = 'agent' WHERE agent_mode = 'grill'",
            [],
        )
        .unwrap();
    }
    conn.pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    conn.pragma_update(None, "user_version", 2_i64).unwrap();
    drop(conn);
    (dir, path)
}

fn has_column(store: &V2Store, table: &str, column: &str) -> bool {
    let conn = store.conn();
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|row| row.unwrap())
        .any(|name| name == column)
}

#[test]
fn a_task_progress_version_two_file_gains_provider_settings_without_losing_data() {
    let (_dir, path) = version_two_fixture(
        "v2-progress-upgrade",
        include_str!("../../migrations/v2/0002_task_progress.sql"),
        false,
    );
    let store = V2Store::open(&path).unwrap();
    assert!(has_column(&store, "settings", "opencode_model"));
    assert!(has_column(&store, "tasks", "grill_turn"));
    let version: i64 = store
        .conn()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, USER_VERSION);
    let settings = store.settings().unwrap();
    assert_eq!(settings.opencode_model, None);
    assert!(settings.import_github_enabled);
    assert_eq!(settings.updated_at, "2026-11-21T00:00:00Z");
    let status: String = store
        .conn()
        .query_row(
            "SELECT status FROM tasks WHERE id = 'act-task'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "briefing");
    let grill: Option<String> = store
        .conn()
        .query_row(
            "SELECT grill_turn FROM tasks WHERE id = 'grill-task'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(grill.as_deref(), Some("agent"));
}

#[test]
fn a_provider_settings_version_two_file_gains_task_progress_without_losing_data() {
    let (_dir, path) = version_two_fixture(
        "v2-provider-upgrade",
        include_str!("../../migrations/v2/0003_provider_settings.sql"),
        true,
    );
    let store = V2Store::open(&path).unwrap();
    assert!(has_column(&store, "tasks", "grill_turn"));
    assert!(has_column(&store, "tasks", "thread_url"));
    assert!(has_column(&store, "settings", "opencode_model"));
    let version: i64 = store
        .conn()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, USER_VERSION);
    assert_eq!(
        store.settings().unwrap().opencode_model.as_deref(),
        Some("opencode/gpt-5-nano")
    );
    assert!(store.settings().unwrap().import_clickup_enabled);
    let grill: Option<String> = store
        .conn()
        .query_row(
            "SELECT grill_turn FROM tasks WHERE id = 'grill-task'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(grill.as_deref(), Some("agent"));
}

// --- #29: local picker query ------------------------------------------------------

fn snapshot(title: &str, extra: &str) -> String {
    if extra.is_empty() {
        format!(r#"{{"title":"{title}"}}"#)
    } else {
        format!(r#"{{"title":"{title}",{extra}}}"#)
    }
}

fn ref_with(kind: &str, target: &str, snap: Option<&str>) -> PreparedReference {
    PreparedReference {
        kind: kind.to_string(),
        target_id: target.to_string(),
        canonical_url: format!("https://example.invalid/{kind}/{target}"),
        snapshot_json: snap.map(str::to_string),
    }
}

#[test]
fn picker_groups_in_order_and_deduplicates_by_key() {
    let dir = TempDir::new("picker-groups");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    let clickup = ref_with("clickup", "CU-8x1", Some(&snapshot("Ship chips", "")));
    let pr = ref_with(
        "github",
        "sample/app#142",
        Some(&snapshot("Add picker", "")),
    );
    let slack = ref_with(
        "slack",
        "C1/1731000000.000001",
        Some(&snapshot("Thread", "")),
    );
    store
        .save_note(create(
            "picker",
            vec![
                plain(
                    "see https://example.com/x",
                    vec![clickup.clone(), pr.clone(), slack],
                ),
                plain("again https://app.clickup.com/t/8x1", vec![clickup]),
                directive("!do the thing", AgentMode::Act, None),
            ],
        ))
        .unwrap();

    let rows = store.reference_picker("@").unwrap();
    let keys: Vec<&str> = rows.iter().map(|row| row.key.as_str()).collect();
    let unique: std::collections::HashSet<&str> = keys.iter().copied().collect();
    assert_eq!(unique.len(), keys.len(), "picker must deduplicate by key");
    let ranks: Vec<u8> = rows.iter().map(|row| row.group.rank()).collect();
    assert!(
        ranks.windows(2).all(|w| w[0] <= w[1]),
        "groups out of order: {ranks:?}"
    );
    assert!(keys.contains(&"clickup:8x1"));
    assert!(keys.contains(&"github:sample/app#142"));
    assert!(keys.contains(&"slack:C1/1731000000.000001"));
    assert!(keys.iter().any(|key| key.starts_with("note:")));
    assert!(keys.iter().any(|key| key.starts_with("task:")));
}

#[test]
fn picker_scopes_narrow_to_cards_issues_and_local_rows() {
    let dir = TempDir::new("picker-scopes");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    store
        .save_note(create(
            "scopes",
            vec![plain(
                "refs",
                vec![
                    ref_with("clickup", "CU-42", None),
                    ref_with("github", "sample/app#142", None),
                    ref_with("github", "sample/app#7", None),
                ],
            )],
        ))
        .unwrap();

    let prs = store.reference_picker("#142").unwrap();
    assert!(
        prs.iter()
            .all(|row| { row.group == V2PickerGroup::Prs || row.group == V2PickerGroup::Issues })
    );
    assert!(prs.iter().any(|row| row.key == "github:sample/app#142"));
    let cards = store.reference_picker("CU-").unwrap();
    assert!(cards.iter().all(|row| row.group == V2PickerGroup::Cards));
    let local = store.reference_picker("N-").unwrap();
    assert!(
        local
            .iter()
            .all(|row| row.group == V2PickerGroup::NotesTasks)
    );
}

#[test]
fn picker_matches_titles_case_insensitively_and_exact_first() {
    let dir = TempDir::new("picker-title");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    store
        .save_note(create(
            "titles",
            vec![plain(
                "refs",
                vec![
                    ref_with(
                        "github",
                        "sample/app#1",
                        Some(&snapshot("Picker panel", "")),
                    ),
                    ref_with(
                        "github",
                        "sample/app#2",
                        Some(&snapshot("Refactor notes", "")),
                    ),
                ],
            )],
        ))
        .unwrap();
    let rows = store.reference_picker("picker").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "github:sample/app#1");
    let exact = store.reference_picker("@sample/app#1").unwrap();
    assert!(
        exact
            .iter()
            .any(|row| row.exact_match && row.key == "github:sample/app#1")
    );
}

#[test]
fn picker_qualifies_duplicate_short_repo_names_with_owner() {
    let dir = TempDir::new("picker-alias");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    store
        .register_project(NewV2Project {
            name: "Alpha app".into(),
            slug: "alpha-app".into(),
            repo_path: None,
            owner_repo: Some("alpha/app".into()),
            t3_target: None,
        })
        .unwrap();
    store
        .register_project(NewV2Project {
            name: "Beta app".into(),
            slug: "beta-app".into(),
            repo_path: None,
            owner_repo: Some("beta/app".into()),
            t3_target: None,
        })
        .unwrap();
    store
        .save_note(create(
            "aliases",
            vec![plain(
                "refs",
                vec![
                    ref_with("github", "alpha/app#1", None),
                    ref_with("github", "beta/app#1", None),
                ],
            )],
        ))
        .unwrap();
    let rows = store.reference_picker("app#1").unwrap();
    let displays: Vec<&str> = rows.iter().map(|row| row.display_id.as_str()).collect();
    assert!(displays.contains(&"@alpha#1"));
    assert!(displays.contains(&"@beta#1"));
}

#[test]
fn picker_unavailable_snapshot_keeps_identity_and_settings_destination() {
    let dir = TempDir::new("picker-unavailable");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    let snap = snapshot(
        "Known card",
        r#""reason":"signed out","content":"cached body","cachedAt":"2026-11-21T00:00:00Z""#,
    );
    store
        .save_note(create(
            "unavailable",
            vec![plain(
                "refs",
                vec![ref_with("clickup", "CU-known", Some(&snap))],
            )],
        ))
        .unwrap();
    let rows = store.reference_picker("@").unwrap();
    let row = rows.iter().find(|row| row.key == "clickup:known").unwrap();
    assert_eq!(row.availability, ReferenceAvailability::Unavailable);
    assert_eq!(row.unavailable_reason.as_deref(), Some("signed out"));
    assert_eq!(
        row.settings_destination.as_deref(),
        Some("Settings → ClickUp")
    );
    assert_eq!(
        row.snapshot.as_ref().unwrap().content.as_deref(),
        Some("cached body")
    );
    assert_eq!(
        row.snapshot.as_ref().unwrap().cached_at.as_deref(),
        Some("2026-11-21T00:00:00Z")
    );
}

#[test]
fn picker_known_typed_id_without_a_local_row_links_immediately() {
    let dir = TempDir::new("picker-known");
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    let rows = store.reference_picker("@CU-brand-new").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "clickup:brand-new");
    assert_eq!(rows[0].display_id, "@CU-brand-new");
}
