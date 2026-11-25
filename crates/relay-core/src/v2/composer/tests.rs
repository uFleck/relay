//! Parser and submission tests. File-backed cases use throwaway directories; no provider
//! setup, network or live profile is involved.

use std::{collections::HashSet, fs, path::PathBuf};

use rusqlite::params;

use super::*;
use crate::model::new_id;
use crate::v2::NewV2Project;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("relay-composer-{tag}-{}", new_id()));
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

fn store_at(tag: &str) -> (TempDir, V2Store) {
    let dir = TempDir::new(tag);
    let store = V2Store::open(dir.file("relay-v2.db")).unwrap();
    (dir, store)
}

fn create(key: &str, text: &str) -> ComposerSaveRequest {
    ComposerSaveRequest::Create {
        submission_key: key.to_string(),
        text: text.to_string(),
    }
}

fn edit(note_id: &str, revision: i64, lines: Vec<(Option<String>, &str)>) -> ComposerSaveRequest {
    ComposerSaveRequest::Edit {
        note_id: note_id.to_string(),
        expected_revision: revision,
        lines: lines
            .into_iter()
            .map(|(line_id, text)| ComposerLineInput {
                line_id,
                text: text.to_string(),
            })
            .collect(),
    }
}

fn project(store: &V2Store, name: &str, slug: &str, owner_repo: &str) -> String {
    store
        .register_project(NewV2Project {
            name: name.to_string(),
            slug: slug.to_string(),
            repo_path: None,
            owner_repo: Some(owner_repo.to_string()),
            t3_target: None,
        })
        .unwrap()
        .id
}

#[test]
fn ipc_request_json_matches_the_typescript_contract() {
    let create: ComposerSaveRequest = serde_json::from_str(
        r#"{"kind":"create","submissionKey":"key-1","text":"plain\r\n! do it"}"#,
    )
    .unwrap();
    assert_eq!(
        create,
        ComposerSaveRequest::Create {
            submission_key: "key-1".to_string(),
            text: "plain\r\n! do it".to_string(),
        }
    );

    let edit: ComposerSaveRequest = serde_json::from_str(
        r#"{"kind":"edit","noteId":"note-1","expectedRevision":4,
            "lines":[{"lineId":null,"text":"new"},{"lineId":"line-1","text":"kept"}]}"#,
    )
    .unwrap();
    assert_eq!(
        edit,
        ComposerSaveRequest::Edit {
            note_id: "note-1".to_string(),
            expected_revision: 4,
            lines: vec![
                ComposerLineInput {
                    line_id: None,
                    text: "new".to_string()
                },
                ComposerLineInput {
                    line_id: Some("line-1".to_string()),
                    text: "kept".to_string()
                },
            ],
        }
    );
}

#[test]
fn plain_only_submission_and_mixed_newlines_preserve_each_line() {
    let (_dir, store) = store_at("plain");
    let saved = save_submission(
        &store,
        create("plain-1", "Alpha line\r\nBeta line\nGamma line\rDelta tail"),
    )
    .unwrap();

    assert_eq!(saved.source, "composer");
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.tasks.len(), 0);
    assert_eq!(saved.lines.len(), 4);
    assert_eq!(
        saved
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Alpha line", "Beta line", "Gamma line", "Delta tail"]
    );
    assert_eq!(saved.body, "Alpha line\nBeta line\nGamma line\nDelta tail");
}

#[test]
fn sigils_match_at_column_one_with_the_longest_prefix() {
    let cases = [
        ("! Check", Some(AgentMode::Act)),
        ("!Check", Some(AgentMode::Act)),
        ("? Review", Some(AgentMode::Wait)),
        ("?Review", Some(AgentMode::Wait)),
        ("?? Explore", Some(AgentMode::Grill)),
        ("??Explore", Some(AgentMode::Grill)),
        (" ! Check", None),
        ("\t! Check", None),
        ("x! Check", None),
        ("plain", None),
    ];
    for (text, mode) in cases {
        assert_eq!(parse_line(text).agent_mode, mode, "line: {text}");
    }
}

#[test]
fn duplicate_line_text_keeps_distinct_ids_and_stable_order() {
    let (_dir, store) = store_at("duplicate");
    let saved = save_submission(&store, create("dup", "same\nsame\nsame")).unwrap();
    let ids: Vec<&str> = saved.lines.iter().map(|line| line.id.as_str()).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().collect::<HashSet<_>>().len() == 3);
    assert_eq!(saved.body, "same\nsame\nsame");
    assert_eq!(
        saved
            .lines
            .iter()
            .map(|line| line.position)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)]
    );
}

