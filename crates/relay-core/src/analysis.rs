//! JEV prompts for the two-stage filter (PLAN 3.2). Pure: the caller runs the calls and
//! stores the answers. Keeping the questions here means the thresholds, ids and wording
//! are reviewable in one place.

use std::collections::BTreeMap;

use crate::jev::{Question, ids};

/// Stage B gets the episode capped at this many characters: the first message plus the
/// newest tail, so the start and the current state both fit.
pub const TRANSCRIPT_LIMIT: usize = 12_000;

/// A shortlisted task for stage B. `status` is the human status name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateTask {
    pub id: String,
    pub title: String,
    pub status: String,
}

/// A project slug offered as a choice when a new task is likely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChoice {
    pub slug: String,
    pub name: String,
}

/// One line of the episode, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLine {
    /// The peer's name, or `me` for the account owner.
    pub author: String,
    pub text: String,
}

/// Stage A: cheap, no task list.
pub fn screen_questions() -> BTreeMap<String, Question> {
    let mut questions = BTreeMap::new();
    questions.insert(
        ids::WORTH_REVIEWING.to_string(),
        Question::noul(
            "Does this conversation contain a request, decision, requirement, deadline, \
             scope change or commitment that could affect work?",
        ),
    );
    questions.insert(
        ids::CONVERSATION_STATE.to_string(),
        Question::choice(
            "What is the state of this conversation right now?",
            [
                ("ongoing", "still being discussed"),
                ("waiting_on_me", "waiting for my reply or action"),
                ("resolved", "decided or finished"),
                ("abandoned", "dropped without a decision"),
            ],
        ),
    );
    questions
}

/// Stage B: match and classify against the shortlist.
pub fn classify_questions(
    candidates: &[CandidateTask],
    projects: &[ProjectChoice],
) -> BTreeMap<String, Question> {
    let mut questions = BTreeMap::new();
    questions.insert(
        ids::AFFECTS_EXISTING.to_string(),
        Question::noul(
            "Does this conversation change the scope, requirements, deadline, priority or \
             status of one of the listed tasks?",
        ),
    );
    let mut task_criteria: BTreeMap<String, String> = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.id.clone(),
                format!("{} ({})", candidate.title, candidate.status),
            )
        })
        .collect();
    task_criteria.insert(ids::TASK_NONE.to_string(), "no listed task".into());
    task_criteria.insert(
        ids::TASK_MULTIPLE.to_string(),
        "more than one listed task".into(),
    );
    questions.insert(
        ids::TASK.to_string(),
        Question::choice(
            "Which listed task does this conversation affect?",
            task_criteria,
        ),
    );
    questions.insert(
        ids::CHANGE_KIND.to_string(),
        Question::choice(
            "What kind of change does this conversation ask for?",
            [
                ("new_task", "asks for work no listed task covers"),
                ("add_requirement", "adds a requirement to a task"),
                ("change_scope", "changes what a task includes"),
                ("change_deadline", "changes a date or deadline"),
                ("change_priority", "changes priority or ordering"),
                ("report_status", "reports progress on a task"),
                ("decision", "records a decision"),
                ("no_change", "no task change is needed"),
            ],
        ),
    );
    questions.insert(
        ids::IS_NEW_TASK.to_string(),
        Question::noul("Does this ask for work that none of the listed tasks cover?"),
    );
    questions.insert(
        ids::CONVERSATION_COMPLETE.to_string(),
        Question::noul("Has the conversation reached a decision or a natural end?"),
    );
    questions.insert(
        ids::URGENCY.to_string(),
        Question::score(
            "How urgent is this for the person being asked?",
            ["can wait", "this week", "today-blocking"],
        ),
    );
    questions.insert(
        ids::NEEDS_HUMAN.to_string(),
        Question::noul(
            "Is this ambiguous, conflicting with a task, or risky to apply without a human?",
        ),
    );
    let mut project_criteria: BTreeMap<String, String> = projects
        .iter()
        .map(|project| (project.slug.clone(), project.name.clone()))
        .collect();
    project_criteria.insert(ids::PROJECT_UNSURE.to_string(), "cannot tell".into());
    questions.insert(
        ids::PROJECT.to_string(),
        Question::choice("Which project should a new task go to?", project_criteria),
    );
    questions
}

