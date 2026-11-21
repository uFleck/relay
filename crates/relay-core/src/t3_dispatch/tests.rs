use std::sync::Arc;
use std::thread;

use crate::model::new_id;
use crate::v2::{
    AgentMode, NewV2Project, NoteWrite, PreparedLine, PreparedNoteWrite, PreparedTaskIntent,
    T3Target, V2Store, V2Task, approve, begin_attempt, complete_attempt, dispatch_readiness,
};

use super::*;

const OUTPUT: &str =
    r#"{"goal":"Ship it","decisions":["A"],"constraints":["B"],"openRisks":["C"]}"#;
const DATA_DIR: &str = r"C:\Users\tester\.t3";

fn ready_store(mode: AgentMode, selector: T3Target) -> (V2Store, V2Task, String) {
    let store = V2Store::open_in_memory().expect("store");
    let project = store
        .register_project(NewV2Project {
            name: "Sample".into(),
            slug: "sample".into(),
            repo_path: None,
            owner_repo: Some("sample/app".into()),
            t3_target: None,
        })
        .expect("project");
    store
        .set_t3_target(&project.id, Some(selector))
        .expect("mapping");
    let note = store
        .save_note(PreparedNoteWrite {
            write: NoteWrite::Create {
                submission_key: format!("dispatch-fixture-{}", new_id()),
            },
            source: "test".into(),
            lines: vec![PreparedLine {
                line_id: None,
                text: match mode {
                    AgentMode::Act => "! Draft release notes with `%PATH%` & quotes".into(),
                    AgentMode::Wait => "? Draft a migration checklist".into(),
                    AgentMode::Grill => "?? Draft a plan".into(),
                },
                references: vec![],
                task_intent: Some(PreparedTaskIntent {
                    agent_mode: mode,
                    project_id: Some(project.id.clone()),
                }),
            }],
        })
        .expect("note");
    let task = note.tasks[0].clone();
    let attempt = begin_attempt(&store, &task.id).unwrap().unwrap();
    complete_attempt(&store, &task.id, &attempt.attempt_id, OUTPUT).unwrap();
    if mode == AgentMode::Wait {
        approve(&store, &task.id, 1, None).unwrap();
    }
    assert!(dispatch_readiness(&store, &task.id).unwrap().is_some());
    (store, task, project.id)
}

fn start(task_id: &str, base_ref: Option<&str>) -> DispatchStart {
    DispatchStart {
        task_id: task_id.to_string(),
        data_dir: DATA_DIR.into(),
        base_ref: base_ref.map(str::to_string),
    }
}

fn identity(attempt: &DispatchAttempt) -> ThreadIdentity {
    ThreadIdentity {
        thread_id: format!("thread-{}", attempt.attempt_id),
        run_id: format!("run-{}", attempt.attempt_id),
        link: format!("https://app.t3.codes/threads/env/{}", attempt.attempt_id),
        project_id: "proj-42".to_string(),
    }
}

#[test]
fn claim_requires_readiness_and_freezes_the_exact_brief_and_inputs() {
    let (store, task, _) = ready_store(AgentMode::Act, T3Target::Id("proj-42".into()));
    let first = claim(&store, &start(&task.id, Some("main")))
        .unwrap()
        .expect("one claim");
    assert_eq!(first.attempt.state, DispatchState::Launching);
    assert_eq!(first.attempt.project_selector, "id:proj-42");
    assert_eq!(first.attempt.t3_data_dir, DATA_DIR);
    assert_eq!(first.attempt.base_ref.as_deref(), Some("main"));
    assert_eq!(
        first.attempt.title,
        format!("Relay Task {}", first.attempt.attempt_id)
    );
    assert!(first.attempt.frozen_line.contains("%PATH%"));
    assert_eq!(first.attempt.brief_revision, 1);
    assert!(first.brief.contains("Ship it"));
    // A second caller, before any result, must not get an attempt to start with.
    assert!(claim(&store, &start(&task.id, None)).unwrap().is_none());
}

