//! GitHub pull-request state for linked tasks (PLAN 3.5). Read-only, github.com only.
//! A token is optional; without one the public API is used and rate limits apply.

use serde_json::Value;

use crate::repos::{GithubRepo, parse_github_remote};

pub const DEFAULT_BASE_URL: &str = "https://api.github.com";
const TIMEOUT_SECS: u64 = 15;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("github request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("github answered {status}: {message}")]
    Status { status: u16, message: String },
    #[error("github response is missing {0}")]
    Missing(&'static str),
    #[error("github json: {0}")]
    Json(#[from] serde_json::Error),
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
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(GithubError::Status { status, message });
        }
        Ok(response.json().await?)
    }
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
}
