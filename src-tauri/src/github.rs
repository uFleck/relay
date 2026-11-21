//! Read-only GitHub session discovery for v2's Reference reads and Note Import. The `gh`
//! CLI session is held in memory only and never logged. The retired v1 PR poller and its
//! task-writing path are gone.

use std::sync::Mutex;
use std::time::Duration;

const GH_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `gh auth token` result. Held in memory only; never written to the keyring or a log.
#[derive(Clone)]
struct GhSession {
    token: String,
    login: Option<String>,
}

static GH_SESSION: Mutex<Option<GhSession>> = Mutex::new(None);

/// The login behind the user's `gh` session, when the CLI reports one. Settings uses
/// this to label an external session; it never reads a v1 token or starts discovery.
pub async fn session_login() -> Option<String> {
    gh_session(false).await.and_then(|session| session.login)
}

/// The `gh` session token for v2 reads. This is the second credential in v2's precedence
/// (after the stored v2 token, before public access).
pub async fn session_token() -> Option<String> {
    gh_session(false).await.map(|session| session.token)
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
