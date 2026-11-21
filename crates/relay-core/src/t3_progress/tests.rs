use crate::model::new_id;
use crate::t3_dispatch::{DispatchStart, ThreadIdentity, bind, claim};
use crate::v2::{
    AgentMode, NewV2Project, NoteWrite, PreparedLine, PreparedNoteWrite, PreparedTaskIntent,
    T3Target, TaskStatus, V2Store, begin_attempt, complete_attempt,
};

use super::*;

const DATA_DIR: &str = r"C:\Users\tester\.t3";
const BRIEF: &str = r#"{"goal":"Read-only smoke","decisions":["Reply only"],"constraints":["Read only"],"openRisks":["None"]}"#;

fn bound_store() -> (V2Store, String, String, String) {
    let store = V2Store::open_in_memory().expect("store");
    let (task_id, attempt_id, thread_id) = build_bound(&store);
    (store, task_id, attempt_id, thread_id)
}

fn build_bound(store: &V2Store) -> (String, String, String) {
    let project = store
        .register_project(NewV2Project {
            name: "Smoke".into(),
            slug: "smoke".into(),
            repo_path: None,
            owner_repo: None,
            t3_target: None,
        })
        .expect("project");
    store
        .set_t3_target(&project.id, Some(T3Target::Id("proj-42".into())))
        .expect("mapping");
    let note = store
        .save_note(PreparedNoteWrite {
            write: NoteWrite::Create {
                submission_key: format!("progress-fixture-{}", new_id()),
            },
            source: "test".into(),
            lines: vec![PreparedLine {
                line_id: None,
                text: "! Read-only progress fixture".into(),
                references: vec![],
                task_intent: Some(PreparedTaskIntent {
                    agent_mode: AgentMode::Act,
                    project_id: Some(project.id.clone()),
                }),
            }],
        })
        .expect("note");
    let task_id = note.tasks[0].id.clone();
    let attempt = begin_attempt(&store, &task_id).unwrap().unwrap();
    complete_attempt(&store, &task_id, &attempt.attempt_id, BRIEF).unwrap();
    let claim = claim(
        &store,
        &DispatchStart {
            task_id: task_id.clone(),
            data_dir: DATA_DIR.into(),
            base_ref: None,
        },
    )
    .unwrap()
    .unwrap();
    let identity = ThreadIdentity {
        thread_id: "thread-1".into(),
        run_id: "run-1".into(),
        link: "https://app.t3.codes/threads/env/thread-1".into(),
        project_id: "proj-42".into(),
    };
    bind(&store, &claim.attempt.attempt_id, &identity).unwrap();
    assert!(
        store
            .ensure_progress(
                &claim.attempt.attempt_id,
                &identity.thread_id,
                Some(&identity.run_id)
            )
            .unwrap()
    );
    (task_id, claim.attempt.attempt_id, identity.thread_id)
}

fn observation(status: RunStatus, run_id: &str) -> ThreadObservation {
    ThreadObservation {
        thread_id: "thread-1".into(),
        active_run_id: Some(run_id.to_string()),
        latest_run_id: Some(run_id.to_string()),
        status,
        pending_request_count: 0,
        branch: Some("smoke/branch".into()),
        worktree_path: Some(r"C:\worktrees\smoke".into()),
        link: Some("https://app.t3.codes/threads/env/thread-1".into()),
        project_id: Some("proj-42".into()),
        run_count: 1,
        messages: vec![],
        pull_requests: vec![],
        error: None,
    }
}

fn context<'a>(
    status: TaskStatus,
    expected: Option<&'a str>,
    send: Option<&'a SendOperation>,
    observation: &'a ThreadObservation,
) -> ObservationContext<'a> {
    ObservationContext {
        current_status: status,
        expected_run_id: expected,
        send,
        observation,
    }
}