#[test]
fn a_task_with_no_mapping_is_not_claimed() {
    let (store, task, project_id) = ready_store(AgentMode::Act, T3Target::Id("proj-42".into()));
    store.set_t3_target(&project_id, None).unwrap();
    assert!(claim(&store, &start(&task.id, None)).unwrap().is_none());
    assert!(store.latest_dispatch_attempt(&task.id).unwrap().is_none());
    // Readiness is still true: mapping the project later unlocks one start, with no stale attempt.
    assert!(dispatch_readiness(&store, &task.id).unwrap().is_some());
    store
        .set_t3_target(&project_id, Some(T3Target::Id("proj-42".into())))
        .unwrap();
    assert!(claim(&store, &start(&task.id, None)).unwrap().is_some());
}

#[test]
fn wait_needs_approval_before_an_attempt_can_be_claimed() {
    let (store, task, _) = ready_store(AgentMode::Wait, T3Target::Id("proj-42".into()));
    let mut brief = store.brief(&task.id).unwrap().unwrap();
    brief.status = crate::v2::BriefStatus::Ready;
    brief.wait_approval_revision = None;
    store.write_brief(&brief).unwrap();
    assert!(dispatch_readiness(&store, &task.id).unwrap().is_none());
    assert!(claim(&store, &start(&task.id, None)).unwrap().is_none());
    assert!(store.latest_dispatch_attempt(&task.id).unwrap().is_none());
    approve(&store, &task.id, brief.revision, None).unwrap();
    assert!(claim(&store, &start(&task.id, None)).unwrap().is_some());
}

#[test]
fn known_failure_preserves_the_brief_and_approval_and_allows_one_retry() {
    let (store, task, _) = ready_store(AgentMode::Wait, T3Target::Id("proj-42".into()));
    let first = claim(&store, &start(&task.id, None)).unwrap().unwrap();
    let failed = record_known_failure(&store, &first.attempt.attempt_id, "no running T3 server")
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, DispatchState::LaunchFailed);
    let after = store.workspace_snapshot().unwrap();
    let saved = after.tasks.iter().find(|item| item.id == task.id).unwrap();
    assert_eq!(saved.status, crate::v2::TaskStatus::Failed);
    assert!(
        saved
            .brief
            .as_deref()
            .is_some_and(|brief| brief.contains("Ship it"))
    );
    let brief = store.brief(&task.id).unwrap().unwrap();
    assert_eq!(brief.status, crate::v2::BriefStatus::Approved);
    assert_eq!(brief.wait_approval_revision, Some(1));
    let view = view(&store, &task.id).unwrap().unwrap();
    assert!(view.can_retry);
    assert!(!view.can_check_again);

    // Retry reserves a *new* attempt; the old failure no longer fences it.
    let second = retry(&store, &start(&task.id, None)).unwrap().unwrap();
    assert_ne!(second.attempt.attempt_id, first.attempt.attempt_id);
    assert!(retry(&store, &start(&task.id, None)).unwrap().is_none());
}