/// The episode as the model sees it: `Author: text`, oldest first, capped.
pub fn transcript(lines: &[TranscriptLine]) -> String {
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| format!("{}: {}", line.author, line.text.trim()))
        .collect();
    let total: usize = rendered.iter().map(|line| line.len() + 1).sum();
    if total <= TRANSCRIPT_LIMIT {
        return rendered.join("\n");
    }

    // Keep the first line and the newest lines that fit.
    let mut tail: Vec<&str> = Vec::new();
    let mut used = rendered.first().map_or(0, |line| line.len() + 1);
    for line in rendered.iter().skip(1).rev() {
        if used + line.len() + 1 > TRANSCRIPT_LIMIT {
            break;
        }
        used += line.len() + 1;
        tail.push(line);
    }
    tail.reverse();
    let mut out = String::new();
    if let Some(first) = rendered.first() {
        out.push_str(first);
        out.push('\n');
    }
    out.push_str("…\n");
    out.push_str(&tail.join("\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_asks_the_two_stage_a_questions() {
        let questions = screen_questions();
        assert_eq!(questions.len(), 2);
        assert!(questions.contains_key(ids::WORTH_REVIEWING));
        assert!(questions.contains_key(ids::CONVERSATION_STATE));
    }

    #[test]
    fn classify_offers_candidates_none_and_multiple() {
        let candidates = vec![
            CandidateTask {
                id: "T1".into(),
                title: "Ship settings".into(),
                status: "In Progress".into(),
            },
            CandidateTask {
                id: "T2".into(),
                title: "Fix hover".into(),
                status: "Todo".into(),
            },
        ];
        let projects = vec![ProjectChoice {
            slug: "relay".into(),
            name: "Relay".into(),
        }];
        let questions = classify_questions(&candidates, &projects);
        for id in [
            ids::AFFECTS_EXISTING,
            ids::TASK,
            ids::CHANGE_KIND,
            ids::IS_NEW_TASK,
            ids::CONVERSATION_COMPLETE,
            ids::URGENCY,
            ids::NEEDS_HUMAN,
            ids::PROJECT,
        ] {
            assert!(questions.contains_key(id), "missing {id}");
        }
        match &questions[ids::TASK] {
            Question::Choice { criteria, .. } => {
                assert!(criteria.contains_key("T1"));
                assert!(criteria.contains_key("T2"));
                assert!(criteria.contains_key(ids::TASK_NONE));
                assert!(criteria.contains_key(ids::TASK_MULTIPLE));
                assert_eq!(criteria.len(), 4);
            }
            other => panic!("task must be a choice, got {other:?}"),
        }
        match &questions[ids::PROJECT] {
            Question::Choice { criteria, .. } => {
                assert!(criteria.contains_key("relay"));
                assert!(criteria.contains_key(ids::PROJECT_UNSURE));
            }
            other => panic!("project must be a choice, got {other:?}"),
        }
    }

    #[test]
    fn a_short_transcript_is_kept_whole() {
        let lines = vec![
            TranscriptLine {
                author: "Alice".into(),
                text: "one".into(),
            },
            TranscriptLine {
                author: "me".into(),
                text: "two".into(),
            },
        ];
        assert_eq!(transcript(&lines), "Alice: one\nme: two");
    }

    #[test]
    fn a_long_transcript_keeps_the_first_and_the_newest_lines() {
        let mut lines = vec![TranscriptLine {
            author: "Alice".into(),
            text: "the original request".into(),
        }];
        for index in 0..500 {
            lines.push(TranscriptLine {
                author: "Alice".into(),
                text: format!("message {index} {}", "x".repeat(40)),
            });
        }
        let out = transcript(&lines);
        assert!(out.len() <= TRANSCRIPT_LIMIT + 2, "{} chars", out.len());
        assert!(out.starts_with("Alice: the original request"));
        assert!(out.contains("message 499"));
        assert!(!out.contains("message 0 "));
    }
}
