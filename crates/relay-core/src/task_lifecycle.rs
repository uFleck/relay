//! One projection for the public Task lifecycle. Stage modules supply committed facts. The
//! store mirrors this projection, and adapters execute the next stage without re-deciding it.

use crate::v2::briefing::{BriefStatus, FAILURE_STAGE as BRIEF_FAILURE_STAGE};
use crate::v2::grilling::{FAILURE_STAGE as GRILL_FAILURE_STAGE, GrillStatus};
use crate::v2::{AgentMode, GrillTurn, TaskStatus};

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
}
