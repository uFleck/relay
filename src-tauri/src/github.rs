//! GitHub pull-request sync and project git state (PLAN 3.5). The poller walks linked PRs
//! every three minutes; git state is read on demand for a project's checkout.
//!
//! Credentials resolve in order: a token stored in Settings (explicit override), then the
//! user's `gh` CLI session, then unauthenticated.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relay_core::git::{self, GitState};
use relay_core::github::{GithubClient, GithubError};
use relay_core::model::TaskLink;
use relay_core::store::Store;
use relay_core::view::{SyncSource, SyncState};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::secrets;

const TOKEN_KEY: &str = "github-token";
/// PR state is not urgent; a poll every three minutes is plenty and stays far under the
/// unauthenticated rate limit for a handful of links.
const POLL_EVERY: Duration = Duration::from_secs(180);
const FIRST_POLL: Duration = Duration::from_secs(20);
const GH_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where the credentials of a GitHub call come from, in priority order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthSource {
    /// A token pasted in Settings (explicit override).
    Token,
    /// The user's active `gh` CLI session.
    Gh,
    #[default]
    None,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubStatus {
    /// A token override is stored. The UI uses it to show the "remove token" action.
    pub configured: bool,
    pub source: AuthSource,
    /// The GitHub login behind the `gh` session, when it is known.
    pub login: Option<String>,
    pub error: Option<String>,
}

/// A `gh auth token` result. Held in memory only; never written to the keyring or a log.
#[derive(Clone)]
struct GhSession {
    token: String,
    login: Option<String>,
}

/// The credentials a request uses. No `Debug`: the token must not reach a log.
struct Credentials {
    source: AuthSource,
    token: Option<String>,
    login: Option<String>,
}

static GH_SESSION: Mutex<Option<GhSession>> = Mutex::new(None);

#[derive(Default)]
pub struct GithubState {
    /// The last status computed; the snapshot reads it without spawning `gh`.
    status: Mutex<GithubStatus>,
}

impl GithubState {
    pub fn get(&self) -> GithubStatus {
        self.status.lock().expect("github state").clone()
    }
}

/// Stored token first, then the `gh` session, then nothing. `gh` is only asked when no
/// token is stored.
async fn resolve_with<F, Fut>(stored: Option<String>, gh: F) -> Credentials
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<GhSession>>,
{
    if let Some(token) = stored {
        return Credentials {
            source: AuthSource::Token,
            token: Some(token),
            login: None,
        };
    }
    match gh().await {
        Some(session) => Credentials {
            source: AuthSource::Gh,
            token: Some(session.token),
            login: session.login,
        },
        None => Credentials {
            source: AuthSource::None,
            token: None,
            login: None,
        },
    }
}

/// `refresh` drops the cached `gh` token first (GitHub answered 401).
async fn credentials(refresh: bool) -> Credentials {
    let stored = secrets::read(TOKEN_KEY);
    resolve_with(stored, || gh_session(refresh)).await
}

/// The token for a one-off call such as the issue import.
pub async fn current_token() -> Option<String> {
    credentials(false).await.token
}

async fn gh_session(refresh: bool) -> Option<GhSession> {
    if !refresh {
        let cached = GH_SESSION.lock().expect("gh session").clone();
        if cached.is_some() {
            return cached;
        }
    }
    let found = query_gh().await;
    *GH_SESSION.lock().expect("gh session") = found.clone();
    found
}

async fn query_gh() -> Option<GhSession> {
    let token = run_gh(&["auth", "token"]).await?;
    let login = run_gh(&["api", "user", "--jq", ".login"]).await;
    Some(GhSession { token, login })
}

/// Runs `gh` without a console window and with a short timeout. Any failure or empty
/// output is `None`; the output is never logged.
async fn run_gh(args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new("gh");
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(GH_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Recomputes the status, stores it and tells the UI when it changed.
async fn refresh_status(app: &AppHandle, error: Option<String>) -> GithubStatus {
    let creds = credentials(false).await;
    publish(app, &creds, error)
}

fn publish(app: &AppHandle, creds: &Credentials, error: Option<String>) -> GithubStatus {
    let status = GithubStatus {
        configured: secrets::read(TOKEN_KEY).is_some(),
        source: creds.source,
        login: creds.login.clone(),
        error,
    };
    let state = app.state::<GithubState>();
    let changed = {
        let mut slot = state.status.lock().expect("github state");
        let changed = slot.configured != status.configured
            || slot.source != status.source
            || slot.login != status.login
            || slot.error != status.error;
        *slot = status.clone();
        changed
    };
    if changed {
        let _ = app.emit("github://status", &status);
        // The rail and Settings → Sources read the snapshot, which carries this status.
        let _ = app.emit("relay://change", ());
    }
    status
}

/// The Sources row for GitHub, from the last computed status.
pub fn sync_source(status: &GithubStatus) -> SyncSource {
    let (state, reason) = if let Some(error) = &status.error {
        (SyncState::Down, Some(error.clone()))
    } else {
        match status.source {
            AuthSource::Token => (SyncState::Ok, Some("token override".to_string())),
            AuthSource::Gh => (
                SyncState::Ok,
                Some(match &status.login {
                    Some(login) => format!("gh session ({login})"),
                    None => "gh session".to_string(),
                }),
            ),
            AuthSource::None => (
                SyncState::Off,
                Some("unauthenticated · 60 requests/hour".to_string()),
            ),
        }
    };
    SyncSource {
        name: "GitHub".into(),
        short: "GitHub".into(),
        state,
        reason,
    }
}

pub fn start(app: AppHandle) {
    app.manage(GithubState::default());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Learn the source right away so the rail does not wait for the first poll.
        refresh_status(&app, None).await;
        tokio::time::sleep(FIRST_POLL).await;
        loop {
            let result = poll_once(&app).await;
            let status = refresh_status(&app, result.err()).await;
            // Without credentials the public API allows 60 calls/hour; slow down instead of
            // burning the limit on every poll.
            let pause = if status.source != AuthSource::None {
                POLL_EVERY
            } else {
                Duration::from_secs(1800)
            };
            tokio::time::sleep(pause).await;
        }
    });
}

struct Outcome {
    updated: usize,
    errors: Vec<String>,
    unauthorized: bool,
}

async fn poll_links(
    store: &Store,
    creds: &Credentials,
    links: &[TaskLink],
) -> Result<Outcome, String> {
    let client = GithubClient::new(creds.token.clone()).map_err(|error| error.to_string())?;
    let mut out = Outcome {
        updated: 0,
        errors: Vec::new(),
        unauthorized: false,
    };
    for link in links {
        let Some((owner, repo, number)) = parse_pr_url(&link.url) else {
            out.errors
                .push(format!("{} is not a GitHub pull request", link.url));
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
                out.updated += 1;
            }
            // One unreachable PR must not stop the others.
            Err(error) => {
                if matches!(error, GithubError::Status { status: 401, .. }) {
                    out.unauthorized = true;
                }
                out.errors.push(format!("PR {}: {error}", link.url));
            }
        }
    }
    Ok(out)
}

