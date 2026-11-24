//! One projection for the public Task lifecycle. Stage modules supply committed facts. The
//! store mirrors this projection, and adapters execute the next stage without re-deciding it.

use crate::v2::briefing::{
    BriefStatus, FAILURE_STAGE as BRIEF_FAILURE_STAGE, dispatch_readiness, source_signature,
};
use crate::v2::grilling::{FAILURE_STAGE as GRILL_FAILURE_STAGE, GrillStatus};
use crate::v2::{AgentMode, GrillTurn, TaskStatus, V2Error, V2Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProjection {
    pub status: TaskStatus,
    pub grill_turn: Option<GrillTurn>,
    pub pending_action: Option<String>,
    pub failure_stage: Option<String>,
    pub failure_reason: Option<String>,
}

impl TaskProjection {
    fn new(status: TaskStatus, pending_action: Option<&str>) -> Self {
        Self {
            status,
            grill_turn: None,
            pending_action: pending_action.map(str::to_string),
            failure_stage: None,
            failure_reason: None,
        }
    }
}

pub fn brief_projection(
    mode: AgentMode,
    status: BriefStatus,
    has_project: bool,
    has_brief: bool,
    failure_stage: Option<String>,
    failure_reason: Option<String>,
) -> TaskProjection {
    match status {
        BriefStatus::Failed => TaskProjection {
            status: TaskStatus::Failed,
            grill_turn: None,
            pending_action: Some("Retry the Briefing stage.".into()),
            failure_stage,
            failure_reason,
        },
        BriefStatus::WaitingReference => TaskProjection {
            status: TaskStatus::Failed,
            grill_turn: None,
            pending_action: Some(
                "Continue with link only, or Retry after the Reference resolves.".into(),
            ),
            failure_stage: Some(BRIEF_FAILURE_STAGE.into()),
            failure_reason: Some(
                "A Reference has no readable or cached content. Continue with link only or Retry."
                    .into(),
            ),
        },
        BriefStatus::Pending => TaskProjection::new(
            if mode == AgentMode::Grill {
                TaskStatus::Grilling
            } else {
                TaskStatus::Briefing
            },
            Some("Writing the Brief…"),
        ),
        BriefStatus::Ready | BriefStatus::Approved if !has_brief => TaskProjection::new(
            if mode == AgentMode::Grill {
                TaskStatus::Grilling
            } else {
                TaskStatus::Briefing
            },
            None,
        ),
        BriefStatus::Ready | BriefStatus::Approved if !has_project => {
            TaskProjection::new(TaskStatus::NeedsProject, None)
        }
        BriefStatus::Ready | BriefStatus::Approved
            if mode == AgentMode::Wait && status != BriefStatus::Approved =>
        {
            TaskProjection::new(TaskStatus::AwaitingApproval, None)
        }
        BriefStatus::Ready | BriefStatus::Approved => {
            TaskProjection::new(TaskStatus::Briefing, None)
        }
    }
}

pub fn grill_projection(
    status: GrillStatus,
    has_project: bool,
    failure_stage: Option<String>,
    failure_reason: Option<String>,
) -> TaskProjection {
    let (status, turn, pending_action) = match status {
        GrillStatus::Pending => (
            TaskStatus::Grilling,
            Some(GrillTurn::Agent),
            Some("Grilling this Task…"),
        ),
        GrillStatus::Question => (
            TaskStatus::Grilling,
            Some(GrillTurn::UserQuestion),
            Some("Answer the Grilling question."),
        ),
        GrillStatus::Brief | GrillStatus::Approved => (
            if has_project {
                TaskStatus::Grilling
            } else {
                TaskStatus::NeedsProject
            },
            Some(GrillTurn::ClosingApproval),
            Some("Review the Brief and press Go."),
        ),
        GrillStatus::Failed => (TaskStatus::Failed, None, Some("Retry the Grilling stage.")),
        GrillStatus::Cancelled => (TaskStatus::Cancelled, None, None),
    };
    TaskProjection {
        status,
        grill_turn: turn,
        pending_action: pending_action.map(str::to_string),
        failure_stage: if status == TaskStatus::Failed {
            Some(failure_stage.unwrap_or_else(|| GRILL_FAILURE_STAGE.into()))
        } else {
            None
        },
        failure_reason: if status == TaskStatus::Failed {
            failure_reason
        } else {
            None
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStage {
    Briefing,
    Grilling,
    Dispatch,
}

pub fn next_stage(store: &V2Store, task_id: &str) -> Result<Option<NextStage>, V2Error> {
    let state = store.task_line_state(task_id)?;
    let signature = source_signature(&state.line_text, &state.references);
    let brief = store.brief(task_id)?;
    let grill = store.grill(task_id)?;
    let dispatch_ready = dispatch_readiness(store, task_id)?.is_some();
    Ok(stage_from_facts(
        state.task.agent_mode,
        state.task.status,
        brief
            .as_ref()
            .map(|record| record.source_signature == signature),
        brief.as_ref().map(|record| record.status),
        grill
            .as_ref()
            .map(|record| record.source_signature == signature),
        grill.as_ref().map(|record| record.status),
        dispatch_ready,
    ))
}

pub fn refresh_task_projection(store: &V2Store, task_id: &str) -> Result<(), V2Error> {
    let state = store.task_line_state(task_id)?;
    if !matches!(
        state.task.status,
        TaskStatus::Briefing
            | TaskStatus::Grilling
            | TaskStatus::NeedsProject
            | TaskStatus::AwaitingApproval
    ) {
        return Ok(());
    }
    if state.task.agent_mode == AgentMode::Grill {
        if let Some(record) = store.grill(task_id)? {
            store.write_grill(&record, None)?;
        }
    } else if let Some(record) = store.brief(task_id)? {
        store.write_brief(&record)?;
    }
    Ok(())
}

fn stage_from_facts(
    mode: AgentMode,
    status: TaskStatus,
    brief_matches: Option<bool>,
    brief: Option<BriefStatus>,
    grill_matches: Option<bool>,
    grill: Option<GrillStatus>,
    dispatch_ready: bool,
) -> Option<NextStage> {
    if matches!(
        status,
        TaskStatus::Done
            | TaskStatus::Cancelled
            | TaskStatus::Dispatched
            | TaskStatus::NeedsAnswer
            | TaskStatus::Returned
    ) {
        return None;
    }
    if !brief_matches.unwrap_or(false) && mode != AgentMode::Grill {
        return Some(NextStage::Briefing);
    }
    if !grill_matches.unwrap_or(false) && mode == AgentMode::Grill {
        return Some(NextStage::Grilling);
    }
    if status == TaskStatus::Failed {
        return None;
    }
    match mode {
        AgentMode::Act | AgentMode::Wait => match brief {
            Some(BriefStatus::Ready | BriefStatus::Approved) if dispatch_ready => {
                Some(NextStage::Dispatch)
            }
            _ => None,
        },
        AgentMode::Grill => match grill {
            Some(GrillStatus::Approved) if dispatch_ready => Some(NextStage::Dispatch),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn act_becomes_ready_only_after_brief_and_project() {
        assert_eq!(
            brief_projection(AgentMode::Act, BriefStatus::Ready, false, true, None, None).status,
            TaskStatus::NeedsProject
        );
        assert_eq!(
            brief_projection(AgentMode::Act, BriefStatus::Ready, true, true, None, None).status,
            TaskStatus::Briefing
        );
    }

    #[test]
    fn wait_requires_one_approval() {
        assert_eq!(
            brief_projection(AgentMode::Wait, BriefStatus::Ready, true, true, None, None).status,
            TaskStatus::AwaitingApproval
        );
        assert_eq!(
            brief_projection(
                AgentMode::Wait,
                BriefStatus::Approved,
                true,
                true,
                None,
                None
            )
            .status,
            TaskStatus::Briefing
        );
    }

    #[test]
    fn grill_question_and_closing_approval_have_distinct_turns() {
        let question = grill_projection(GrillStatus::Question, true, None, None);
        assert_eq!(question.status, TaskStatus::Grilling);
        assert_eq!(question.grill_turn, Some(GrillTurn::UserQuestion));
        let closing = grill_projection(GrillStatus::Brief, false, None, None);
        assert_eq!(closing.status, TaskStatus::NeedsProject);
        assert_eq!(closing.grill_turn, Some(GrillTurn::ClosingApproval));
    }

    #[test]
    fn failures_keep_the_stage_and_reason() {
        let brief = brief_projection(
            AgentMode::Act,
            BriefStatus::Failed,
            true,
            true,
            Some("briefing".into()),
            Some("provider offline".into()),
        );
        assert_eq!(brief.status, TaskStatus::Failed);
        assert_eq!(brief.failure_reason.as_deref(), Some("provider offline"));
        let grill = grill_projection(
            GrillStatus::Failed,
            true,
            Some("grilling".into()),
            Some("turn failed".into()),
        );
        assert_eq!(grill.status, TaskStatus::Failed);
        assert_eq!(grill.failure_reason.as_deref(), Some("turn failed"));
    }

    #[test]
    fn next_stage_starts_missing_or_stale_work_once() {
        assert_eq!(
            stage_from_facts(
                AgentMode::Act,
                TaskStatus::Briefing,
                None,
                None,
                None,
                None,
                false,
            ),
            Some(NextStage::Briefing)
        );
        assert_eq!(
            stage_from_facts(
                AgentMode::Act,
                TaskStatus::Failed,
                Some(false),
                Some(BriefStatus::Failed),
                None,
                None,
                false,
            ),
            Some(NextStage::Briefing)
        );
        assert_eq!(
            stage_from_facts(
                AgentMode::Act,
                TaskStatus::Briefing,
                Some(true),
                Some(BriefStatus::Pending),
                None,
                None,
                false,
            ),
            None
        );
    }

    #[test]
    fn project_assignment_refreshes_a_wait_projection() {
        use crate::v2::{
            BriefRecord, NewV2Project, NoteWrite, PreparedLine, PreparedNoteWrite,
            PreparedTaskIntent,
        };
        let store = V2Store::open_in_memory().expect("store");
        let project = store
            .register_project(NewV2Project {
                name: "Sample".into(),
                slug: "sample".into(),
                repo_path: None,
                owner_repo: None,
                t3_target: None,
            })
            .expect("project");
        let note = store
            .save_note(PreparedNoteWrite {
                write: NoteWrite::Create {
                    submission_key: "lifecycle-project".into(),
                },
                source: "test".into(),
                lines: vec![PreparedLine {
                    line_id: None,
                    text: "? Wait for the Brief".into(),
                    references: Vec::new(),
                    task_intent: Some(PreparedTaskIntent {
                        agent_mode: AgentMode::Wait,
                        project_id: None,
                    }),
                }],
            })
            .expect("note");
        let task_id = note.tasks[0].id.clone();
        store
            .write_brief(&BriefRecord {
                task_id: task_id.clone(),
                revision: 1,
                source_signature: "signature".into(),
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
        refresh_task_projection(&store, &task_id).expect("refresh");
        assert_eq!(
            store.task_line_state(&task_id).expect("task").task.status,
            TaskStatus::AwaitingApproval
        );
    }

    #[test]
    fn next_stage_dispatches_only_after_the_final_gate() {
        assert_eq!(
            stage_from_facts(
                AgentMode::Wait,
                TaskStatus::AwaitingApproval,
                Some(true),
                Some(BriefStatus::Ready),
                None,
                None,
                false,
            ),
            None
        );
        assert_eq!(
            stage_from_facts(
                AgentMode::Wait,
                TaskStatus::Briefing,
                Some(true),
                Some(BriefStatus::Approved),
                None,
                None,
                true,
            ),
            Some(NextStage::Dispatch)
        );
        assert_eq!(
            stage_from_facts(
                AgentMode::Grill,
                TaskStatus::Grilling,
                Some(true),
                Some(BriefStatus::Ready),
                Some(true),
                Some(GrillStatus::Approved),
                true,
            ),
            Some(NextStage::Dispatch)
        );
    }
}
