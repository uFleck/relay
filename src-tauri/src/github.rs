//! GitHub pull-request sync and project git state (PLAN 3.5). The poller walks linked PRs
//! every three minutes; git state is read on demand for a project's checkout.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relay_core::git::{self, GitState};
use relay_core::github::GithubClient;
use relay_core::store::Store;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::secrets;

const TOKEN_KEY: &str = "github-token";
/// PR state is not urgent; a poll every three minutes is plenty and stays far under the
/// unauthenticated rate limit for a handful of links.
const POLL_EVERY: Duration = Duration::from_secs(180);
const FIRST_POLL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubStatus {
    pub configured: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct GithubState {
    status: Mutex<GithubStatus>,
}

impl GithubState {
    fn get(&self) -> GithubStatus {
        self.status.lock().expect("github state").clone()
    }

    fn set_error(&self, error: Option<String>) -> GithubStatus {
        let mut status = self.status.lock().expect("github state");
        status.configured = secrets::read(TOKEN_KEY).is_some();
        status.error = error;
        status.clone()
    }
}

pub fn start(app: AppHandle) {
    app.manage(GithubState::default());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_POLL).await;
        loop {
            let result = poll_once(&app).await;
            let state = app.state::<GithubState>();
            let status = state.set_error(result.err());
            let _ = app.emit("github://status", &status);
            // Without a token the public API allows 60 calls/hour; slow down instead of
            // burning the limit on every poll.
            let pause = if secrets::read(TOKEN_KEY).is_some() {
                POLL_EVERY
            } else {
                Duration::from_secs(1800)
            };
            tokio::time::sleep(pause).await;
        }
    });
}

async fn poll_once(app: &AppHandle) -> Result<usize, String> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let links = store
        .list_links_by_kind("pr")
        .map_err(|error| error.to_string())?;
    if links.is_empty() {
        return Ok(0);
    }
    let client = GithubClient::new(secrets::read(TOKEN_KEY)).map_err(|error| error.to_string())?;
    let mut updated = 0;
    let mut errors: Vec<String> = Vec::new();
    for link in links {
        let Some((owner, repo, number)) = parse_pr_url(&link.url) else {
            errors.push(format!("{} is not a GitHub pull request", link.url));
            continue;
        };
        match client.pull_request(&owner, &repo, number).await {
            Ok(pull) => {
                let state = if pull.merged {
                    "merged"
                } else if pull.state == "closed" {
                    "closed"
                } else if pull.draft {
                    "draft"
                } else {
                    "open"
                };
                let metadata = serde_json::json!({
                    "checks": pull.checks,
                    "review": pull.review,
                    "headSha": pull.head_sha,
                })
                .to_string();
                store
                    .set_link_sync(&link.id, Some(state), Some(&pull.title), Some(&metadata))
                    .map_err(|error| error.to_string())?;
                updated += 1;
            }
            // One unreachable PR must not stop the others.
            Err(error) => errors.push(format!("PR {}: {error}", link.url)),
        }
    }
    if updated > 0 {
        let _ = app.emit("relay://change", ());
    }
    if errors.is_empty() {
        Ok(updated)
    } else if updated > 0 {
        let state = app.state::<GithubState>();
        let status = state.set_error(Some(errors.join("; ")));
        let _ = app.emit("github://status", &status);
        Ok(updated)
    } else {
        Err(errors.join("; "))
    }
}

fn parse_pr_url(url: &str) -> Option<(String, String, u64)> {
    let rest = url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| url.trim().strip_prefix("http://github.com/"))?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    let number = parts.next()?.parse().ok()?;
    Some((owner.to_string(), repo.to_string(), number))
}

#[tauri::command]
pub fn github_status(state: State<'_, GithubState>) -> GithubStatus {
    let mut status = state.get();
    status.configured = secrets::read(TOKEN_KEY).is_some();
    status
}

#[tauri::command]
pub async fn github_set_token(
    app: AppHandle,
    state: State<'_, GithubState>,
    token: String,
) -> Result<GithubStatus, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("the GitHub token is empty".into());
    }
    let client = GithubClient::new(Some(token.clone())).map_err(|error| error.to_string())?;
    client
        .check()
        .await
        .map_err(|error| format!("GitHub rejected the token: {error}"))?;
    secrets::write(TOKEN_KEY, &token)?;
    let status = state.set_error(None);
    let _ = app.emit("github://status", &status);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = poll_once(&app).await;
    });
    Ok(status)
}

#[tauri::command]
pub fn github_clear_token(app: AppHandle, state: State<'_, GithubState>) -> GithubStatus {
    secrets::delete(TOKEN_KEY);
    let status = state.set_error(None);
    let _ = app.emit("github://status", &status);
    status
}

#[tauri::command]
pub async fn sync_prs_now(app: AppHandle) -> Result<usize, String> {
    poll_once(&app).await
}

#[tauri::command]
pub fn project_git_state(
    store: State<'_, Arc<Store>>,
    project_id: String,
) -> Result<Option<GitState>, String> {
    let project = store
        .get_project(&project_id)
        .map_err(|error| error.to_string())?;
    let Some(path) = project.repo_path else {
        return Ok(None);
    };
    git::state(Path::new(&path))
        .map(Some)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn project_set_repo(
    store: State<'_, Arc<Store>>,
    project_id: String,
    repo_path: Option<String>,
) -> Result<bool, String> {
    store
        .set_project_repo(&project_id, repo_path.as_deref())
        .map(|project| project.repo_path.is_some())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_pr_url;

    #[test]
    fn only_real_pull_urls_are_polled() {
        assert_eq!(
            parse_pr_url("https://github.com/acme/relay/pull/42"),
            Some(("acme".into(), "relay".into(), 42))
        );
        assert!(parse_pr_url("https://github.com/acme/relay/issues/42").is_none());
        assert!(parse_pr_url("https://gitlab.com/acme/relay/pull/42").is_none());
        assert!(parse_pr_url("https://github.com/acme/relay/pull/not-a-number").is_none());
    }
}