#[test]
fn uncertain_outcome_fences_retry_until_a_full_identity_is_committed() {
    let (store, task, _) = ready_store(AgentMode::Act, T3Target::Id("proj-42".into()));
    let claim = claim(&store, &start(&task.id, None)).unwrap().unwrap();
    record_uncertain(&store, &claim.attempt.attempt_id, "lost response").unwrap();
    // Repeated Retry must not start another thread while the first outcome is unknown.
    assert!(retry(&store, &start(&task.id, None)).unwrap().is_none());
    let view = view(&store, &task.id).unwrap().unwrap();
    assert!(view.can_check_again && view.can_link_existing && !view.can_retry);

    let partial = identity(&claim.attempt);
    save_thread_id(&store, &claim.attempt.attempt_id, &partial.thread_id)
        .unwrap()
        .unwrap();
    assert!(retry(&store, &start(&task.id, None)).unwrap().is_none());

    let mut incomplete = partial.clone();
    incomplete.run_id.clear();
    assert!(bind(&store, &claim.attempt.attempt_id, &incomplete).is_err());
    let mut wrong_project = partial.clone();
    wrong_project.project_id = "another-project".into();
    assert!(bind(&store, &claim.attempt.attempt_id, &wrong_project).is_err());

    let bound = bind(&store, &claim.attempt.attempt_id, &partial)
        .unwrap()
        .unwrap();
    assert_eq!(bound.state, DispatchState::Bound);
    let saved = store.latest_dispatch_attempt(&task.id).unwrap().unwrap();
    assert_eq!(saved.thread_url, Some(partial.link.clone()));
    let after = store.workspace_snapshot().unwrap();
    let saved_task = after.tasks.iter().find(|item| item.id == task.id).unwrap();
    assert_eq!(saved_task.status, crate::v2::TaskStatus::Dispatched);
    assert_eq!(saved_task.thread_url, Some(partial.link));
    assert!(retry(&store, &start(&task.id, None)).unwrap().is_none());
}

#[test]
fn reconciliation_adopts_only_one_exact_title_with_a_confirmed_identity() {
    let (store, task, _) = ready_store(AgentMode::Act, T3Target::Id("proj-42".into()));
    let claim = claim(&store, &start(&task.id, None)).unwrap().unwrap();
    let attempt = claim.attempt;
    let exact = ListedThread {
        thread_id: "thread-exact".into(),
        title: Some(attempt.title.clone()),
        project_id: Some("proj-42".into()),
    };
    let mut renamed = exact.clone();
    renamed.thread_id = "thread-renamed".into();
    renamed.title = Some("A totally different title".into());
    let mut wrong_project = exact.clone();
    wrong_project.thread_id = "thread-other".into();
    wrong_project.project_id = Some("other".into());
    assert_eq!(
        unique_candidate(&attempt, &[]).map(|c| c.thread_id.as_str()),
        None
    );
    assert_eq!(
        unique_candidate(&attempt, &[exact.clone(), renamed.clone()]).map(|c| c.thread_id.as_str()),
        Some("thread-exact")
    );
    assert!(unique_candidate(&attempt, &[exact.clone(), exact.clone()]).is_none());

    record_uncertain(&store, &attempt.attempt_id, "read error").unwrap();
    // A renamed title yields no candidate, so the Task stays fenced for manual reconciliation.
    assert!(unique_candidate(&attempt, &[renamed]).is_none());
    assert!(retry(&store, &start(&task.id, None)).unwrap().is_none());

    let confirmed = ThreadIdentity {
        thread_id: exact.thread_id.clone(),
        run_id: "run-exact".into(),
        link: "https://app.t3.codes/threads/env/thread-exact".into(),
        project_id: "proj-42".into(),
    };
    assert!(identity_matches(&attempt, &confirmed));
    let bound = bind(&store, &attempt.attempt_id, &confirmed)
        .unwrap()
        .unwrap();
    assert_eq!(bound.thread_id.as_deref(), Some("thread-exact"));
    let wrong_identity = ThreadIdentity {
        thread_id: wrong_project.thread_id,
        run_id: "run-other".into(),
        link: "https://app.t3.codes/threads/env/thread-other".into(),
        project_id: wrong_project.project_id.unwrap_or_default(),
    };
    assert!(!identity_matches(&attempt, &wrong_identity));
}

#[test]
fn manual_link_checks_profile_and_user_comparison() {
    let (store, task, _) = ready_store(AgentMode::Act, T3Target::Id("proj-42".into()));
    let claim = claim(&store, &start(&task.id, None)).unwrap().unwrap();
    record_uncertain(&store, &claim.attempt.attempt_id, "ambiguous").unwrap();
    let attempt = store.latest_dispatch_attempt(&task.id).unwrap().unwrap();
    let confirmed = identity(&attempt);
    assert!(link_existing(&store, &attempt, &confirmed, "id:proj-42", DATA_DIR, false).is_err());
    assert!(
        link_existing(
            &store,
            &attempt,
            &confirmed,
            "id:other-project",
            DATA_DIR,
            true
        )
        .is_err()
    );
    assert!(
        link_existing(
            &store,
            &attempt,
            &confirmed,
            "id:proj-42",
            r"C:\somewhere\else",
            true
        )
        .is_err()
    );
    let linked = link_existing(&store, &attempt, &confirmed, "id:proj-42", DATA_DIR, true)
        .unwrap()
        .unwrap();
    assert_eq!(linked.state, DispatchState::Bound);
}