#[test]
fn active_states_display_dispatched_and_pending_requests_enter_needs_answer() {
    for status in [
        RunStatus::Preparing,
        RunStatus::Queued,
        RunStatus::Starting,
        RunStatus::Running,
        RunStatus::Waiting,
    ] {
        let mut observed = observation(status.clone(), "run-1");
        let effect = decide(&context(
            TaskStatus::Dispatched,
            Some("run-1"),
            None,
            &observed,
        ))
        .expect("active run has an effect");
        assert_eq!(effect.status, TaskStatus::Dispatched, "{observed:?}");
        observed.pending_request_count = 1;
        let effect = decide(&context(
            TaskStatus::Dispatched,
            Some("run-1"),
            None,
            &observed,
        ))
        .unwrap();
        assert_eq!(effect.status, TaskStatus::NeedsAnswer, "{observed:?}");
    }
}

#[test]
fn matching_terminal_results_return_or_fail_and_idle_or_unknown_preserve_state() {
    let completed = observation(RunStatus::Completed, "run-1");
    assert_eq!(
        decide(&context(
            TaskStatus::Dispatched,
            Some("run-1"),
            None,
            &completed
        ))
        .unwrap()
        .status,
        TaskStatus::Returned
    );
    // A completed run that does not match the expected run cannot return the Task.
    assert!(
        decide(&context(
            TaskStatus::Dispatched,
            Some("run-2"),
            None,
            &completed
        ))
        .is_none()
    );
    for status in [
        RunStatus::Failed,
        RunStatus::Interrupted,
        RunStatus::Cancelled,
        RunStatus::RolledBack,
    ] {
        let observed = observation(status.clone(), "run-1");
        let effect = decide(&context(
            TaskStatus::Dispatched,
            Some("run-1"),
            None,
            &observed,
        ))
        .unwrap_or_else(|| panic!("{status:?} must be actionable"));
        assert_eq!(effect.status, TaskStatus::Failed);
        assert_eq!(effect.failure_stage.as_deref(), Some(FAILURE_STAGE));
        assert!(effect.failure_reason.unwrap().contains(status.as_str()));
    }
    let idle = observation(RunStatus::Idle, "run-1");
    assert!(decide(&context(TaskStatus::Dispatched, Some("run-1"), None, &idle)).is_none());
    let unknown = observation(RunStatus::Unknown("mystery".into()), "run-1");
    assert!(
        decide(&context(
            TaskStatus::Dispatched,
            Some("run-1"),
            None,
            &unknown
        ))
        .is_none()
    );
}

#[test]
fn replies_are_selected_by_assistant_role_and_run_id() {
    let messages = vec![
        ObservedMessage {
            role: "user".into(),
            run_id: "run-1".into(),
            text: "please continue".into(),
        },
        ObservedMessage {
            role: "assistant".into(),
            run_id: "run-0".into(),
            text: "an older answer".into(),
        },
        ObservedMessage {
            role: "assistant".into(),
            run_id: "run-1".into(),
            text: "RELAY_RETURNED".into(),
        },
    ];
    assert_eq!(
        assistant_reply(&messages, Some("run-1")).as_deref(),
        Some("RELAY_RETURNED")
    );
    assert_eq!(assistant_reply(&messages, Some("run-2")).as_deref(), None);
    assert_eq!(
        assistant_reply(&messages, None).as_deref(),
        Some("RELAY_RETURNED")
    );
}

#[test]
fn a_prior_run_cannot_return_a_task_with_an_acknowledged_queued_send() {
    let send = SendOperation {
        operation_id: "op-1".into(),
        attempt_id: "attempt-1".into(),
        task_id: "task-1".into(),
        thread_id: "thread-1".into(),
        kind: SendKind::Followup,
        state: SendState::Acknowledged,
        message: "follow up".into(),
        baseline_run_id: Some("run-1".into()),
        baseline_run_count: 1,
        run_id: Some("run-2".into()),
        delivery_state: Some("accepted".into()),
        failure_reason: None,
        revision: 1,
    };
    let mut prior = observation(RunStatus::Completed, "run-1");
    assert!(
        decide(&context(
            TaskStatus::Dispatched,
            Some("run-2"),
            Some(&send),
            &prior,
        ))
        .is_none()
    );
    prior.status = RunStatus::Waiting;
    prior.pending_request_count = 1;
    assert_eq!(
        decide(&context(
            TaskStatus::Dispatched,
            Some("run-2"),
            Some(&send),
            &prior,
        ))
        .unwrap()
        .status,
        TaskStatus::NeedsAnswer
    );
}

