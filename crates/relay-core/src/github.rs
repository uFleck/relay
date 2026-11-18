//! GitHub pull-request state for linked tasks (PLAN 3.5). Read-only, github.com only.
//! A token is optional; without one the public API is used and rate limits apply.

use serde_json::Value;

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
        let app = pulls.merge(checks).merge(reviews);
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