#[test]
fn reopen_with_an_unresolved_attempt_reconciles_and_stale_results_cannot_reopen() {
    let dir = std::env::temp_dir().join(format!("relay-dispatch-test-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("relay-v2.db");
    let selector = T3Target::Id("proj-42".into());
    let (claim, task_id, _project_id) = {
        let store = V2Store::open(&path).unwrap();
        let project = store
            .register_project(NewV2Project {
                name: "Sample".into(),
                slug: "sample".into(),
                repo_path: None,
                owner_repo: Some("sample/app".into()),
                t3_target: None,
            })
            .unwrap();
        store.set_t3_target(&project.id, Some(selector)).unwrap();
        let note = store
            .save_note(PreparedNoteWrite {
                write: NoteWrite::Create {
                    submission_key: "reopen-fixture".into(),
                },
                source: "test".into(),
                lines: vec![PreparedLine {
                    line_id: None,
                    text: "! Draft release notes".into(),
                    references: vec![],
                    task_intent: Some(PreparedTaskIntent {
                        agent_mode: AgentMode::Act,
                        project_id: Some(project.id.clone()),
                    }),
                }],
            })
            .unwrap();
        let task = note.tasks[0].clone();
        let input = begin_attempt(&store, &task.id).unwrap().unwrap();
        complete_attempt(&store, &task.id, &input.attempt_id, OUTPUT).unwrap();
        let claim = claim(&store, &start(&task.id, None)).unwrap().unwrap();
        (claim, task.id, project.id)
    };
    let old_attempt_id = claim.attempt.attempt_id.clone();
    let confirmed = identity(&claim.attempt);

    // Reopen the SQLite file: a crash after launch left the attempt in `launching`.
    {
        let reopened = V2Store::open(&path).unwrap();
        let recovery = recover(&reopened).unwrap();
        assert_eq!(recovery.reconcile.len(), 1);
        assert!(recovery.start.is_empty());
        assert_eq!(
            recovery.reconcile[0].state,
            DispatchState::ReconcileRequired
        );
        assert_eq!(recovery.reconcile[0].attempt_id, old_attempt_id);
        // A stale result for the abandoned attempt cannot bind a fresh Task or start a new thread.
        assert!(
            bind_after_cancel(&reopened, "stale-attempt", &confirmed)
                .unwrap()
                .is_none()
        );
        assert!(retry(&reopened, &start(&task_id, None)).unwrap().is_none());
        let view = view(&reopened, &task_id).unwrap().unwrap();
        assert!(view.can_check_again && view.can_link_existing && !view.can_retry);
        let bound = bind_after_cancel(&reopened, &old_attempt_id, &confirmed)
            .unwrap()
            .unwrap();
        assert_eq!(bound.state, DispatchState::Bound);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_callers_produce_exactly_one_claim() {
    let (store, task, _) = ready_store(AgentMode::Act, T3Target::Id("proj-42".into()));
    let shared = Arc::new(store);
    let task_id = task.id.clone();
    let winner = thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..8 {
            let store = shared.clone();
            let task_id = task_id.clone();
            handles.push(scope.spawn(move || {
                claim(&store, &start(&task_id, None))
                    .unwrap()
                    .map(|claim| claim.attempt.attempt_id)
            }));
        }
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(winner.len(), 1);
    assert_eq!(
        shared
            .latest_dispatch_attempt(&task_id)
            .unwrap()
            .unwrap()
            .attempt_id,
        winner[0]
    );
}
