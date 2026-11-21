//! GitHub pull-request state for linked tasks (PLAN 3.5). Read-only, github.com only.
//! A token is optional; without one the public API is used and rate limits apply.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::{ProviderFailure, failure_from_status, parse_reset_seconds};
use crate::repos::{GithubRepo, parse_github_remote};

pub const DEFAULT_BASE_URL: &str = "https://api.github.com";
const TIMEOUT_SECS: u64 = 15;
/// A picker query touches at most this many registered repos, and returns at most this many
/// rows, so a wide project registry cannot turn one keystroke into an unbounded fan-out.
pub const MAX_SEARCH_REPOS: usize = 10;
pub const MAX_SEARCH_RESULTS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("github request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("github answered {status}: {message}")]
    Status {
        status: u16,
        message: String,
        /// `x-ratelimit-reset` converted to Unix milliseconds, when present.
        rate_limit_reset_ms: Option<i64>,
    },
    #[error("github response is missing {0}")]
    Missing(&'static str),
    #[error("github json: {0}")]
    Json(#[from] serde_json::Error),
}

impl GithubError {
    /// A safe, typed outcome for the picker and recovery worker. Raw messages never escape.
    pub fn failure(&self) -> ProviderFailure {
        match self {
            Self::Status {
                status,
                message,
                rate_limit_reset_ms,
            } => failure_from_status(*status, message, *rate_limit_reset_ms),
            Self::Http(_) | Self::Missing(_) | Self::Json(_) => ProviderFailure::Unavailable,
        }
    }
}

/// PR or issue, as distinguished by the GitHub response's pull marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubItem {
    Pull,
    Issue,
}

impl GithubItem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Issue => "issue",
        }
    }
}

/// One live read result, bounded and allowlist-checked before it reaches the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubHit {
    pub owner_repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub kind: GithubItem,
    pub state: String,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checks {
    None,
    Pending,
    Passing,
    Failing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    None,
    Pending,
    Approved,
    ChangesRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    /// `open` or `closed`.
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub title: String,
    pub url: String,
    pub head_sha: String,
    pub checks: Checks,
    pub review: Review,
}

pub struct GithubClient {
    token: Option<String>,
    http: reqwest::Client,
    base: String,
}

/// One open issue, without pull requests (the issues endpoint returns both).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubIssue {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub url: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRole {
    Author,
    ReviewRequested,
}

/// An open pull request found by search, before it is linked to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundPull {
    pub repo: GithubRepo,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub role: PullRole,
}

impl std::fmt::Debug for GithubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubClient")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("base", &self.base)
            .finish()
    }
}

impl GithubClient {
    pub fn new(token: Option<String>) -> Result<Self, GithubError> {
        Self::with_base(token, DEFAULT_BASE_URL.to_string())
    }

