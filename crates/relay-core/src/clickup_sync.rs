//! ClickUp "cards assigned to me" policy: which Relay status a ClickUp status maps to, and
//! what a sync run must create, move or archive. Pure; the adapter applies the plan.

use std::collections::{HashMap, HashSet};

use crate::clickup::{AssignedTasks, ClickUpTask};
use crate::model::StatusCategory;
use crate::v2::{ImportCandidate, ReferenceKey, ReferenceSnapshot, dedupe_candidates};

/// Keyword groups, tried in order, matched against the start of each word of the
/// normalized status name. Anything unmatched (todo, analyzing, open) is `Todo`.
const KEYWORDS: [(StatusCategory, &[&str]); 6] = [
    (
        StatusCategory::Active,
        &["doing", "progress", "develop", "wip"],
    ),
    (StatusCategory::Review, &["review", "qa", "test"]),
    (StatusCategory::Deploying, &["deploy"]),
    (StatusCategory::Blocked, &["impediment", "blocked", "block"]),
    (StatusCategory::Done, &["done", "closed", "complete"]),
    (
        StatusCategory::Backlog,
        &["inbox", "backlog", "epic", "icebox"],
    ),
];

/// ClickUp statuses carry emoji and free text ("👨‍💻 doing"), so Relay maps by category, not
/// by name. `status_type` `done`/`closed` wins over the name.
pub fn clickup_category(status_name: &str, status_type: Option<&str>) -> StatusCategory {
    if matches!(status_type, Some("closed" | "done")) {
        return StatusCategory::Done;
    }
    let normalized: String = status_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    for (category, keywords) in KEYWORDS {
        let hit = normalized
            .split_whitespace()
            .any(|word| keywords.iter().any(|keyword| word.starts_with(keyword)));
        if hit {
            return category;
        }
    }
    StatusCategory::Todo
}

pub fn card_category(card: &ClickUpTask) -> StatusCategory {
    clickup_category(
        card.status.as_deref().unwrap_or_default(),
        card.status_type.as_deref(),
    )
}