async fn poll_once(app: &AppHandle) -> Result<usize, String> {
    let store = app.state::<Arc<Store>>().inner().clone();
    let links = store
        .list_links_by_kind("pr")
        .map_err(|error| error.to_string())?;
    if links.is_empty() {
        return Ok(0);
    }
    let mut creds = credentials(false).await;
    let mut out = poll_links(&store, &creds, &links).await?;
    // The cached `gh` token went stale (logout, rotation): ask `gh` again, once.
    if out.unauthorized && creds.source == AuthSource::Gh {
        creds = credentials(true).await;
        out = poll_links(&store, &creds, &links).await?;
    }
    if out.updated > 0 {
        let _ = app.emit("relay://change", ());
    }
    if out.errors.is_empty() {
        Ok(out.updated)
    } else if out.updated > 0 {
        publish(app, &creds, Some(out.errors.join("; ")));
        Ok(out.updated)
    } else {
        Err(out.errors.join("; "))
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
pub async fn github_status(app: AppHandle) -> GithubStatus {
    let creds = credentials(false).await;
    let error = app.state::<GithubState>().get().error;
    publish(&app, &creds, error)
}

#[tauri::command]
pub async fn github_set_token(app: AppHandle, token: String) -> Result<GithubStatus, String> {
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
    let status = refresh_status(&app, None).await;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = poll_once(&app).await;
    });
    Ok(status)
}

#[tauri::command]
pub async fn github_clear_token(app: AppHandle) -> GithubStatus {
    secrets::delete(TOKEN_KEY);
    refresh_status(&app, None).await
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
    use super::{AuthSource, GhSession, parse_pr_url, resolve_with};

    fn gh() -> Option<GhSession> {
        Some(GhSession {
            token: "gh-token".into(),
            login: Some("octo".into()),
        })
    }

    #[tokio::test]
    async fn stored_token_wins_and_gh_is_not_asked() {
        let creds = resolve_with(Some("pasted".into()), || async {
            panic!("gh must not run when a token is stored")
        })
        .await;
        assert_eq!(creds.source, AuthSource::Token);
        assert_eq!(creds.token.as_deref(), Some("pasted"));
    }

    #[tokio::test]
    async fn gh_session_is_used_without_a_stored_token() {
        let creds = resolve_with(None, || async { gh() }).await;
        assert_eq!(creds.source, AuthSource::Gh);
        assert_eq!(creds.token.as_deref(), Some("gh-token"));
        assert_eq!(creds.login.as_deref(), Some("octo"));
    }

    #[tokio::test]
    async fn no_token_and_no_gh_is_unauthenticated() {
        let creds = resolve_with(None, || async { None }).await;
        assert_eq!(creds.source, AuthSource::None);
        assert!(creds.token.is_none());
    }

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