#[test]
fn a_queued_send_needs_one_new_run_with_the_exact_message() {
    let send = SendOperation {
        operation_id: "op-1".into(),
        attempt_id: "attempt-1".into(),
        task_id: "task-1".into(),
        thread_id: "thread-1".into(),
        kind: SendKind::Followup,
        state: SendState::NeedsConfirmation,
        message: "line one\nline two".into(),
        baseline_run_id: Some("run-1".into()),
        baseline_run_count: 1,
        run_id: None,
        delivery_state: None,
        failure_reason: Some("lost response".into()),
        revision: 1,
    };
    let mut observed = observation(RunStatus::Completed, "run-2");
    observed.run_count = 1;
    assert!(confirms_send(&send, &observed).is_none());
    observed.run_count = 2;
    observed.messages.push(ObservedMessage {
        role: "assistant".into(),
        run_id: "run-2".into(),
        text: "reply".into(),
    });
    assert!(confirms_send(&send, &observed).is_none());
    observed.messages.push(ObservedMessage {
        role: "user".into(),
        run_id: "run-2".into(),
        text: "different message".into(),
    });
    assert!(confirms_send(&send, &observed).is_none());
    observed.messages[1].text = send.message.clone();
    assert_eq!(confirms_send(&send, &observed).as_deref(), Some("run-2"));
}

fn make_guard(store: &V2Store, task_id: &str, attempt_id: &str, thread_id: &str) -> CommitGuard {
    let view = store.progress_view(task_id).unwrap().unwrap();
    let send = store.active_send(task_id).unwrap();
    CommitGuard {
        task_id: task_id.to_string(),
        attempt_id: attempt_id.to_string(),
        thread_id: thread_id.to_string(),
        revision: view.revision,
        operation_revision: send.map(|operation| operation.revision),
    }
}

fn mark_returned(store: &V2Store, task_id: &str, attempt_id: &str, thread_id: &str) {
    let mut observed = observation(RunStatus::Completed, "run-1");
    observed.messages.push(ObservedMessage {
        role: "assistant".into(),
        run_id: "run-1".into(),
        text: "first reply".into(),
    });
    let effect = decide(&context(
        TaskStatus::Dispatched,
        Some("run-1"),
        None,
        &observed,
    ))
    .unwrap();
    let guard = make_guard(store, task_id, attempt_id, thread_id);
    assert!(
        store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );
}

#[test]
fn commit_persists_observation_and_prs_and_preserves_links_on_a_subset() {
    let (store, task_id, attempt_id, thread_id) = bound_store();
    let mut observed = observation(RunStatus::Running, "run-1");
    observed.pull_requests.push(ObservedPullRequest {
        url: "https://github.com/sample/app/pull/142".into(),
        source: Some("agent".into()),
        state: Some("open".into()),
        watching: true,
    });
    observed.pull_requests.push(ObservedPullRequest {
        url: "https://github.com/sample/app/pull/143".into(),
        source: Some("agent".into()),
        state: Some("open".into()),
        watching: true,
    });
    let effect = decide(&context(
        TaskStatus::Dispatched,
        Some("run-1"),
        None,
        &observed,
    ))
    .unwrap();
    let guard = make_guard(&store, &task_id, &attempt_id, &thread_id);
    assert!(
        store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );
    let view = store.progress_view(&task_id).unwrap().unwrap();
    assert_eq!(view.task_status, TaskStatus::Dispatched.as_str());
    assert_eq!(view.branch.as_deref(), Some("smoke/branch"));
    assert_eq!(view.worktree_path.as_deref(), Some(r"C:\worktrees\smoke"));
    assert_eq!(view.pull_requests.len(), 2);

    // A stale revision cannot commit a second read that started before the first one landed.
    assert!(
        !store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );

    // The next response reports only one PR; the earlier link is preserved by canonical identity.
    observed.pull_requests.truncate(1);
    observed.pull_requests[0].state = Some("closed".into());
    observed.status = RunStatus::Completed;
    observed.messages.push(ObservedMessage {
        role: "assistant".into(),
        run_id: "run-1".into(),
        text: "RELAY_RETURNED".into(),
    });
    let effect = decide(&context(
        TaskStatus::Dispatched,
        Some("run-1"),
        None,
        &observed,
    ))
    .unwrap();
    let guard = make_guard(&store, &task_id, &attempt_id, &thread_id);
    assert!(
        store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );
    let view = store.progress_view(&task_id).unwrap().unwrap();
    assert_eq!(view.task_status, TaskStatus::Returned.as_str());
    assert_eq!(view.reply.as_deref(), Some("RELAY_RETURNED"));
    assert_eq!(
        view.pull_requests.len(),
        2,
        "a subset does not drop a seen PR"
    );
    assert!(view.can_follow_up);
    assert!(view.can_close);
}