    pub fn with_base(token: Option<String>, base: String) -> Result<Self, GithubError> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .user_agent("relay")
            .build()?;
        Ok(Self {
            token: token.filter(|token| !token.trim().is_empty()),
            http,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    /// The PR, its head commit's check runs and the latest review state.
    pub async fn pull_request(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, GithubError> {
        let pull = self
            .get(&format!("/repos/{owner}/{repo}/pulls/{number}"))
            .await?;
        let title = pull
            .get("title")
            .and_then(Value::as_str)
            .ok_or(GithubError::Missing("title"))?
            .to_string();
        let url = pull
            .get("html_url")
            .and_then(Value::as_str)
            .ok_or(GithubError::Missing("html_url"))?
            .to_string();
        let head_sha = pull
            .pointer("/head/sha")
            .and_then(Value::as_str)
            .ok_or(GithubError::Missing("head.sha"))?
            .to_string();
        let state = pull
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("open")
            .to_string();
        let draft = pull.get("draft").and_then(Value::as_bool).unwrap_or(false);
        let merged = pull.get("merged").and_then(Value::as_bool).unwrap_or(false);

        let checks = match self
            .get(&format!(
                "/repos/{owner}/{repo}/commits/{head_sha}/check-runs"
            ))
            .await
        {
            Ok(value) => checks_from(&value),
            Err(GithubError::Status { status, .. }) if status == 403 || status == 404 => {
                Checks::None
            }
            Err(error) => return Err(error),
        };
        let review = match self
            .get(&format!("/repos/{owner}/{repo}/pulls/{number}/reviews"))
            .await
        {
            Ok(value) => review_from(&value),
            Err(GithubError::Status { status, .. }) if status == 403 || status == 404 => {
                Review::None
            }
            Err(error) => return Err(error),
        };

        Ok(PullRequest {
            number,
            state,
            draft,
            merged,
            title,
            url,
            head_sha,
            checks,
            review,
        })
    }

    /// A cheap authenticated call that proves a token (or the public rate limit).
    pub async fn check(&self) -> Result<(), GithubError> {
        self.get("/rate_limit").await.map(|_| ())
    }

    /// Open issues for a repository, up to 10 pages of 100, pull requests excluded.
    pub async fn list_issues(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<GithubIssue>, GithubError> {
        let mut out = Vec::new();
        for page in 1..=10 {
            let value = self
                .get(&format!(
                    "/repos/{owner}/{repo}/issues?state=open&per_page=100&page={page}"
                ))
                .await?;
            let items = value.as_array().ok_or(GithubError::Missing("issues"))?;
            out.extend(items.iter().filter_map(issue_from));
            if items.len() < 100 {
                break;
            }
        }
        Ok(out)
    }

    /// Open pull requests the token's user authored or was asked to review. One page of
    /// 100 per query is enough for a person's open work.
    pub async fn search_my_pulls(&self) -> Result<Vec<FoundPull>, GithubError> {
        let mut out = Vec::new();
        for (query, role) in [
            ("is:pr+is:open+author:@me", PullRole::Author),
            (
                "is:pr+is:open+review-requested:@me",
                PullRole::ReviewRequested,
            ),
        ] {
            let value = self
                .get(&format!("/search/issues?q={query}&per_page=100"))
                .await?;
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .ok_or(GithubError::Missing("items"))?;
            out.extend(items.iter().filter_map(|item| found_from(item, role)));
        }
        Ok(out)
    }

    async fn get(&self, path: &str) -> Result<Value, GithubError> {
        let mut request = self
            .http
            .get(format!("{}{}", self.base, path))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_reset_seconds_for_str);
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(GithubError::Status {
                status,
                message,
                rate_limit_reset_ms: reset,
            });
        }
        Ok(response.json().await?)
    }

    /// Repo-scoped issue and PR text search. User text is encoded as search text (never as a
    /// query operator) and results outside the registered allowlist are dropped. A qualified
    /// number like `#142 sample/app` narrows the search to that repo before the call.
    pub async fn search_repos(
        &self,
        query: &str,
        repos: &[String],
    ) -> Result<Vec<GithubHit>, GithubError> {
        let (text, qualified) = split_search_query(query);
        if text.is_empty() && qualified.is_none() {
            return Ok(Vec::new());
        }
        let allowlist: Vec<String> = qualified
            .into_iter()
            .chain(repos.iter().map(|repo| repo.trim().to_ascii_lowercase()))
            .filter(|repo| repo.contains('/'))
            .take(MAX_SEARCH_REPOS)
            .collect();
        if allowlist.is_empty() {
            return Ok(Vec::new());
        }
        let mut terms = vec![text];
        terms.extend(allowlist.iter().map(|repo| format!("repo:{repo}")));
        let path = format!(
            "/search/issues?q={}&per_page={}",
            encode_search(&terms.join(" ")),
            MAX_SEARCH_RESULTS
        );
        let value = self.get(&path).await?;
        let items = value
            .get("items")
            .and_then(Value::as_array)
            .ok_or(GithubError::Missing("items"))?;
        let allowed: Vec<&str> = allowlist.iter().map(String::as_str).collect();
        Ok(items
            .iter()
            .filter_map(|item| hit_from_search(item, &allowed))
            .take(MAX_SEARCH_RESULTS)
            .collect())
    }

    /// Direct read of one issue or PR number. The issues endpoint answers both and marks PRs,
    /// which is how a bare `#142` is classified without a second request. No allowlist here:
    /// a pasted or linked id stays readable even outside the registered repo set.
    pub async fn issue_or_pull(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<GithubHit, GithubError> {
        let value = self
            .get(&format!("/repos/{owner}/{repo}/issues/{number}"))
            .await?;
        hit_from_issue(&value, &format!("{owner}/{repo}").to_ascii_lowercase())
            .ok_or(GithubError::Missing("issue"))
    }

    /// Lightweight PR read for a direct GitHub id: one request with merged and draft state,
    /// without the checks and reviews the v1 poller needs.
    pub async fn pull_summary(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<GithubHit, GithubError> {
        let value = self
            .get(&format!("/repos/{owner}/{repo}/pulls/{number}"))
            .await?;
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .ok_or(GithubError::Missing("title"))?
            .to_string();
        let url = value
            .get("html_url")
            .and_then(Value::as_str)
            .ok_or(GithubError::Missing("html_url"))?
            .to_string();
        let closed = value.get("state").and_then(Value::as_str) == Some("closed");
        let merged = value
            .get("merged")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let draft = value.get("draft").and_then(Value::as_bool).unwrap_or(false);
        let state = if merged {
            "merged"
        } else if closed {
            "closed"
        } else if draft {
            "draft"
        } else {
            "open"
        };
        Ok(GithubHit {
            owner_repo: format!("{owner}/{repo}").to_ascii_lowercase(),
            number,
            title,
            url,
            kind: GithubItem::Pull,
            state: state.to_string(),
            updated_at: value
                .get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            draft,
            merged,
        })
    }
}

fn parse_reset_seconds_for_str(value: &str) -> Option<i64> {
    parse_reset_seconds(Some(value))
}

/// Splits `@#142 sample/app` into the numeric text and an optional owner/repo qualifier.
fn split_search_query(query: &str) -> (String, Option<String>) {
    let mut words = query.split_whitespace();
    let first = words.next().unwrap_or("").trim();
    let mut number = first;
    let mut qualifier = None;
    if first.starts_with('#') {
        number = first.trim_start_matches('#');
    }
    for word in words {
        let trimmed = word.trim();
        if trimmed.contains('/') && !qualifier.is_some() {
            qualifier = Some(
                trimmed
                    .trim_matches(|c: char| {
                        !c.is_ascii_alphanumeric() && c != '/' && c != '-' && c != '_'
                    })
                    .to_ascii_lowercase(),
            );
        } else if number.is_empty() {
            number = trimmed;
        }
    }
    (
        number.trim().to_string(),
        qualifier.filter(|q| q.contains('/')),
    )
}

fn encode_search(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn hit_from_search(item: &Value, allowed: &[&str]) -> Option<GithubHit> {
    let api_repo = item.get("repository_url")?.as_str()?;
    let parsed = parse_github_remote(&api_repo.replacen(
        "https://api.github.com/repos/",
        "https://github.com/",
        1,
    ))?;
    let owner_repo = format!("{}/{}", parsed.owner, parsed.name).to_ascii_lowercase();
    if !allowed
        .iter()
        .any(|repo| repo.eq_ignore_ascii_case(&owner_repo))
    {
        return None;
    }
    let kind = if item.get("pull_request").is_some() {
        GithubItem::Pull
    } else {
        GithubItem::Issue
    };
    Some(GithubHit {
        owner_repo,
        number: item.get("number")?.as_u64()?,
        title: item.get("title")?.as_str()?.to_string(),
        url: item.get("html_url")?.as_str()?.to_string(),
        kind,
        state: item
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("open")
            .to_string(),
        updated_at: item
            .get("updated_at")
            .and_then(Value::as_str)
            .map(str::to_string),
        draft: false,
        merged: false,
    })
}

fn hit_from_issue(value: &Value, owner_repo: &str) -> Option<GithubHit> {
    let kind = if value.get("pull_request").is_some() {
        GithubItem::Pull
    } else {
        GithubItem::Issue
    };
    Some(GithubHit {
        owner_repo: owner_repo.to_string(),
        number: value.get("number")?.as_u64()?,
        title: value.get("title")?.as_str()?.to_string(),
        url: value
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or(owner_repo)
            .to_string(),
        kind,
        state: value
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("open")
            .to_string(),
        updated_at: value
            .get("updated_at")
            .and_then(Value::as_str)
            .map(str::to_string),
        draft: false,
        merged: false,
    })
}

fn issue_from(item: &Value) -> Option<GithubIssue> {
    if item.get("pull_request").is_some() {
        return None;
    }
    Some(GithubIssue {
        number: item.get("number")?.as_u64()?,
        title: item.get("title")?.as_str()?.to_string(),
        body: item
            .get("body")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        state: item.get("state")?.as_str()?.to_string(),
        url: item.get("html_url")?.as_str()?.to_string(),
        labels: item
            .get("labels")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn found_from(item: &Value, role: PullRole) -> Option<FoundPull> {
    let api_repo = item.get("repository_url")?.as_str()?;
    let repo = parse_github_remote(&api_repo.replacen(
        "https://api.github.com/repos/",
        "https://github.com/",
        1,
    ))?;
    Some(FoundPull {
        repo,
        number: item.get("number")?.as_u64()?,
        title: item.get("title")?.as_str()?.to_string(),
        url: item.get("html_url")?.as_str()?.to_string(),
        role,
    })
}

fn checks_from(value: &Value) -> Checks {
    let runs = match value.get("check_runs").and_then(Value::as_array) {
        Some(runs) if !runs.is_empty() => runs,
        _ => return Checks::None,
    };
    let mut pending = false;
    let mut failing = false;
    for run in runs {
        let status = run.get("status").and_then(Value::as_str).unwrap_or("");
        if status != "completed" {
            pending = true;
            continue;
        }
        match run.get("conclusion").and_then(Value::as_str).unwrap_or("") {
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure" => {
                failing = true;
            }
            _ => {}
        }
    }
    if failing {
        Checks::Failing
    } else if pending {
        Checks::Pending
    } else {
        Checks::Passing
    }
}

fn review_from(value: &Value) -> Review {
    let reviews = match value.as_array() {
        Some(reviews) if !reviews.is_empty() => reviews,
        _ => return Review::None,
    };
    // The newest review per author wins.
    let mut latest: std::collections::BTreeMap<&str, (&str, &str)> =
        std::collections::BTreeMap::new();
    for review in reviews {
        let Some(user) = review.pointer("/user/login").and_then(Value::as_str) else {
            continue;
        };
        let state = review.get("state").and_then(Value::as_str).unwrap_or("");
        let at = review
            .get("submitted_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        let entry = latest.entry(user).or_insert((at, state));
        if at >= entry.0 {
            *entry = (at, state);
        }
    }
    let states: Vec<&str> = latest.values().map(|(_, state)| *state).collect();
    if states.contains(&"CHANGES_REQUESTED") {
        Review::ChangesRequested
    } else if states.contains(&"APPROVED") {
        Review::Approved
    } else if states.contains(&"PENDING") {
        Review::Pending
    } else {
        Review::None
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use serde_json::json;

    use super::*;

    async fn mock() -> String {
        let pulls = Router::new().route(
            "/repos/acme/relay/pulls/7",
            get(|| async {
                axum::Json(json!({
                    "number": 7,
                    "state": "open",
                    "draft": false,
                    "merged": false,
                    "title": "Add the hover tab",
                    "html_url": "https://github.com/acme/relay/pull/7",
                    "head": { "sha": "abc123" }
                }))
            }),
        );
        let checks = Router::new().route(
            "/repos/acme/relay/commits/abc123/check-runs",
            get(|| async {
                axum::Json(json!({
                    "total_count": 2,
                    "check_runs": [
                        {"status": "completed", "conclusion": "success"},
                        {"status": "in_progress", "conclusion": null}
                    ]
                }))
            }),
        );
        let reviews = Router::new().route(
            "/repos/acme/relay/pulls/7/reviews",
            get(|| async {
                axum::Json(json!([
                    {"user": {"login": "ana"}, "state": "APPROVED", "submitted_at": "2026-11-17T10:00:00Z"},
                    {"user": {"login": "bo"}, "state": "CHANGES_REQUESTED", "submitted_at": "2026-11-17T11:00:00Z"}
                ]))
            }),
        );
        let issues = Router::new().route(
            "/repos/acme/relay/issues",
            get(|| async {
                axum::Json(json!([
                    {
                        "number": 12,
                        "title": "Hover tab sticks",
                        "body": "When the main window closes...",
                        "state": "open",
                        "html_url": "https://github.com/acme/relay/issues/12",
                        "labels": [{"name": "bug"}]
                    },
                    {
                        "number": 7,
                        "title": "Add the hover tab",
                        "state": "open",
                        "html_url": "https://github.com/acme/relay/pull/7",
                        "pull_request": {"url": "https://api.github.com/repos/acme/relay/pulls/7"}
                    }
                ]))
            }),
        );
        let search = Router::new().route(
            "/search/issues",
            get(
                |axum::extract::Query(q): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| async move {
                    let wanted = q.get("q").cloned().unwrap_or_default();
                    let item = |number: u64, repo: &str| {
                        json!({
                            "number": number,
                            "title": format!("Pull {number}"),
                            "html_url": format!("https://github.com/{repo}/pull/{number}"),
                            "repository_url": format!("https://api.github.com/repos/{repo}")
                        })
                    };
                    axum::Json(if wanted.contains("author:@me") {
                        json!({"items": [item(1, "acme/relay"), item(2, "acme/Other")]})
                    } else {
                        json!({"items": [item(3, "acme/relay"), {"number": 4}]})
                    })
                },
            ),
        );
        let app = pulls
            .merge(checks)
            .merge(search)
            .merge(reviews)
            .merge(issues);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn reads_the_pr_checks_and_review() {
        let base = mock().await;
        let client = GithubClient::with_base(Some("ghp_test".into()), base).unwrap();
        let pull = client.pull_request("acme", "relay", 7).await.unwrap();
        assert_eq!(pull.number, 7);
        assert_eq!(pull.title, "Add the hover tab");
        assert_eq!(pull.head_sha, "abc123");
        assert_eq!(pull.checks, Checks::Pending);
        assert_eq!(pull.review, Review::ChangesRequested);
        assert!(!pull.merged);

        let issues = client.list_issues("acme", "relay").await.unwrap();
        assert_eq!(issues.len(), 1, "the pull request is skipped");
        assert_eq!(issues[0].number, 12);
        assert_eq!(issues[0].labels, vec!["bug"]);
        assert_eq!(issues[0].url, "https://github.com/acme/relay/issues/12");
    }

    #[tokio::test]
    async fn searches_my_open_pulls_by_role() {
        let client = GithubClient::with_base(Some("t".into()), mock().await).unwrap();
        let found = client.search_my_pulls().await.unwrap();
        let got: Vec<_> = found
            .iter()
            .map(|p| (p.repo.name.as_str(), p.number, p.role))
            .collect();
        assert_eq!(
            got,
            [
                ("relay", 1, PullRole::Author),
                ("Other", 2, PullRole::Author),
                ("relay", 3, PullRole::ReviewRequested),
            ],
            "the malformed item is skipped"
        );
        assert_eq!(found[0].repo.owner, "acme");
        assert_eq!(found[0].url, "https://github.com/acme/relay/pull/1");
    }

    /// `RELAY_GH_TOKEN=$(gh auth token) cargo test -p relay-core real_search -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "calls api.github.com"]
    async fn real_search() {
        let token = std::env::var("RELAY_GH_TOKEN").expect("RELAY_GH_TOKEN");
        let found = GithubClient::new(Some(token))
            .unwrap()
            .search_my_pulls()
            .await
            .unwrap();
        for p in found {
            println!("{:?} {}/{}#{}", p.role, p.repo.owner, p.repo.name, p.number);
        }
    }

    #[test]
    fn check_summaries() {
        assert_eq!(checks_from(&json!({})), Checks::None);
        assert_eq!(
            checks_from(&json!({"check_runs": [{"status": "completed", "conclusion": "success"}]})),
            Checks::Passing
        );
        assert_eq!(
            checks_from(&json!({"check_runs": [{"status": "completed", "conclusion": "failure"}]})),
            Checks::Failing
        );
        assert_eq!(
            checks_from(&json!({"check_runs": [{"status": "queued"}]})),
            Checks::Pending
        );
    }

    #[test]
    fn the_newest_review_per_author_wins() {
        let reviews = json!([
            {"user": {"login": "ana"}, "state": "CHANGES_REQUESTED", "submitted_at": "2026-11-17T09:00:00Z"},
            {"user": {"login": "ana"}, "state": "APPROVED", "submitted_at": "2026-11-17T10:00:00Z"}
        ]);
        assert_eq!(review_from(&reviews), Review::Approved);
        assert_eq!(review_from(&json!([])), Review::None);
    }

    async fn serve(app: axum::Router) -> GithubClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        GithubClient::with_base(Some("token".into()), format!("http://{addr}")).unwrap()
    }

    #[tokio::test]
    async fn live_search_drops_results_outside_the_registered_repos() {
        use axum::extract::RawQuery;
        let app = Router::new().route(
            "/search/issues",
            get(|RawQuery(query): RawQuery| async move {
                let query = query.unwrap_or_default();
                assert!(query.contains("repo%3Aacme%2Frelay") || query.contains("repo:acme/relay"));
                let item = |repo: &str, number: u64| {
                    json!({
                        "number": number,
                        "title": format!("Hit {number}"),
                        "html_url": format!("https://github.com/{repo}/issues/{number}"),
                        "repository_url": format!("https://api.github.com/repos/{repo}"),
                        "state": "open",
                        "pull_request": {"url": "https://api.github.com/repos/x/pulls/1"}
                    })
                };
                axum::Json(json!({"items": [item("acme/relay", 7), item("evil/other", 8)]}))
            }),
        );
        let client = serve(app).await;
        let hits = client
            .search_repos("hover", &["acme/relay".into()])
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "the unregistered repo is rejected");
        assert_eq!(hits[0].owner_repo, "acme/relay");
        assert_eq!(
            hits[0].kind,
            GithubItem::Pull,
            "the pull marker classifies it"
        );
    }

    #[tokio::test]
    async fn a_number_uses_the_issue_response_marker_and_404_is_not_a_failed_query() {
        let app = Router::new()
            .route(
                "/repos/acme/relay/issues/142",
                get(|| async {
                    axum::Json(json!({
                        "number": 142,
                        "title": "Ship the RC",
                        "html_url": "https://github.com/acme/relay/pull/142",
                        "state": "open",
                        "pull_request": {"url": "https://api.github.com/repos/acme/relay/pulls/142"}
                    }))
                }),
            )
            .route(
                "/repos/acme/relay/issues/999",
                get(|| async { (axum::http::StatusCode::NOT_FOUND, axum::Json(json!({}))) }),
            );
        let client = serve(app).await;
        let pull = client.issue_or_pull("acme", "relay", 142).await.unwrap();
        assert_eq!(pull.kind, GithubItem::Pull);
        assert_eq!(pull.number, 142);
        assert!(matches!(
            client.issue_or_pull("acme", "relay", 999).await,
            Err(GithubError::Status { status: 404, .. })
        ));
    }

    #[tokio::test]
    async fn a_pasted_id_reads_directly_outside_the_repo_allowlist() {
        let app = Router::new()
            .route(
                "/repos/other/side/issues/9",
                get(|| async {
                    axum::Json(json!({
                        "number": 9, "title": "Direct issue", "state": "closed",
                        "html_url": "https://github.com/other/side/issues/9"
                    }))
                }),
            )
            .route(
                "/repos/other/side/pulls/10",
                get(|| async {
                    axum::Json(json!({
                        "number": 10, "title": "Direct PR", "state": "closed",
                        "merged": true, "draft": false,
                        "html_url": "https://github.com/other/side/pull/10",
                        "updated_at": "2026-11-21T00:00:00Z"
                    }))
                }),
            );
        let client = serve(app).await;
        let issue = client.issue_or_pull("other", "side", 9).await.unwrap();
        assert_eq!(issue.kind, GithubItem::Issue);
        assert_eq!(issue.state, "closed");
        let pull = client.pull_summary("other", "side", 10).await.unwrap();
        assert_eq!(pull.state, "merged");
        assert!(pull.merged);
    }

    #[tokio::test]
    async fn rate_limit_headers_become_a_typed_failure() {
        use axum::response::IntoResponse;
        let app = Router::new().route(
            "/repos/acme/rate/issues/1",
            get(|| async {
                let mut response =
                    (axum::http::StatusCode::FORBIDDEN, "API rate limit exceeded").into_response();
                response
                    .headers_mut()
                    .insert("x-ratelimit-reset", "1795000000".parse().unwrap());
                response
            }),
        );
        let client = serve(app).await;
        let error = client.issue_or_pull("acme", "rate", 1).await.unwrap_err();
        assert_eq!(
            error.failure(),
            ProviderFailure::RateLimited {
                retry_after_ms: Some(1_795_000_000_000)
            }
        );
    }
}