/// A ClickUp-linked task already in the target project.
#[derive(Debug, Clone)]
pub struct LinkedTask {
    pub task_id: String,
    pub link_id: String,
    pub external_id: String,
    /// The ClickUp status name stored on the link at the last sync.
    pub link_state: Option<String>,
    /// Whether an assigned-cards sync has already mapped this link's status. Links from the
    /// old list import have not, and their status was guessed by exact name.
    pub mapped: bool,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardUpdate {
    pub task_id: String,
    pub link_id: String,
    pub title: String,
    pub state: Option<String>,
    /// Move the task to this category's status. `None` keeps the status the user set.
    pub category: Option<StatusCategory>,
    pub unarchive: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub create: Vec<ClickUpTask>,
    pub update: Vec<CardUpdate>,
    pub archive: Vec<String>,
}

/// `complete` is false when the assigned list was cut off: nothing may be archived then,
/// because a missing card might just be on a page that was never read.
pub fn reconcile(assigned: &[ClickUpTask], existing: &[LinkedTask], complete: bool) -> Plan {
    let by_card: HashMap<&str, &LinkedTask> = existing
        .iter()
        .map(|linked| (linked.external_id.as_str(), linked))
        .collect();
    let mut plan = Plan::default();
    let mut seen = HashSet::new();

    for card in assigned {
        if !seen.insert(card.id.as_str()) {
            continue;
        }
        let Some(linked) = by_card.get(card.id.as_str()) else {
            // ClickUp keeps finished cards in the open listing; importing them only buries
            // the work in progress.
            if card_category(card) != StatusCategory::Done {
                plan.create.push(card.clone());
            }
            continue;
        };
        let state_changed = !linked.mapped || linked.link_state != card.status;
        if !state_changed && !linked.archived {
            continue;
        }
        plan.update.push(CardUpdate {
            task_id: linked.task_id.clone(),
            link_id: linked.link_id.clone(),
            title: card.name.clone(),
            state: card.status.clone(),
            category: (state_changed || linked.archived).then(|| card_category(card)),
            unarchive: linked.archived,
        });
    }

    if complete {
        plan.archive = existing
            .iter()
            .filter(|linked| !linked.archived && !seen.contains(linked.external_id.as_str()))
            .map(|linked| linked.task_id.clone())
            .collect();
    }
    plan
}

/// Builds create-only Note Import candidates from the verified `me → assigned_tasks` read.
/// The workspace-wide query is already done by the caller, so a guest with no spaces still
/// contributes cards. Pagination and truncation reporting stay with the provider read; a
/// truncated page only limits what is offered now, it never archives or unassigns anything.
/// Finished cards keep v1's scope and are not imported.
pub fn import_candidates(
    assigned: &AssignedTasks,
    account_id: &str,
    cached_at: &str,
) -> Vec<ImportCandidate> {
    dedupe_candidates(
        assigned
            .tasks
            .iter()
            .filter(|card| card_category(card) != StatusCategory::Done)
            .map(|card| {
                let key = ReferenceKey::Clickup {
                    id: card.id.clone(),
                };
                let state = card.status.clone().unwrap_or_else(|| "open".to_string());
                let mut metadata = Vec::new();
                if let Some(list) = &card.list_name {
                    metadata.push(format!("list {list}"));
                }
                if let Some(assignee) = card.assignee_ids.first() {
                    metadata.push(format!("assignee {assignee}"));
                }
                if let Some(updated) = &card.date_updated {
                    metadata.push(format!("updated {updated}"));
                }
                let snapshot =
                    ReferenceSnapshot::from_provider(&card.name, &state, metadata, cached_at);
                ImportCandidate {
                    canonical_url: card.url.clone().unwrap_or_else(|| key.canonical_url()),
                    key,
                    source_kind: "clickup".to_string(),
                    title: card.name.clone(),
                    snapshot_json: Some(snapshot.to_json()),
                    account_id: Some(account_id.to_string()),
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, status: &str) -> ClickUpTask {
        ClickUpTask {
            id: id.into(),
            name: format!("card {id}"),
            description: None,
            status: Some(status.into()),
            status_type: Some("custom".into()),
            list_name: None,
            due_date: None,
            priority: None,
            url: None,
            date_updated: None,
            assignee_ids: Vec::new(),
        }
    }

    fn linked(id: &str, state: &str) -> LinkedTask {
        LinkedTask {
            task_id: format!("t-{id}"),
            link_id: format!("l-{id}"),
            external_id: id.into(),
            link_state: Some(state.into()),
            mapped: true,
            archived: false,
        }
    }

    #[test]
    fn real_workspace_statuses_map_by_category() {
        use StatusCategory::*;
        let cases = [
            ("\u{1f468}\u{200d}\u{1f4bb} doing", Active),
            ("\u{1f91d} review", Review),
            ("\u{1f680} waiting deploy", Deploying),
            ("\u{1f6d1} impediment", Blocked),
            ("\u{1f4e6} inbox", Backlog),
            ("\u{1f48e} todo", Todo),
            ("\u{1f4cf}analyzing", Todo),
            ("\u{1f3af} epics", Backlog),
            ("epics history", Backlog),
            ("done", Done),
            ("in progress", Active),
            ("QA testing", Review),
            ("", Todo),
        ];
        for (name, expected) in cases {
            assert_eq!(clickup_category(name, Some("custom")), expected, "{name}");
        }
        assert_eq!(clickup_category("shipped", Some("closed")), Done);
        assert_eq!(clickup_category("anything", Some("done")), Done);
    }

    #[test]
    fn creates_unseen_cards_and_archives_the_unassigned_ones() {
        let plan = reconcile(
            &[card("1", "doing"), card("2", "todo")],
            &[linked("2", "todo"), linked("3", "doing")],
            true,
        );
        assert_eq!(plan.create.len(), 1);
        assert_eq!(plan.create[0].id, "1");
        assert!(plan.update.is_empty());
        assert_eq!(plan.archive, vec!["t-3".to_string()]);
    }

    #[test]
    fn a_status_change_in_clickup_moves_the_task_but_an_unchanged_one_keeps_the_local_status() {
        let plan = reconcile(
            &[card("1", "\u{1f91d} review"), card("2", "doing")],
            &[linked("1", "doing"), linked("2", "doing")],
            true,
        );
        assert_eq!(plan.update.len(), 1);
        let update = &plan.update[0];
        assert_eq!(update.task_id, "t-1");
        assert_eq!(update.category, Some(StatusCategory::Review));
        assert_eq!(update.state.as_deref(), Some("\u{1f91d} review"));
        assert!(plan.archive.is_empty());
    }

    #[test]
    fn links_never_mapped_by_a_sync_get_their_status_fixed_once() {
        let mut old = linked("1", "doing");
        old.mapped = false;
        let plan = reconcile(&[card("1", "doing")], &[old], true);
        assert_eq!(plan.update[0].category, Some(StatusCategory::Active));
    }

    #[test]
    fn a_card_assigned_again_is_unarchived_instead_of_duplicated() {
        let mut gone = linked("1", "doing");
        gone.archived = true;
        let plan = reconcile(&[card("1", "doing")], &[gone], true);
        assert!(plan.create.is_empty());
        assert!(plan.update[0].unarchive);
        assert_eq!(plan.update[0].category, Some(StatusCategory::Active));
    }

    #[test]
    fn finished_cards_are_not_imported_but_existing_ones_still_move_to_done() {
        let plan = reconcile(
            &[card("1", "done"), card("2", "done")],
            &[linked("2", "doing")],
            true,
        );
        assert!(plan.create.is_empty());
        assert_eq!(plan.update[0].category, Some(StatusCategory::Done));
        assert!(plan.archive.is_empty());
    }

    #[test]
    fn a_truncated_list_archives_nothing() {
        let plan = reconcile(&[card("1", "doing")], &[linked("9", "doing")], false);
        assert!(plan.archive.is_empty());
    }

    #[test]
    fn import_candidates_keep_exact_titles_and_skip_finished_cards() {
        let mut titled = card("1", "doing");
        titled.name = "! Ship the RC".into();
        let assigned = AssignedTasks {
            tasks: vec![titled, card("2", "done")],
            truncated: true,
        };
        let candidates = import_candidates(&assigned, "42", "2026-11-21T00:00:00.000Z");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].title, "! Ship the RC");
        assert_eq!(candidates[0].account_id.as_deref(), Some("42"));
        assert_eq!(candidates[0].key.identity(), "clickup:1");
    }
}
