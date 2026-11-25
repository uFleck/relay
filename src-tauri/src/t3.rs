//! Bounded, explicit T3 mapping checks.
//!
//! The pinned CLI resolves a project id or a path inside a registered T3 project before
//! listing its threads. Relay runs it as a child process under the same OS user and T3
//! data directory, passes the mapping as one argument (never as shell text), discards
//! stdout and translates only recognised failures into fixed, sanitized statuses. stderr
//! is never returned or logged: it may contain user paths or provider text.

use std::time::Duration;

use crate::t3_cli::{self, CliError};

use relay_core::v2::T3Target;
use serde::Serialize;

const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingState {
    Ok,
    LauncherMissing,
    T3Stopped,
    UnregisteredTarget,
    UnsupportedVersion,
    Timeout,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingResult {
    pub state: MappingState,
    pub reason: &'static str,
}

impl MappingResult {
    fn of(state: MappingState, reason: &'static str) -> Self {
        Self { state, reason }
    }
}

/// Checks the mapping through the same launcher and process runner used by dispatch.
pub async fn check_target(target: &T3Target) -> MappingResult {
    let selector = target.value().to_owned();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let home = t3_cli::t3_home().ok_or(CliError::LauncherMissing)?;
        t3_cli::list_output(&selector, &home.to_string_lossy(), CHECK_TIMEOUT)
    })
    .await;

    match result {
        Ok(Ok(output)) => classify(
            output.status_success,
            &String::from_utf8_lossy(&output.stderr),
        ),
        Ok(Err(CliError::LauncherMissing)) => MappingResult::of(
            MappingState::LauncherMissing,
            "The T3 launcher could not be started. Check the T3 installation.",
        ),
        Ok(Err(CliError::Timeout)) => MappingResult::of(
            MappingState::Timeout,
            "The T3 mapping check timed out. Is T3 Code running?",
        ),
        _ => MappingResult::of(MappingState::Unavailable, "T3 could not check the mapping."),
    }
}

/// Pure translation of a finished CLI call. Every reason is a fixed string.
fn classify(success: bool, stderr: &str) -> MappingResult {
    if success {
        return MappingResult::of(
            MappingState::Ok,
            "T3 resolved the project or path and listed its threads.",
        );
    }
    let text = stderr.to_ascii_lowercase();
    if text.contains("no running t3 code server") || text.contains("start t3 code") {
        MappingResult::of(
            MappingState::T3Stopped,
            "T3 Code is not running. Start it and check the mapping again.",
        )
    } else if text.contains("threadprojectnotfounderror")
        || text.contains("project not found")
        || text.contains("not a registered project")
        || text.contains("no registered project")
    {
        MappingResult::of(
            MappingState::UnregisteredTarget,
            "T3 did not recognise the mapping. Register the project in T3 Code.",
        )
    } else if text.contains("update the t3 cli")
        || text.contains("unknown option")
        || text.contains("unrecognized option")
        || text.contains("unknown command")
        || text.contains("usage: t3")
    {
        MappingResult::of(
            MappingState::UnsupportedVersion,
            "The installed t3 CLI does not support this check. Update T3 Code.",
        )
    } else {
        MappingResult::of(
            MappingState::Unavailable,
            "T3 could not check the mapping. Check the T3 installation and try again.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_failures_become_fixed_sanitized_statuses() {
        assert_eq!(classify(true, "").state, MappingState::Ok);
        assert_eq!(
            classify(
                false,
                "No running T3 Code server uses C:\\Users\\someone\\AppData"
            )
            .state,
            MappingState::T3Stopped
        );
        assert_eq!(
            classify(false, "ThreadProjectNotFoundError: C:\\Secret Hill\\app").state,
            MappingState::UnregisteredTarget
        );
        assert_eq!(
            classify(false, "error: unknown option '--project'").state,
            MappingState::UnsupportedVersion
        );
        let other = classify(false, "secret-token leaked in stderr");
        assert_eq!(other.state, MappingState::Unavailable);
        assert!(!other.reason.contains("secret-token"));
    }
}
