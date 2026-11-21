//! Create-only Note Import contract (#31). Discovery produces candidates from the existing
//! provider reads; the store commits each canonical target independently. Live state comes from
//! #30's exact reads through `import_refresh`, never from discovery, toggles or a Note.

use serde::{Deserialize, Serialize};

use crate::clickup::ClickUpTask;
use crate::github::GithubHit;
use crate::model::from_unix_millis;
use crate::provider::ProviderFailure;

use super::recovery::{RecoveryReason, next_delay_ms};
use super::reference::{ReferenceKey, ReferenceSnapshot};

/// Authenticated GitHub PRs are refreshed every three minutes; ClickUp cards every ten.
pub const GITHUB_REFRESH_MS: i64 = 3 * 60 * 1000;
pub const CLICKUP_REFRESH_MS: i64 = 10 * 60 * 1000;

/// One provider item that Import may create a Note for. Presentation fields are captured once
/// from the provider read; identity comes from [`ReferenceKey`] and drives the ledger key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCandidate {
    pub key: ReferenceKey,
    /// `clickup` or `github`; the first half of the durable ledger key.
    pub source_kind: String,
    /// The provider's exact title, including on review-requested PRs.
    pub title: String,
    pub canonical_url: String,
    /// The read-only Lens snapshot seeded at import time.
    pub snapshot_json: Option<String>,
    /// ClickUp account id captured during discovery, used to report reassignment separately
    /// from the card's workflow state. GitHub candidates leave this unset.
    pub account_id: Option<String>,
}

impl ImportCandidate {
    pub fn identity(&self) -> String {
        self.key.identity()
    }

    /// `(source_kind, target_id)` is the unique Import ledger key.
    pub fn ledger_key(&self) -> (String, String) {
        (self.source_kind.clone(), self.key.target_id())
    }

    pub fn provider(&self) -> &str {
        &self.source_kind
    }

    pub fn refresh_ms(&self) -> i64 {
        match self.source_kind.as_str() {
            "github" => GITHUB_REFRESH_MS,
            _ => CLICKUP_REFRESH_MS,
        }
    }
}

/// The outcome of one transactional create attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportOutcome {
    Created { note_id: String },
    AlreadyImported,
}

/// Overlapping discovery results (author plus review request) and duplicate repo registrations
/// collapse to one candidate by canonical identity, keeping the first occurrence.
pub fn dedupe_candidates(candidates: Vec<ImportCandidate>) -> Vec<ImportCandidate> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.identity()))
        .collect()
}

/// One due exact read from `import_refresh`. All fields are owned so the worker releases the
/// SQLite connection before awaiting a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRefreshTarget {
    pub key: String,
    pub kind: String,
    pub target_id: String,
    pub canonical_url: String,
    pub provider: String,
    pub account_id: Option<String>,
    pub attempts: i64,
    pub next_due_at: String,
    pub next_due_ms: i64,
    pub reason: Option<String>,
    pub updated_at: String,
}

/// The worker's decision, applied by `V2Store::record_import_refresh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshUpdate {
    pub attempts: i64,
    pub next_due_at: String,
    pub next_due_ms: i64,
    pub reason: Option<String>,
}

/// The only states an exact read may write to an imported Reference's snapshot. Discovery list
/// absence, a truncated page and a request failure never produce one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedState {
    Open,
    Closed,
    Merged,
    Reassigned,
}

impl ImportedState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
            Self::Reassigned => "unassigned",
        }
    }
}

/// A PR detail read distinguishes open, closed and merged. Draft stays open; there is no
/// unassignment concept for a PR, so reassignment can only come from a ClickUp detail read.
pub fn github_imported_state(hit: &GithubHit) -> ImportedState {
    if hit.merged {
        ImportedState::Merged
    } else if hit.state.eq_ignore_ascii_case("closed") {
        ImportedState::Closed
    } else {
        ImportedState::Open
    }
}

/// A ClickUp detail read compares assignees with the account id captured during discovery.
/// Reassignment is reported separately from the workflow state; when no account id was captured
/// the read cannot establish unassignment, so it stays silent about it.
pub fn clickup_imported_state(task: &ClickUpTask, account_id: Option<&str>) -> ImportedState {
    let reassigned = match account_id {
        Some(id) if !id.is_empty() && !task.assignee_ids.is_empty() => {
            !task.assignee_ids.iter().any(|candidate| candidate == id)
        }
        _ => false,
    };
    if reassigned {
        return ImportedState::Reassigned;
    }
    if matches!(task.status_type.as_deref(), Some("done" | "closed")) {
        return ImportedState::Closed;
    }
    ImportedState::Open
}

/// A failed refresh uses #30's backoff, capped at 30 minutes, with a rate-limit deadline
/// taking precedence. Sign-in and configuration failures pause until Settings changes.
pub fn refresh_failure_update(
    attempts_after: i64,
    failure: &ProviderFailure,
    now_ms: i64,
) -> RefreshUpdate {
    let rate_limit_until_ms = match failure {
        ProviderFailure::RateLimited { retry_after_ms } => *retry_after_ms,
        _ => None,
    };
    let next_due_ms = next_delay_ms(attempts_after, rate_limit_until_ms, now_ms);
    RefreshUpdate {
        attempts: attempts_after,
        next_due_at: from_unix_millis(next_due_ms).unwrap_or_else(crate::model::now),
        next_due_ms,
        reason: Some(RecoveryReason::from_failure(failure).as_str().to_string()),
    }
}