#[test]
fn closure_invalidates_an_in_flight_read_and_a_late_result() {
    let (store, task_id, attempt_id, thread_id) = bound_store();
    let observed = observation(RunStatus::Completed, "run-1");
    let effect = decide(&context(
        TaskStatus::Dispatched,
        Some("run-1"),
        None,
        &observed,
    ))
    .unwrap();
    let guard = make_guard(&store, &task_id, &attempt_id, &thread_id);
    assert!(store.close_task(&task_id, TaskStatus::Cancelled).unwrap());
    assert!(
        !store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );
    let task = store
        .workspace_snapshot()
        .unwrap()
        .tasks
        .into_iter()
        .find(|t| t.id == task_id)
        .unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert!(
        !store
            .watch_items()
            .unwrap()
            .iter()
            .any(|item| item.task_id == task_id)
    );
    // A second close is a no-op; only an explicit user action ever reaches Done or Cancelled.
    assert!(!store.close_task(&task_id, TaskStatus::Done).unwrap());
}

#[test]
fn a_failed_read_keeps_the_last_good_view_and_marks_it_stale() {
    let (store, task_id, attempt_id, thread_id) = bound_store();
    let observed = observation(RunStatus::Running, "run-1");
    let effect = decide(&context(
        TaskStatus::Dispatched,
        Some("run-1"),
        None,
        &observed,
    ))
    .unwrap();
    let guard = make_guard(&store, &task_id, &attempt_id, &thread_id);
    assert!(
        store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );
    assert!(
        store
            .mark_stale(&task_id, "T3 is unreachable. Check it in Settings.")
            .unwrap()
    );
    let view = store.progress_view(&task_id).unwrap().unwrap();
    assert_eq!(
        view.branch.as_deref(),
        Some("smoke/branch"),
        "last good view is kept"
    );
    assert!(view.stale_reason.unwrap().contains("Settings"));
    assert_eq!(view.task_status, TaskStatus::Dispatched.as_str());
}

#[test]
fn queued_send_reserves_before_delivery_and_survives_a_restart() {
    let (store, task_id, attempt_id, thread_id) = bound_store();
    mark_returned(&store, &task_id, &attempt_id, &thread_id);

    let message = "first line\nsecond line";
    let send = store
        .reserve_send(&task_id, SendKind::Followup, message)
        .unwrap()
        .expect("a returned Task reserves one send");
    assert_eq!(send.message, message);
    assert_eq!(send.baseline_run_id.as_deref(), Some("run-1"));
    assert_eq!(send.state, SendState::Reserved);
    // A second caller cannot reserve another send while one is unresolved.
    assert!(
        store
            .reserve_send(&task_id, SendKind::Followup, "again")
            .unwrap()
            .is_none()
    );

    let draft = store.send_operation(&send.operation_id).unwrap().unwrap();
    assert_eq!(draft.message, message);
    assert_eq!(draft.state, SendState::Reserved);
    assert!(
        store
            .watch_items()
            .unwrap()
            .iter()
            .any(|item| item.task_id == task_id && item.send.is_some())
    );

    assert!(
        store
            .acknowledge_send(&send.operation_id, "run-2", "accepted")
            .unwrap()
    );
    let view = store.progress_view(&task_id).unwrap().unwrap();
    assert_eq!(view.task_status, TaskStatus::Dispatched.as_str());
    assert_eq!(view.run_id.as_deref(), Some("run-2"));
    assert_eq!(
        view.active_send.as_ref().unwrap().state,
        SendState::Acknowledged
    );
}