#[test]
fn references_are_ordered_per_line_and_adjacent_lines_stay_independent() {
    let (_dir, store) = store_at("refs");
    let saved = save_submission(
        &store,
        create(
            "refs-1",
            "Capture a follow-up\r\n\
             ! Check https://app.clickup.com/t/demo123 https://github.com/example-org/alpha/pull/7 https://example.slack.com/archives/C01/p1731000000000100\n\
             ? Review https://github.com/example-org/beta/issues/8\n\
             ?? Explore https://app.clickup.com/t/other9",
        ),
    )
    .unwrap();

    assert_eq!(saved.body, "Capture a follow-up");
    let plain = &saved.lines[0];
    assert!(plain.references.is_empty());
    let first = &saved.lines[1];
    assert_eq!(
        first
            .references
            .iter()
            .map(|reference| (reference.kind.as_str(), reference.target_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("clickup", "CU-demo123"),
            ("github", "example-org/alpha#7"),
            ("slack", "C01/1731000000.000100"),
        ]
    );
    for reference in &first.references {
        assert_eq!(reference.snapshot_json, None);
    }
    assert_eq!(
        first.references[1].canonical_url,
        "https://github.com/example-org/alpha/pull/7"
    );

    assert_eq!(saved.tasks.len(), 3);
    assert_eq!(
        saved
            .tasks
            .iter()
            .map(|task| task.agent_mode)
            .collect::<Vec<_>>(),
        vec![AgentMode::Act, AgentMode::Wait, AgentMode::Grill]
    );
    assert_eq!(
        saved.tasks[0].references, first.references,
        "a Task copies its own line's References in order"
    );
    assert_eq!(
        saved.tasks[1]
            .references
            .iter()
            .map(|reference| reference.target_id.as_str())
            .collect::<Vec<_>>(),
        vec!["example-org/beta#8"]
    );
    assert_eq!(
        saved.tasks[2]
            .references
            .iter()
            .map(|reference| reference.target_id.as_str())
            .collect::<Vec<_>>(),
        vec!["CU-other9"]
    );
}

#[test]
fn provider_free_save_succeeds_and_blank_create_is_rejected() {
    let (_dir, store) = store_at("provider-free");
    assert!(store.notebook_snapshot().unwrap().projects.is_empty());
    let saved = save_submission(&store, create("ok", "just text")).unwrap();
    assert_eq!(saved.lines[0].text, "just text");
    for bad in ["", "   ", "\r\n \t \n"] {
        assert!(matches!(
            save_submission(&store, create("blank", bad)),
            Err(V2Error::InvalidInput(_))
        ));
    }
}

#[test]
fn explicit_project_wins_even_when_unknown_and_beats_a_registered_repo() {
    let (_dir, store) = store_at("explicit");
    let alpha = project(&store, "Alpha", "alpha", "example-org/alpha");
    let beta = project(&store, "Beta", "beta", "example-org/beta");

    let explicit = save_submission(
        &store,
        create(
            "explicit-1",
            "! Check https://github.com/example-org/alpha/pull/7 +BeTa",
        ),
    )
    .unwrap();
    assert_eq!(explicit.tasks[0].project_id.as_deref(), Some(beta.as_str()));

    let unknown = save_submission(
        &store,
        create(
            "explicit-2",
            "! Check https://github.com/example-org/alpha/pull/7 +unknown",
        ),
    )
    .unwrap();
    assert_eq!(unknown.tasks[0].project_id, None);

    let _ = alpha;
}

#[test]
fn only_the_first_github_repo_infers_a_project() {
    let (_dir, store) = store_at("first-repo");
    let beta = project(&store, "Beta", "beta", "example-org/beta");

    let unmatched = save_submission(
        &store,
        create(
            "unmatched",
            "! Review https://github.com/example-org/unknown/issues/8 https://github.com/example-org/beta/pull/9",
        ),
    )
    .unwrap();
    assert_eq!(unmatched.tasks[0].project_id, None);

    let matched = save_submission(
        &store,
        create(
            "matched",
            "? Check https://github.com/example-org/beta/issues/8",
        ),
    )
    .unwrap();
    assert_eq!(matched.tasks[0].project_id.as_deref(), Some(beta.as_str()));
}

#[test]
fn promotion_and_reedits_keep_the_original_task_brief_and_id() {
    let (_dir, store) = store_at("promotion");
    let saved = save_submission(
        &store,
        create(
            "promote",
            "Capture a follow-up\n! Check https://github.com/example-org/alpha/pull/7",
        ),
    )
    .unwrap();
    let plain_id = saved.lines[0].id.clone();
    let source_id = saved.lines[1].id.clone();
    let original = saved.tasks[0].clone();
    assert_eq!(
        original.original_line,
        "! Check https://github.com/example-org/alpha/pull/7"
    );
    assert!(original.project_id.is_none());

    // A seeded Brief stands in for the Briefing stage that a later issue owns.
    store
        .conn()
        .execute(
            "UPDATE tasks SET brief = ?1 WHERE id = ?2",
            params!["goal: check the PR", original.id.as_str()],
        )
        .unwrap();

    // Promote the plain line: same line id, now with Act intent.
    let promoted = save_submission(
        &store,
        edit(
            &saved.id,
            saved.revision,
            vec![
                (Some(plain_id.clone()), "! Capture a follow-up"),
                (
                    Some(source_id.clone()),
                    "! Check https://github.com/example-org/alpha/pull/7",
                ),
            ],
        ),
    )
    .unwrap();
    assert_eq!(promoted.tasks.len(), 2);

    // Reorder, change the original sigil to Wait, then remove it entirely. The Task's id,
    // original line, mode, References and Brief never change.
    let reordered = save_submission(
        &store,
        edit(
            &saved.id,
            promoted.revision,
            vec![
                (
                    Some(source_id.clone()),
                    "? Check https://github.com/example-org/alpha/pull/7",
                ),
                (Some(plain_id.clone()), "! Capture a follow-up"),
            ],
        ),
    )
    .unwrap();
    let unsigiled = save_submission(
        &store,
        edit(
            &saved.id,
            reordered.revision,
            vec![
                (
                    Some(source_id.clone()),
                    "Check https://github.com/example-org/alpha/pull/7",
                ),
                (Some(plain_id.clone()), "! Capture a follow-up"),
            ],
        ),
    )
    .unwrap();

    // Remove the source line. Its Task stays addressable with a null position.
    let removed = save_submission(
        &store,
        edit(
            &saved.id,
            unsigiled.revision,
            vec![(Some(plain_id.clone()), "! Capture a follow-up")],
        ),
    )
    .unwrap();

    let tasks: Vec<_> = removed
        .tasks
        .iter()
        .filter(|task| task.id == original.id)
        .collect();
    assert_eq!(tasks.len(), 1, "no duplicate Task was created");
    let kept = tasks[0];
    assert_eq!(kept.original_line, original.original_line);
    assert_eq!(kept.agent_mode, AgentMode::Act);
    assert_eq!(kept.project_id, None);
    assert_eq!(kept.brief.as_deref(), Some("goal: check the PR"));
    assert_eq!(kept.references, original.references);
    assert_eq!(kept.source_line_id, source_id);
    assert_eq!(removed.tasks.len(), 2);
    assert_eq!(removed.lines.len(), 1);
    assert_eq!(removed.lines[0].id, plain_id);
}

#[test]
fn stale_revision_and_create_replay_are_guarded_against_sqlite() {
    let (dir, store) = store_at("sqlite");
    let first = save_submission(&store, create("replay", "original text")).unwrap();

    // A replay of the same submission key returns the original aggregate, not a new Note.
    let replay = save_submission(&store, create("replay", "different text")).unwrap();
    assert_eq!(replay, first);

    assert!(matches!(
        save_submission(
            &store,
            edit(&first.id, first.revision + 9, vec![(None, "x")])
        ),
        Err(V2Error::StaleRevision { .. })
    ));
    assert_eq!(
        store.notebook_snapshot().unwrap().notes[0].revision,
        first.revision
    );

    let updated = save_submission(
        &store,
        edit(
            &first.id,
            first.revision,
            vec![(Some(first.lines[0].id.clone()), "edited text")],
        ),
    )
    .unwrap();
    assert_eq!(updated.revision, first.revision + 1);

    drop(store);
    let reopened = V2Store::open(dir.file("relay-v2.db")).unwrap();
    assert_eq!(reopened.notebook_snapshot().unwrap().notes, vec![updated]);
}

#[test]
fn typed_chips_and_urls_become_line_references_and_infer_projects() {
    let (_dir, store) = store_at("chips");
    let project_id = project(&store, "Sample", "sample", "sample/app");
    let saved = save_submission(
        &store,
        create("chips", "!ship @CU-8x1 and @app#142\nplain @N-12"),
    )
    .unwrap();
    assert_eq!(saved.lines.len(), 2);
    let first = &saved.lines[0];
    assert_eq!(first.references.len(), 2);
    assert_eq!(first.references[0].target_id, "CU-8x1");
    assert_eq!(first.references[1].target_id, "sample/app#142");
    assert_eq!(
        first.references[1].canonical_url,
        "https://github.com/sample/app/pull/142"
    );
    let task = &saved.tasks[0];
    assert_eq!(task.references.len(), 2);
    assert_eq!(task.project_id.as_deref(), Some(project_id.as_str()));
    let second = &saved.lines[1];
    assert_eq!(second.references.len(), 1);
    assert_eq!(second.references[0].target_id, "12");
}

#[test]
fn ambiguous_short_repo_chip_resolves_to_no_reference() {
    let (_dir, store) = store_at("ambiguous-chip");
    project(&store, "Alpha", "alpha", "alpha/app");
    project(&store, "Beta", "beta", "beta/app");
    let saved = save_submission(&store, create("ambiguous", "!ship @app#1")).unwrap();
    assert!(saved.lines[0].references.is_empty());
}

fn capture(key: &str, text: &str) -> ComposerSaveRequest {
    ComposerSaveRequest::Capture {
        submission_key: key.to_string(),
        text: text.to_string(),
    }
}

#[test]
fn append_target_reads_a_leading_note_or_task_reference() {
    assert_eq!(
        append_target("@N-01AB ! do x"),
        Some((AppendTarget::Note("01AB".into()), "! do x".into()))
    );
    assert_eq!(
        append_target("  @T-9\r\n? ask\nplain  \n"),
        Some((AppendTarget::Task("9".into()), "? ask\nplain".into()))
    );
    assert_eq!(append_target("see @N-1 later"), None);
    assert_eq!(append_target("@CU-123 card"), None);
    assert_eq!(append_target("plain note"), None);
}

#[test]
fn capture_with_a_note_reference_appends_lines_and_tasks_to_that_note() {
    let (_dir, store) = store_at("append-note");
    let meeting = save_submission(&store, create("m", "Meeting notes\n! first follow-up")).unwrap();

    let appended =
        save_submission(&store, capture("c1", &format!("@N-{} ! do x", meeting.id))).unwrap();

    assert_eq!(appended.id, meeting.id);
    assert_eq!(appended.revision, meeting.revision + 1);
    let texts: Vec<_> = appended.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, ["Meeting notes", "! first follow-up", "! do x"]);
    assert_eq!(appended.lines[0].id, meeting.lines[0].id);
    assert_eq!(appended.tasks.len(), 2);
    assert_eq!(store.notebook_snapshot().unwrap().notes.len(), 1);
}