/// A successful read reschedules at the provider cadence rather than clearing the schedule.
pub fn refresh_success_update(provider: &str, now_ms: i64) -> RefreshUpdate {
    let cadence = if provider == "github" {
        GITHUB_REFRESH_MS
    } else {
        CLICKUP_REFRESH_MS
    };
    let next_due_ms = now_ms.saturating_add(cadence);
    RefreshUpdate {
        attempts: 0,
        next_due_at: from_unix_millis(next_due_ms).unwrap_or_else(crate::model::now),
        next_due_ms,
        reason: None,
    }
}

/// Applies only the read-only Lens state and Relay's fetch time. Title, metadata and identity
/// survive unchanged; this never touches a Note, a Task or the Import ledger.
pub fn refresh_snapshot(
    previous: &ReferenceSnapshot,
    state: ImportedState,
    cached_at: &str,
) -> ReferenceSnapshot {
    let mut next = previous.clone();
    next.state = Some(state.as_str().to_string());
    next.cached_at = Some(cached_at.to_string());
    next.reason = None;
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str) -> ImportCandidate {
        ImportCandidate {
            key: ReferenceKey::Clickup { id: id.into() },
            source_kind: "clickup".into(),
            title: format!("Card {id}"),
            canonical_url: format!("https://app.clickup.com/t/{id}"),
            snapshot_json: None,
            account_id: Some("42".into()),
        }
    }

    #[test]
    fn dedupe_collapses_overlapping_roles_and_keeps_the_first() {
        let mut a = candidate("1");
        a.title = "Authored".into();
        let mut b = candidate("1");
        b.title = "Review requested".into();
        let deduped = dedupe_candidates(vec![a, b, candidate("2")]);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].title, "Authored");
    }

    #[test]
    fn ledger_key_is_the_canonical_target_id() {
        let (kind, target) = candidate("8x1").ledger_key();
        assert_eq!(kind, "clickup");
        assert_eq!(target, "CU-8x1");
        assert_eq!(candidate("8x1").refresh_ms(), CLICKUP_REFRESH_MS);
    }

    #[test]
    fn pull_states_distinguish_open_closed_and_merged() {
        let hit = |state: &str, merged| GithubHit {
            owner_repo: "acme/relay".into(),
            number: 7,
            title: "PR".into(),
            url: "https://github.com/acme/relay/pull/7".into(),
            kind: crate::github::GithubItem::Pull,
            state: state.into(),
            updated_at: None,
            draft: false,
            merged,
        };
        assert_eq!(
            github_imported_state(&hit("open", false)),
            ImportedState::Open
        );
        assert_eq!(
            github_imported_state(&hit("closed", false)),
            ImportedState::Closed
        );
        assert_eq!(
            github_imported_state(&hit("closed", true)),
            ImportedState::Merged
        );
    }

    #[test]
    fn reassignment_is_separate_from_workflow_state() {
        let task = |assignees: &[&str], status_type: &str| ClickUpTask {
            id: "8x1".into(),
            name: "Card".into(),
            description: None,
            status: Some("doing".into()),
            status_type: Some(status_type.into()),
            list_name: None,
            due_date: None,
            priority: None,
            url: None,
            date_updated: None,
            assignee_ids: assignees.iter().map(|id| id.to_string()).collect(),
        };
        assert_eq!(
            clickup_imported_state(&task(&["42"], "custom"), Some("42")),
            ImportedState::Open
        );
        assert_eq!(
            clickup_imported_state(&task(&["77"], "custom"), Some("42")),
            ImportedState::Reassigned
        );
        assert_eq!(
            clickup_imported_state(&task(&["77"], "done"), Some("42")),
            ImportedState::Reassigned
        );
        assert_eq!(
            clickup_imported_state(&task(&["77"], "done"), None),
            ImportedState::Closed
        );
        // An empty assignee list from a partial read cannot establish unassignment.
        assert_eq!(
            clickup_imported_state(&task(&[], "custom"), Some("42")),
            ImportedState::Open
        );
    }

    #[test]
    fn refresh_only_changes_state_and_cache_time() {
        let previous = ReferenceSnapshot::from_provider(
            "Card",
            "open",
            vec!["list Sprint".into()],
            "2026-11-21T00:00:00.000Z",
        );
        let next = refresh_snapshot(&previous, ImportedState::Merged, "2026-11-21T00:03:00.000Z");
        assert_eq!(next.title.as_deref(), Some("Card"));
        assert_eq!(next.metadata, vec!["list Sprint"]);
        assert_eq!(next.state.as_deref(), Some("merged"));
        assert_eq!(next.cached_at.as_deref(), Some("2026-11-21T00:03:00.000Z"));
        assert_eq!(
            previous.state.as_deref(),
            Some("open"),
            "the input is not mutated"
        );
    }
}
