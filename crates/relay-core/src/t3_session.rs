//! One durable T3 thread session. Core owns the sequence from intended start through bound
//! identity and read-only reconciliation. A transport adapter supplies CLI results only.

use crate::t3_dispatch::{
    DispatchAttempt, DispatchClaim, DispatchStart, ListedThread, ThreadIdentity, bind, claim,
    identity_matches, record_known_failure, record_uncertain, save_thread_id, unique_candidate,
};
use crate::v2::{T3Target, V2Error, V2Store};

#[derive(Debug)]
pub enum TransportError {
    LauncherMissing,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    Responded(ThreadIdentity),
    KnownFailure(String),
    Uncertain(String),
}

pub struct StartRequest<'a> {
    pub selector: &'a str,
    pub data_dir: &'a str,
    pub title: &'a str,
    pub brief: &'a str,
    pub base_ref: Option<&'a str>,
}

pub trait Transport {
    fn start(&self, request: &StartRequest<'_>) -> Result<StartOutcome, TransportError>;
    fn list(&self, selector: &str, data_dir: &str) -> Result<Vec<ListedThread>, TransportError>;
    fn show(&self, thread_id: &str, data_dir: &str) -> Result<ThreadIdentity, TransportError>;
}

pub fn start(
    store: &V2Store,
    transport: &dyn Transport,
    start: &DispatchStart,
) -> Result<Option<DispatchAttempt>, V2Error> {
    let Some(claim) = claim(store, start)? else {
        return Ok(None);
    };
    let DispatchClaim {
        attempt,
        brief,
        selector,
    } = claim;
    let outcome = transport.start(&StartRequest {
        selector: &selector,
        data_dir: &attempt.t3_data_dir,
        title: &attempt.title,
        brief: &brief,
        base_ref: attempt.base_ref.as_deref(),
    });
    match outcome {
        Ok(StartOutcome::Responded(mut identity)) => {
            match complete_identity(store, transport, &attempt, &mut identity) {
                Ok(attempt) => Ok(attempt),
                Err(error) => record_uncertain(store, &attempt.attempt_id, &error.to_string()),
            }
        }
        Ok(StartOutcome::KnownFailure(reason)) => {
            record_known_failure(store, &attempt.attempt_id, &reason)
        }
        Ok(StartOutcome::Uncertain(reason)) => {
            record_uncertain(store, &attempt.attempt_id, &reason)
        }
        Err(TransportError::LauncherMissing) => record_known_failure(
            store,
            &attempt.attempt_id,
            "The t3 launcher was not found. Install T3 Code or restart Relay.",
        ),
        Err(TransportError::Other(reason)) => record_uncertain(store, &attempt.attempt_id, &reason),
    }
}

pub fn reconcile(
    store: &V2Store,
    transport: &dyn Transport,
    attempt: &DispatchAttempt,
) -> Result<Option<DispatchAttempt>, V2Error> {
    let selector = T3Target::from_storage(&attempt.project_selector)
        .map(|target| target.value().to_string())
        .unwrap_or_default();
    let candidates = match transport.list(&selector, &attempt.t3_data_dir) {
        Ok(candidates) => candidates,
        Err(error) => {
            let reason = match error {
                TransportError::LauncherMissing => "The t3 launcher was not found.".to_string(),
                TransportError::Other(reason) => reason,
            };
            return record_uncertain(store, &attempt.attempt_id, &reason);
        }
    };
    let Some(candidate) = unique_candidate(attempt, &candidates) else {
        return record_uncertain(
            store,
            &attempt.attempt_id,
            "No unique thread with the attempt title was found. Check again or link it.",
        );
    };
    let identity = match transport.show(&candidate.thread_id, &attempt.t3_data_dir) {
        Ok(identity) => identity,
        Err(error) => {
            let reason = match error {
                TransportError::LauncherMissing => "The t3 launcher was not found.".to_string(),
                TransportError::Other(reason) => reason,
            };
            return record_uncertain(store, &attempt.attempt_id, &reason);
        }
    };
    if !identity_matches(attempt, &identity) {
        return record_uncertain(
            store,
            &attempt.attempt_id,
            "The matching thread belongs to another project or profile.",
        );
    }
    bind(store, &attempt.attempt_id, &identity)
}

fn complete_identity(
    store: &V2Store,
    transport: &dyn Transport,
    attempt: &DispatchAttempt,
    identity: &mut ThreadIdentity,
) -> Result<Option<DispatchAttempt>, V2Error> {
    save_thread_id(store, &attempt.attempt_id, &identity.thread_id)?;
    if !identity.is_complete() {
        let shown = transport
            .show(&identity.thread_id, &attempt.t3_data_dir)
            .map_err(|error| {
                let reason = match error {
                    TransportError::LauncherMissing => "The t3 launcher was not found.".to_string(),
                    TransportError::Other(reason) => reason,
                };
                V2Error::InvalidInput(reason)
            })?;
        if identity.run_id.trim().is_empty() {
            identity.run_id = shown.run_id;
        }
        if identity.link.trim().is_empty() {
            identity.link = shown.link;
        }
        if identity.project_id.trim().is_empty() {
            identity.project_id = shown.project_id;
        }
    }
    bind(store, &attempt.attempt_id, identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t3_dispatch::DispatchState;
    use crate::v2::briefing::{BriefRecord, BriefStatus, source_signature};
    use crate::v2::{
        AgentMode, NewV2Project, NoteWrite, PreparedLine, PreparedNoteWrite, PreparedTaskIntent,
        TaskStatus,
    };
    use std::cell::RefCell;

    struct FakeTransport {
        start: RefCell<StartOutcome>,
        listed: Vec<ListedThread>,
        shown: ThreadIdentity,
    }

    impl Transport for FakeTransport {
        fn start(&self, _request: &StartRequest<'_>) -> Result<StartOutcome, TransportError> {
            Ok(self.start.borrow().clone())
        }

        fn list(
            &self,
            _selector: &str,
            _data_dir: &str,
        ) -> Result<Vec<ListedThread>, TransportError> {
            Ok(self.listed.clone())
        }

        fn show(
            &self,
            _thread_id: &str,
            _data_dir: &str,
        ) -> Result<ThreadIdentity, TransportError> {
            Ok(self.shown.clone())
        }
    }

    fn fixture() -> (V2Store, String, DispatchAttempt) {
        let store = V2Store::open_in_memory().expect("store");
        let project = store
            .register_project(NewV2Project {
                name: "Sample".into(),
                slug: "sample".into(),
                repo_path: None,
                owner_repo: None,
                t3_target: Some("id:project".into()),
            })
            .expect("project");
        let note = store
            .save_note(PreparedNoteWrite {
                write: NoteWrite::Create {
                    submission_key: "t3-session".into(),
                },
                source: "test".into(),
                lines: vec![PreparedLine {
                    line_id: None,
                    text: "! Ship the change".into(),
                    references: Vec::new(),
                    task_intent: Some(PreparedTaskIntent {
                        agent_mode: AgentMode::Act,
                        project_id: None,
                    }),
                }],
            })
            .expect("note");
        let task_id = note.tasks[0].id.clone();
        let state = store.task_line_state(&task_id).expect("state");
        store
            .write_brief(&BriefRecord {
                task_id: task_id.clone(),
                revision: 1,
                source_signature: source_signature(&state.line_text, &state.references),
                status: BriefStatus::Ready,
                content: None,
                rendered: Some("Brief".into()),
                link_only_refs: Vec::new(),
                wait_approval_revision: None,
                attempt_id: None,
                rewrite_instruction: None,
                failure_stage: None,
                failure_reason: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            })
            .expect("brief");
        store
            .assign_task_project(&task_id, &project.id)
            .expect("assign");
        crate::task_lifecycle::refresh_task_projection(&store, &task_id).expect("refresh");
        let claim = claim(
            &store,
            &DispatchStart {
                task_id: task_id.clone(),
                data_dir: "C:\\t3".into(),
                base_ref: None,
            },
        )
        .expect("claim")
        .expect("ready");
        // Release the fixture claim so the tested session can make its own durable attempt.
        store
            .finish_dispatch(
                &claim.attempt.attempt_id,
                &[DispatchState::Launching],
                DispatchState::LaunchFailed,
                None,
                Some("fixture".into()),
                TaskStatus::Briefing,
                None,
            )
            .expect("release");
        (store, task_id, claim.attempt)
    }

    fn identity() -> ThreadIdentity {
        ThreadIdentity {
            thread_id: "thread-1".into(),
            run_id: "run-1".into(),
            link: "t3code://thread-1".into(),
            project_id: "project".into(),
        }
    }

    #[test]
    fn complete_start_binds_one_identity() {
        let (store, task_id, _) = fixture();
        let transport = FakeTransport {
            start: RefCell::new(StartOutcome::Responded(identity())),
            listed: Vec::new(),
            shown: identity(),
        };
        let attempt = start(
            &store,
            &transport,
            &DispatchStart {
                task_id,
                data_dir: "C:\\t3".into(),
                base_ref: None,
            },
        )
        .expect("start")
        .expect("attempt");
        assert_eq!(attempt.state, DispatchState::Bound);
    }

    #[test]
    fn uncertain_start_reconciles_only_a_unique_matching_title() {
        let (store, task_id, _) = fixture();
        let transport = FakeTransport {
            start: RefCell::new(StartOutcome::Uncertain("lost response".into())),
            listed: vec![ListedThread {
                thread_id: "thread-1".into(),
                title: Some(format!("Relay Task {}", "unknown")),
                project_id: Some("project".into()),
            }],
            shown: identity(),
        };
        let attempt = start(
            &store,
            &transport,
            &DispatchStart {
                task_id,
                data_dir: "C:\\t3".into(),
                base_ref: None,
            },
        )
        .expect("start")
        .expect("attempt");
        assert_eq!(attempt.state, DispatchState::ReconcileRequired);
        let listed = vec![ListedThread {
            thread_id: "thread-1".into(),
            title: Some(attempt.title.clone()),
            project_id: Some("project".into()),
        }];
        let transport = FakeTransport {
            start: RefCell::new(StartOutcome::Uncertain("unused".into())),
            listed,
            shown: identity(),
        };
        let bound = reconcile(&store, &transport, &attempt)
            .expect("reconcile")
            .expect("attempt");
        assert_eq!(bound.state, DispatchState::Bound);
    }
}