#[test]
fn a_reserved_send_is_restored_as_an_observation_after_restart() {
    let dir = std::env::temp_dir().join(format!("relay-37-restart-{}", new_id()));
    let path = dir.join("relay-v2.db");
    let operation_id = {
        let store = V2Store::open(&path).unwrap();
        let (task_id, attempt_id, thread_id) = build_bound(&store);
        mark_returned(&store, &task_id, &attempt_id, &thread_id);
        store
            .reserve_send(&task_id, SendKind::Followup, "verbatim across restart")
            .unwrap()
            .unwrap()
            .operation_id
    };
    // The reopened store sees the reserved draft and the watch item; no CLI call is replayed here.
    let reopened = V2Store::open(&path).unwrap();
    let draft = reopened.send_operation(&operation_id).unwrap().unwrap();
    assert_eq!(draft.message, "verbatim across restart");
    assert_eq!(draft.state, SendState::Reserved);
    assert_eq!(draft.baseline_run_count, 1);
    let watches = reopened.watch_items().unwrap();
    assert_eq!(watches.len(), 1);
    assert_eq!(watches[0].send.as_ref().unwrap().operation_id, operation_id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_lost_response_fences_sends_until_show_confirms_the_exact_message() {
    let (store, task_id, attempt_id, thread_id) = bound_store();
    mark_returned(&store, &task_id, &attempt_id, &thread_id);

    let send = store
        .reserve_send(&task_id, SendKind::Followup, "exact text")
        .unwrap()
        .unwrap();
    assert!(
        store
            .fail_send(&send.operation_id, false, "the send response was lost")
            .unwrap()
    );
    assert_eq!(
        store.active_send(&task_id).unwrap().unwrap().state,
        SendState::NeedsConfirmation
    );
    assert!(
        store
            .reserve_send(&task_id, SendKind::Followup, "second try")
            .unwrap()
            .is_none()
    );

    let mut confirmed = observation(RunStatus::Running, "run-2");
    confirmed.run_count = 2;
    confirmed.messages.push(ObservedMessage {
        role: "user".into(),
        run_id: "run-2".into(),
        text: "exact text".into(),
    });
    assert_eq!(
        confirms_send(&store.active_send(&task_id).unwrap().unwrap(), &confirmed).as_deref(),
        Some("run-2")
    );
    assert!(store.confirm_send(&send.operation_id, "run-2").unwrap());
    let view = store.progress_view(&task_id).unwrap().unwrap();
    assert_eq!(view.task_status, TaskStatus::Dispatched.as_str());
    assert_eq!(
        view.active_send.as_ref().unwrap().state,
        SendState::Acknowledged
    );
}

#[test]
fn a_known_pre_send_failure_keeps_the_draft_for_an_explicit_retry() {
    let (store, task_id, attempt_id, thread_id) = bound_store();
    let mut observed = observation(RunStatus::Failed, "run-1");
    observed.error = Some("provider error".into());
    let effect = decide(&context(
        TaskStatus::Dispatched,
        Some("run-1"),
        None,
        &observed,
    ))
    .unwrap();
    let guard = make_guard(&store, &task_id, &attempt_id, &thread_id);
    assert!(
        store
            .commit_observation(&guard, &effect, &observed)
            .unwrap()
    );
    let send = store
        .reserve_send(&task_id, SendKind::Retry, RETRY_MESSAGE)
        .unwrap()
        .unwrap();
    assert!(
        store
            .fail_send(&send.operation_id, true, "T3 Code is not running")
            .unwrap()
    );
    let view = store.progress_view(&task_id).unwrap().unwrap();
    assert_eq!(view.active_send.as_ref().unwrap().state, SendState::Failed);
    assert!(view.can_resend);
    assert_eq!(view.active_send.as_ref().unwrap().message, RETRY_MESSAGE);
}