#[test]
fn capture_with_a_task_reference_appends_to_the_task_note() {
    let (_dir, store) = store_at("append-task");
    let meeting = save_submission(&store, create("m", "! ship it")).unwrap();
    let task = meeting.tasks[0].id.clone();

    let appended = save_submission(&store, capture("c1", &format!("@T-{task}\nremember the docs"))).unwrap();

    assert_eq!(appended.id, meeting.id);
    assert_eq!(appended.lines.last().unwrap().text, "remember the docs");
}

#[test]
fn capture_without_a_leading_reference_creates_a_note() {
    let (_dir, store) = store_at("capture-create");
    let first = save_submission(&store, create("m", "older")).unwrap();

    let created = save_submission(&store, capture("c1", &format!("new thing about @N-{}", first.id))).unwrap();

    assert_ne!(created.id, first.id);
    assert_eq!(store.notebook_snapshot().unwrap().notes.len(), 2);
}

#[test]
fn capture_refuses_an_empty_append_and_an_unknown_note() {
    let (_dir, store) = store_at("capture-refuse");
    let note = save_submission(&store, create("m", "text")).unwrap();

    assert!(matches!(
        save_submission(&store, capture("c1", &format!("@N-{}   ", note.id))),
        Err(V2Error::InvalidInput(_))
    ));
    assert!(save_submission(&store, capture("c2", "@N-01MISSING add this")).is_err());
    assert_eq!(store.note(&note.id).unwrap().revision, note.revision);
}
