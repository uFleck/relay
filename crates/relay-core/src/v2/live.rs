//! Live picker queries and exact reads. Core owns eligibility, the registered-repo allowlist,
//! credential-independent typing and the conversion to the shared picker row contract. The
//! Tauri adapter only resolves credentials and calls these functions; it never decides which
//! provider a query reaches or how a result is ranked.

use serde::{Deserialize, Serialize};

use crate::clickup::ClickUpClient;
use crate::github::{GithubClient, GithubHit, GithubItem};
use crate::provider::ProviderFailure;

use super::reference::{
    GithubItemKind, ReferenceKey, ReferenceSnapshot, V2PickerGroup, V2PickerRow,
};

/// Live work starts after two characters. One character keeps only local results.
pub const LIVE_MIN_CHARS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProgressState {
    Searching,
    Ok,
    Unavailable,
}

/// A quiet row the picker shows while a provider works. A failure carries only a typed reason
/// and a Settings destination; there is no raw provider error and no credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProgress {
    pub provider: String,
    pub state: ProviderProgressState,
    pub reason: Option<String>,
    pub settings_destination: Option<String>,
}

impl ProviderProgress {
    fn searching(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            state: ProviderProgressState::Searching,
            reason: None,
            settings_destination: None,
        }
    }

    fn ok(provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            state: ProviderProgressState::Ok,
            reason: None,
            settings_destination: None,
        }
    }

    fn failed(provider: &str, failure: &ProviderFailure) -> Self {
        Self {
            provider: provider.to_string(),
            state: ProviderProgressState::Unavailable,
            reason: Some(failure.reason_text().to_string()),
            settings_destination: Some(failure.settings_destination(provider)),
        }
    }

    fn setup(provider: &str, reason: &str) -> Self {
        Self {
            provider: provider.to_string(),
            state: ProviderProgressState::Unavailable,
            reason: Some(reason.to_string()),
            settings_destination: Some(format!(
                "Settings → {}",
                match provider {
                    "github" => "GitHub",
                    _ => "ClickUp",
                }
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveQueryResult {
    /// The query this answer belongs to; the UI drops a response whose query changed.
    pub query: String,
    pub rows: Vec<V2PickerRow>,
    pub progress: Vec<ProviderProgress>,
}

/// Credentials and repository scope for one live read. Tokens are borrowed only for the call;
/// they never enter a result, an event or the store.
#[derive(Debug, Clone, Default)]
pub struct LiveCredentials {
    pub github_token: Option<String>,
    pub clickup_token: Option<String>,
    pub registered_repos: Vec<String>,
}

/// Whether a query is eligible for a live provider read. Local results always remain usable,
/// including for queries that are not eligible here.
pub fn live_eligible(query: &str) -> bool {
    bare_query(query).chars().count() >= LIVE_MIN_CHARS
}

fn bare_query(query: &str) -> &str {
    query
        .trim()
        .strip_prefix('@')
        .unwrap_or(query.trim())
        .trim()
}

/// Runs the live reads an eligible query needs. GitHub text search stays inside the registered
/// repos; a qualified or pasted GitHub id is read directly even outside them. ClickUp uses one
/// exact card request and only for a `CU-` id, never a title search.
pub async fn live_query(query: &str, creds: &LiveCredentials, cached_at: &str) -> LiveQueryResult {
    let bare = bare_query(query);
    let mut result = LiveQueryResult {
        query: bare.to_string(),
        ..LiveQueryResult::default()
    };
    if !live_eligible(bare) {
        return result;
    }

    if let Some(id) = clickup_id(bare) {
        result.progress.push(ProviderProgress::searching("clickup"));
        let Some(token) = creds
            .clickup_token
            .as_deref()
            .filter(|t| !t.trim().is_empty())
        else {
            result.progress[0] =
                ProviderProgress::setup("clickup", "Add a ClickUp token for exact card lookup.");
            return result;
        };
        match ClickUpClient::new(token.to_string()) {
            Err(_) => {
                result.progress[0] =
                    ProviderProgress::failed("clickup", &ProviderFailure::Configuration);
            }
            Ok(client) => match client.task(id).await {
                Ok(task) => {
                    result.rows.push(clickup_row(&task, bare, cached_at));
                    result.progress[0] = ProviderProgress::ok("clickup");
                }
                Err(error) => {
                    result.progress[0] = ProviderProgress::failed("clickup", &error.failure())
                }
            },
        }
        return result;
    }

    result.progress.push(ProviderProgress::searching("github"));
    let client = match GithubClient::new(creds.github_token.clone()) {
        Ok(client) => client,
        Err(_) => {
            result.progress[0] =
                ProviderProgress::failed("github", &ProviderFailure::Configuration);
            return result;
        }
    };
    let outcome = if let Some((number, qualifier)) = numeric_query(bare) {
        let mut repos = creds.registered_repos.clone();
        if let Some(qualified) = qualifier {
            repos.retain(|repo| repo.eq_ignore_ascii_case(&qualified));
        }
        numeric_hits(&client, number, &repos).await
    } else if let Some((owner_repo, number)) = typed_github_id(bare) {
        // A pasted or linked id is a direct read and is deliberately not allowlist-bound.
        let (owner, repo) = owner_repo
            .split_once('/')
            .unwrap_or((owner_repo.as_str(), ""));
        client
            .issue_or_pull(owner, repo, number)
            .await
            .map(|hit| vec![hit])
    } else {
        if creds.registered_repos.is_empty() {
            result.progress[0] =
                ProviderProgress::setup("github", "Register a repo to search its issues and PRs.");
            return result;
        }
        client.search_repos(bare, &creds.registered_repos).await
    };

    match outcome {
        Ok(hits) => {
            for hit in hits {
                result
                    .rows
                    .push(github_row(&hit, &creds.registered_repos, bare, cached_at));
            }
            result.progress[0] = ProviderProgress::ok("github");
        }
        Err(error) => result.progress[0] = ProviderProgress::failed("github", &error.failure()),
    }
    result
}

/// The raw provider item behind an exact read. Import refresh needs the original fields to
/// distinguish closed/merged PRs and to compare ClickUp assignees with the discovery account;
/// the picker and recovery worker only need the shared row.
pub enum ExactItem {
    Github(GithubHit),
    Clickup(crate::clickup::ClickUpTask),
}

/// One exact read for a known key, used by the pasted-URL read, the recovery worker and Import
/// refresh. The typed failure is preserved so a worker can schedule a pause or a retry correctly.
pub async fn read_exact_item(
    key: &ReferenceKey,
    creds: &LiveCredentials,
    cached_at: &str,
) -> Result<(V2PickerRow, ExactItem), ProviderFailure> {
    match key {
        ReferenceKey::Github {
            owner_repo,
            number,
            item,
        } => {
            let (owner, repo) = owner_repo
                .split_once('/')
                .ok_or(ProviderFailure::Unavailable)?;
            let client = GithubClient::new(creds.github_token.clone())
                .map_err(|_| ProviderFailure::Configuration)?;
            let hit = match item {
                GithubItemKind::Pull => client.pull_summary(owner, repo, *number).await,
                GithubItemKind::Issue => client.issue_or_pull(owner, repo, *number).await,
            }
            .map_err(|error| error.failure())?;
            Ok((
                github_row(&hit, &creds.registered_repos, "", cached_at),
                ExactItem::Github(hit),
            ))
        }
        ReferenceKey::Clickup { id } => {
            let token = creds
                .clickup_token
                .as_deref()
                .filter(|token| !token.trim().is_empty())
                .ok_or(ProviderFailure::Configuration)?;
            let task = ClickUpClient::new(token.to_string())
                .map_err(|_| ProviderFailure::Configuration)?
                .task(id)
                .await
                .map_err(|error| error.failure())?;
            Ok((clickup_row(&task, "", cached_at), ExactItem::Clickup(task)))
        }
        _ => Err(ProviderFailure::Unavailable),
    }
}

/// The picker and recovery worker's exact read, unchanged in behaviour.
pub async fn read_exact(
    key: &ReferenceKey,
    creds: &LiveCredentials,
    cached_at: &str,
) -> Result<V2PickerRow, ProviderFailure> {
    read_exact_item(key, creds, cached_at)
        .await
        .map(|(row, _)| row)
}

/// Option-returning wrapper for the picker's pasted-URL read; any cached copy stays intact.
pub async fn exact_github(
    key: &ReferenceKey,
    creds: &LiveCredentials,
    cached_at: &str,
) -> Option<V2PickerRow> {
    read_exact(key, creds, cached_at).await.ok()
}

/// Option-returning wrapper for the picker's exact card read.
pub async fn exact_clickup(
    key: &ReferenceKey,
    creds: &LiveCredentials,
    cached_at: &str,
) -> Option<V2PickerRow> {
    read_exact(key, creds, cached_at).await.ok()
}

fn clickup_id(bare: &str) -> Option<&str> {
    if let Some(id) = bare.strip_prefix("CU-") {
        return (!id.is_empty()).then_some(id);
    }
    if bare.len() > 3 && bare[..3].eq_ignore_ascii_case("cu-") {
        return Some(&bare[3..]);
    }
    None
}

/// `#142` is a bare number; `#142 sample/app` narrows it to one repo. A qualified number is
/// also a direct read, so the response's pull marker decides the kind.
fn numeric_query(bare: &str) -> Option<(u64, Option<String>)> {
    let mut words = bare.split_whitespace();
    let first = words.next()?.trim_start_matches('#');
    let number = first.parse::<u64>().ok()?;
    let qualifier = words
        .find(|word| word.contains('/'))
        .map(|word| word.trim().to_ascii_lowercase());
    Some((number, qualifier))
}

fn typed_github_id(bare: &str) -> Option<(String, u64)> {
    let (owner_repo, number) = bare.rsplit_once('#')?;
    if owner_repo.contains('/') && !owner_repo.starts_with('#') {
        let number = number.parse::<u64>().ok()?;
        return Some((owner_repo.trim().to_ascii_lowercase(), number));
    }
    None
}

async fn numeric_hits(
    client: &GithubClient,
    number: u64,
    repos: &[String],
) -> Result<Vec<GithubHit>, crate::github::GithubError> {
    use crate::github::MAX_SEARCH_REPOS;

    let mut hits = Vec::new();
    for repo in repos.iter().take(MAX_SEARCH_REPOS) {
        let (owner, name) = repo.split_once('/').unwrap_or((repo.as_str(), ""));
        match client.issue_or_pull(owner, name, number).await {
            Ok(hit) => hits.push(hit),
            // An absent number in one repo is not a failed query.
            Err(crate::github::GithubError::Status { status: 404, .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(hits)
}

fn github_alias(owner_repo: &str, repos: &[String]) -> Option<String> {
    let (_, repo) = owner_repo.rsplit_once('/')?;
    let count = repos
        .iter()
        .filter(|candidate| {
            candidate
                .rsplit_once('/')
                .is_some_and(|(_, name)| name.eq_ignore_ascii_case(repo))
        })
        .count();
    (count > 1).then(|| owner_repo.split('/').next().unwrap_or("").to_string())
}

fn exact_hit(needle: &str, values: &[&str]) -> bool {
    let needle = needle.trim_start_matches('@').to_ascii_lowercase();
    !needle.is_empty()
        && values
            .iter()
            .any(|value| value.to_ascii_lowercase() == needle)
}

fn github_row(hit: &GithubHit, repos: &[String], needle: &str, cached_at: &str) -> V2PickerRow {
    let item = match hit.kind {
        GithubItem::Pull => GithubItemKind::Pull,
        GithubItem::Issue => GithubItemKind::Issue,
    };
    let key = ReferenceKey::Github {
        owner_repo: hit.owner_repo.clone(),
        number: hit.number,
        item,
    };
    let mut metadata = vec![format!("repo {}", hit.owner_repo)];
    if hit.merged {
        metadata.push("merged".to_string());
    } else if hit.draft {
        metadata.push("draft".to_string());
    }
    if let Some(updated) = &hit.updated_at {
        metadata.push(format!("updated {updated}"));
    }
    let snapshot = ReferenceSnapshot::from_provider(&hit.title, &hit.state, metadata, cached_at);
    let alias = github_alias(&hit.owner_repo, repos);
    let mut row = V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(alias.as_deref()),
        canonical_link: key.canonical_url(),
        availability: super::reference::ReferenceAvailability::Ready,
        group: match item {
            GithubItemKind::Pull => V2PickerGroup::Prs,
            GithubItemKind::Issue => V2PickerGroup::Issues,
        },
        title: hit.title.clone(),
        state: Some(hit.state.clone()),
        metadata: snapshot.metadata.clone(),
        snapshot: Some(snapshot),
        unavailable_reason: None,
        settings_destination: None,
        updated_at: hit
            .updated_at
            .clone()
            .unwrap_or_else(|| cached_at.to_string()),
        exact_match: false,
    };
    let values = [
        row.key
            .split_once(':')
            .map(|(_, t)| t)
            .unwrap_or(&row.key)
            .to_string(),
        row.display_id.trim_start_matches('@').to_string(),
        row.title.clone(),
    ];
    row.exact_match = exact_hit(
        needle,
        &values.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    row
}

fn clickup_row(task: &crate::clickup::ClickUpTask, needle: &str, cached_at: &str) -> V2PickerRow {
    let key = ReferenceKey::Clickup {
        id: task.id.clone(),
    };
    let state = task.status.clone().unwrap_or_else(|| "open".to_string());
    let mut metadata = Vec::new();
    if let Some(list) = &task.list_name {
        metadata.push(format!("list {list}"));
    }
    if !task.assignee_ids.is_empty() {
        metadata.push(format!("assignee {}", task.assignee_ids[0]));
    }
    if let Some(updated) = &task.date_updated {
        metadata.push(format!("updated {updated}"));
    }
    let snapshot = ReferenceSnapshot::from_provider(&task.name, &state, metadata, cached_at);
    let mut row = V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(None),
        canonical_link: task.url.clone().unwrap_or_else(|| key.canonical_url()),
        availability: super::reference::ReferenceAvailability::Ready,
        group: V2PickerGroup::Cards,
        title: task.name.clone(),
        state: Some(state),
        metadata: snapshot.metadata.clone(),
        snapshot: Some(snapshot),
        unavailable_reason: None,
        settings_destination: None,
        updated_at: task
            .date_updated
            .clone()
            .unwrap_or_else(|| cached_at.to_string()),
        exact_match: false,
    };
    let values = [
        task.id.clone(),
        row.display_id.trim_start_matches('@').to_string(),
        row.title.clone(),
    ];
    row.exact_match = exact_hit(
        needle,
        &values.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_and_query_shapes() {
        assert!(!live_eligible("@"));
        assert!(!live_eligible("@a"));
        assert!(live_eligible("@#142"));
        assert_eq!(clickup_id("@CU-8x1"), None);
        assert_eq!(clickup_id("CU-8x1"), Some("8x1"));
        assert_eq!(
            numeric_query("#142 sample/app"),
            Some((142, Some("sample/app".into())))
        );
        assert_eq!(numeric_query("fix the chip"), None);
        assert_eq!(
            typed_github_id("sample/app#142"),
            Some(("sample/app".into(), 142))
        );
    }

    #[tokio::test]
    async fn missing_clickup_credential_reports_immediately_without_a_request() {
        let result = live_query(
            "CU-8x1",
            &LiveCredentials {
                clickup_token: None,
                ..LiveCredentials::default()
            },
            "2026-11-21T00:00:00.000Z",
        )
        .await;
        assert!(result.rows.is_empty());
        assert_eq!(result.progress.len(), 1);
        assert_eq!(result.progress[0].provider, "clickup");
        assert_eq!(result.progress[0].state, ProviderProgressState::Unavailable);
        assert_eq!(
            result.progress[0].settings_destination.as_deref(),
            Some("Settings → ClickUp")
        );
    }

    #[tokio::test]
    async fn one_character_stays_local() {
        let result = live_query("@a", &LiveCredentials::default(), "now").await;
        assert!(result.rows.is_empty());
        assert!(result.progress.is_empty());
    }
}
