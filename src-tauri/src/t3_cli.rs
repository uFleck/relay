//! Local T3 Code CLI runner for dispatch (#36).
//!
//! The CLI runs as the same OS user with the same T3 data directory and needs no stored credential.
//! Every call uses separate process arguments, a UTF-8 stdin payload, bounded output, a deadline,
//! a hidden window and a Job Object that kills the launcher tree. `.cmd` shims are invoked through
//! a **static** `cmd.exe /D /V:OFF /S /C` wrapper whose only text is `%VAR%` references; every
//! untrusted value is set as a child environment variable and expanded exactly once. Caret
//! insertion into an argument is not used: inside cmd's quotes a caret is literal, and it does not
//! reliably stop percent expansion. A directly invocable bundled Electron entrypoint is preferred
//! over the shim when the shim exposes one.
//!
//! Nothing here decides Task state. stderr is translated into fixed reasons and never returned or
//! logged: it may contain user paths or provider text.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use relay_core::t3_dispatch::{ListedThread, ThreadIdentity};
use relay_core::t3_progress::{ObservedMessage, ObservedPullRequest, RunStatus, ThreadObservation};
use serde::Deserialize;
use serde_json::Value;

use crate::process::{self, ProcessError};

const LAUNCHERS: &[&str] = &["t3.cmd", "t3.exe", "t3"];
const START_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTPUT: usize = 64 * 1024;

/// Fixed reasons used when the CLI is rejected before any thread can exist.
pub const SERVER_STOPPED: &str = "T3 Code is not running. Start it, then Retry dispatch.";
pub const PROJECT_NOT_FOUND: &str =
    "T3 did not recognise the project mapping. Register it in T3 Code, then Retry dispatch.";
pub const UNSUPPORTED: &str = "The installed t3 CLI does not support this command. Update T3 Code.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchClass {
    Responded(StartResponse),
    KnownFailure(&'static str),
    Uncertain(&'static str),
}

/// A typed start response. `thread_id` is the only required field; a missing run id or link is
/// hydrated with a read-only `show` before the identity is committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartResponse {
    pub thread_id: String,
    pub run_id: Option<String>,
    pub link: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug)]
pub enum CliError {
    LauncherMissing,
    BadOutput(&'static str),
    Timeout,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::LauncherMissing => "the t3 launcher was not found",
            Self::BadOutput(reason) => reason,
            Self::Timeout => "the CLI call timed out",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for CliError {}

/// The T3 home is `T3CODE_HOME` when set, otherwise the Windows default the desktop shim installs.
pub fn t3_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("T3CODE_HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }
    let user = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(user).join(".t3"))
}

/// Prefers an absolute launcher on PATH. A GUI app started before T3 changed the user PATH falls
/// back to the shim in the configured or default T3 home's `bin` directory.
pub fn resolve_launcher() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in LAUNCHERS {
                let candidate = dir.join(name);
                if candidate.is_file() && candidate.is_absolute() {
                    return Some(candidate);
                }
            }
        }
    }
    let home = t3_home()?;
    let bin = home.join("bin");
    if !bin.is_absolute() {
        return None;
    }
    LAUNCHERS
        .iter()
        .map(|name| bin.join(name))
        .find(|candidate| candidate.is_file())
}

/// Reads a generated `.cmd` shim and returns its bundled Electron executable and CLI script when
/// both are absolute files. Invoking that pair directly avoids `cmd.exe` entirely, including every
/// percent, caret and ampersand concern. An unreadable or changed shim simply uses the wrapper.
pub fn bundled_entrypoint(launcher: &Path) -> Option<(PathBuf, PathBuf)> {
    if !matches!(
        launcher.extension().and_then(|ext| ext.to_str()),
        Some("cmd" | "bat")
    ) {
        return None;
    }
    let text = std::fs::read_to_string(launcher).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('"') || !line.contains("%*") {
            continue;
        }
        let quoted: Vec<&str> = line
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|part| !part.is_empty())
            .collect();
        if quoted.len() < 2 {
            continue;
        }
        let executable = PathBuf::from(quoted[0]);
        let script = PathBuf::from(quoted[1]);
        if executable.is_absolute()
            && executable.is_file()
            && script.is_absolute()
            && script.is_file()
        {
            return Some((executable, script));
        }
    }
    None
}

/// Validates one value that will be expanded from a child environment variable. Quotes and control
/// characters cannot survive cmd's tokenizer or an argv parser; a trailing run of backslashes is
/// doubled so a quoted `"%VAR%"` still ends in a literal backslash rather than escaping the quote.
pub fn prepare_arg(value: &str) -> Result<String, CliError> {
    if value.contains(['"', '\r', '\n', '\0']) {
        return Err(CliError::BadOutput(
            "unsupported character in a CLI argument",
        ));
    }
    let trailing = value
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count();
    let mut prepared = String::with_capacity(value.len() + trailing);
    prepared.push_str(value);
    for _ in 0..trailing {
        prepared.push('\\');
    }
    Ok(prepared)
}

struct Prepared {
    command: Command,
}

fn command_for(
    launcher: &Path,
    arguments: &[String],
    data_dir: &str,
) -> Result<Prepared, CliError> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        if let Some((executable, script)) = bundled_entrypoint(launcher) {
            let mut command = Command::new(executable);
            command.arg(script).args(arguments);
            command.env("ELECTRON_RUN_AS_NODE", "1");
            command.env("T3CODE_DESKTOP_CLI_LAUNCHER", "1");
            command.env("T3CODE_HOME", data_dir);
            return Ok(Prepared { command });
        }

        let is_script = matches!(
            launcher.extension().and_then(|ext| ext.to_str()),
            Some("cmd" | "bat")
        );
        let mut command = if is_script {
            let mut command = Command::new("cmd.exe");
            // Fixed text with one placeholder per argument. /V:OFF disables delayed expansion, so
            // a literal `!` in an env value is never re-read. No CALL means one expansion pass.
            let mut script = String::from("\"\"%RELAY_T3_CMD%\"");
            for index in 0..arguments.len() {
                script.push_str(&format!(" \"%RELAY_T3_ARG_{index}%\""));
            }
            script.push('"');
            command.args(["/D", "/V:OFF", "/S", "/C"]);
            command.raw_arg(script);
            command.env("RELAY_T3_CMD", prepare_arg(&launcher.to_string_lossy())?);
            for (index, argument) in arguments.iter().enumerate() {
                command.env(format!("RELAY_T3_ARG_{index}"), prepare_arg(argument)?);
            }
            command
        } else {
            let mut command = Command::new(launcher);
            command.args(arguments);
            command
        };
        command.env("T3CODE_HOME", data_dir);
        Ok(Prepared { command })
    }
    #[cfg(not(windows))]
    {
        let _ = (launcher, data_dir);
        let mut command = Command::new(launcher);
        command.args(arguments);
        Ok(Prepared { command })
    }
}

pub(crate) struct Output {
    pub(crate) status_success: bool,
    stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    timed_out: bool,
}

fn run_with(
    override_launcher: Option<&Path>,
    arguments: &[String],
    brief: Option<&str>,
    data_dir: &str,
    timeout: Duration,
) -> Result<Output, CliError> {
    let launcher = match override_launcher {
        Some(path) => path.to_path_buf(),
        None => resolve_launcher().ok_or(CliError::LauncherMissing)?,
    };
    let Prepared { command } = command_for(&launcher, arguments, data_dir)?;
    let output = process::run(
        command,
        brief.map(str::as_bytes),
        timeout,
        MAX_OUTPUT,
        MAX_OUTPUT,
        "t3",
    )
    .map_err(|error| match error {
        ProcessError::Timeout(_) => CliError::Timeout,
        ProcessError::Start(_) => CliError::LauncherMissing,
        ProcessError::Cleanup(_) => CliError::BadOutput("the t3 process did not clean up"),
    })?;
    // T3 historically ignored a broken stdin pipe when the CLI exited with a classified error.
    let _ = &output.stdin_error;
    Ok(Output {
        status_success: output.success,
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: false,
    })
}

/// Only an exact, verified pre-launch diagnostic is a known failure. A timeout or malformed result
/// is uncertain and must be reconciled rather than retried blindly.
pub fn classify_start(output: &Output) -> LaunchClass {
    if output.timed_out {
        return LaunchClass::Uncertain("The T3 start timed out; the thread may have been created.");
    }
    if output.status_success {
        return match parse_start(&output.stdout) {
            Some(response) if !response.thread_id.trim().is_empty() => {
                LaunchClass::Responded(response)
            }
            _ => LaunchClass::Uncertain("T3 returned an unreadable start result."),
        };
    }
    let text = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if text.contains("no running t3 code server") || text.contains("start t3 code") {
        LaunchClass::KnownFailure(SERVER_STOPPED)
    } else if text.contains("threadprojectnotfounderror")
        || text.contains("project not found")
        || text.contains("not a registered project")
        || text.contains("no registered project")
    {
        LaunchClass::KnownFailure(PROJECT_NOT_FOUND)
    } else if text.contains("update the t3 cli")
        || text.contains("unknown option")
        || text.contains("unrecognized option")
        || text.contains("unknown command")
        || text.contains("usage: t3")
    {
        LaunchClass::KnownFailure(UNSUPPORTED)
    } else {
        LaunchClass::Uncertain("T3 rejected the start with an unrecognised error.")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartJson {
    thread_id: String,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

fn parse_start(stdout: &[u8]) -> Option<StartResponse> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let payload = value.get("result").unwrap_or(&value);
    let parsed: StartJson = serde_json::from_value(payload.clone()).ok()?;
    Some(StartResponse {
        thread_id: parsed.thread_id,
        run_id: parsed.run_id,
        link: parsed.link,
        project_id: parsed.project_id,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadJson {
    thread_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    latest_run_id: Option<String>,
}

impl From<ThreadJson> for ListedThread {
    fn from(thread: ThreadJson) -> Self {
        Self {
            thread_id: thread.thread_id,
            title: thread.title,
            project_id: thread.project_id,
        }
    }
}

fn parse_threads(stdout: &[u8]) -> Option<Vec<ThreadJson>> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let payload = value.get("result").unwrap_or(&value);
    if let Some(threads) = payload.as_array() {
        return serde_json::from_value(serde_json::Value::Array(threads.clone())).ok();
    }
    for key in ["threads", "items"] {
        if let Some(threads) = payload.get(key).and_then(|value| value.as_array()) {
            return serde_json::from_value(serde_json::Value::Array(threads.clone())).ok();
        }
    }
    None
}

/// `t3 thread start <selector> - --json [--title ...] [--base-dir ...] [--worktree ...]`.
/// The Brief is written verbatim to stdin so quotes, newlines, Unicode and `%PATH%` never pass
/// through a shell.
pub fn start(
    selector: &str,
    data_dir: &str,
    title: &str,
    brief: &str,
    base_ref: Option<&str>,
) -> Result<LaunchClass, CliError> {
    let mut arguments = vec![
        "thread".to_string(),
        "start".to_string(),
        selector.to_string(),
        "-".to_string(),
        "--json".to_string(),
        "--title".to_string(),
        title.to_string(),
        "--base-dir".to_string(),
        data_dir.to_string(),
    ];
    if let Some(base) = base_ref {
        arguments.push("--worktree".to_string());
        arguments.push(base.to_string());
    }
    let output = run_with(None, &arguments, Some(brief), data_dir, START_TIMEOUT)?;
    Ok(classify_start(&output))
}

/// Extracts a start response from raw output after a response was captured at another boundary.
#[allow(dead_code)]
pub fn response_from_stdout(stdout: &[u8]) -> Option<StartResponse> {
    parse_start(stdout)
}

pub(crate) fn list_output(
    selector: &str,
    data_dir: &str,
    timeout: Duration,
) -> Result<Output, CliError> {
    let arguments = vec![
        "thread".to_string(),
        "list".to_string(),
        "--project".to_string(),
        selector.to_string(),
        "--json".to_string(),
        "--base-dir".to_string(),
        data_dir.to_string(),
    ];
    run_with(None, &arguments, None, data_dir, timeout)
}

/// `t3 thread list --project <selector> --json` in the frozen data directory.
pub fn list(selector: &str, data_dir: &str) -> Result<Vec<ListedThread>, CliError> {
    let output = list_output(selector, data_dir, QUERY_TIMEOUT)?;
    if !output.status_success {
        return Err(CliError::BadOutput("the thread list could not be read"));
    }
    parse_threads(&output.stdout)
        .map(|threads| threads.into_iter().map(ListedThread::from).collect())
        .ok_or(CliError::BadOutput("the thread list shape changed"))
}

/// `t3 thread show <id> --json`: the identity fields needed to bind and to verify the project.
pub fn show(thread_id: &str, data_dir: &str) -> Result<ThreadIdentity, CliError> {
    let arguments = vec![
        "thread".to_string(),
        "show".to_string(),
        thread_id.to_string(),
        "--json".to_string(),
        "--base-dir".to_string(),
        data_dir.to_string(),
    ];
    let output = run_with(None, &arguments, None, data_dir, QUERY_TIMEOUT)?;
    if !output.status_success {
        return Err(CliError::BadOutput("the thread could not be shown"));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| CliError::BadOutput("bad show JSON"))?;
    let payload = value.get("result").unwrap_or(&value);
    let thread = payload.get("thread").unwrap_or(payload);
    let shown: ThreadJson = serde_json::from_value(thread.clone())
        .map_err(|_| CliError::BadOutput("bad thread JSON"))?;
    Ok(ThreadIdentity {
        thread_id: shown.thread_id,
        run_id: shown.latest_run_id.unwrap_or_default(),
        link: shown.link.unwrap_or_default(),
        project_id: shown.project_id.unwrap_or_default(),
    })
}

/// Fixed reasons for a send rejected before a new run could exist. The draft is kept verbatim.
pub const SEND_SERVER_STOPPED: &str = "T3 Code is not running. Start it, then Retry this message.";
pub const SEND_THREAD_MISSING: &str =
    "T3 no longer has this thread. Check it in T3 before sending again.";
pub const SEND_UNSUPPORTED: &str = "The installed t3 CLI cannot send to a thread. Update T3 Code.";

/// The acknowledged part of a `thread send` result. A missing run id is not a failure: the adapter
/// then uses a read to confirm the exact message on exactly one new run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResponse {
    pub thread_id: Option<String>,
    pub run_id: Option<String>,
    pub run_count: Option<u64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Accepted(SendResponse),
    KnownFailure(&'static str),
    Uncertain(&'static str),
}

fn field_text(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        match value.get(*key) {
            Some(Value::String(text)) if !text.trim().is_empty() => return Some(text.clone()),
            Some(Value::Number(number)) => return Some(number.to_string()),
            Some(Value::Bool(flag)) => return Some(flag.to_string()),
            _ => {}
        }
    }
    None
}

fn field_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(number) = value.get(*key).and_then(Value::as_u64) {
            return Some(number);
        }
    }
    None
}

fn field_bool(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
        .unwrap_or(false)
}

fn field_array<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Vec<Value>> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array))
}

/// `t3 thread show <id> --json --messages <n>` decoded into relay-core's typed observation. Only
/// the bounded recent messages are read; the thread object carries branch, worktree, run identity,
/// pending request count and the reported pull requests.
pub fn show_observation(thread_id: &str, data_dir: &str) -> Result<ThreadObservation, CliError> {
    let arguments = vec![
        "thread".to_string(),
        "show".to_string(),
        thread_id.to_string(),
        "--json".to_string(),
        "--messages".to_string(),
        "20".to_string(),
        "--base-dir".to_string(),
        data_dir.to_string(),
    ];
    let output = run_with(None, &arguments, None, data_dir, QUERY_TIMEOUT)?;
    if !output.status_success {
        return Err(CliError::BadOutput("the thread could not be shown"));
    }
    parse_observation(thread_id, &output.stdout)
        .ok_or(CliError::BadOutput("the thread observation shape changed"))
}

fn parse_observation(thread_id: &str, stdout: &[u8]) -> Option<ThreadObservation> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    let payload = value.get("result").unwrap_or(&value);
    let thread = payload.get("thread").unwrap_or(payload);
    // Never attribute a response for another thread to the Task being observed.
    if let Some(shown_id) = field_text(thread, &["threadId", "id"])
        && shown_id != thread_id
    {
        return None;
    }
    let error = payload
        .get("error")
        .and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Object(_) => field_text(value, &["message", "reason", "text"]),
            _ => None,
        })
        .filter(|text| !text.trim().is_empty());
    let mut messages = Vec::new();
    if let Some(items) =
        field_array(payload, &["messages"]).or_else(|| field_array(thread, &["messages"]))
    {
        for item in items {
            let text =
                field_text(item, &["text", "content", "body", "message"]).unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            messages.push(ObservedMessage {
                role: field_text(item, &["role", "authorRole"]).unwrap_or_default(),
                run_id: field_text(item, &["runId"]).unwrap_or_default(),
                text,
            });
        }
    }
    let mut pull_requests = Vec::new();
    if let Some(items) = field_array(thread, &["pullRequests", "linkedPullRequests"]) {
        for item in items {
            let url = field_text(item, &["url", "link", "htmlUrl"]).unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            pull_requests.push(ObservedPullRequest {
                url,
                source: field_text(item, &["source"]),
                state: field_text(item, &["state", "status"]),
                watching: field_bool(item, &["watching"]),
            });
        }
    }
    // `linkedPullRequest` may be a single object when the array is absent. Keep previously
    // observed links elsewhere: this read only adds what it can identify.
    if pull_requests.is_empty()
        && let Some(linked) = payload
            .get("thread")
            .and_then(|thread| thread.get("linkedPullRequest"))
            .or_else(|| payload.get("linkedPullRequest"))
        && linked.is_object()
        && let Some(url) = field_text(linked, &["url", "link", "htmlUrl"])
    {
        pull_requests.push(ObservedPullRequest {
            url,
            source: field_text(linked, &["source"]),
            state: field_text(linked, &["state", "status"]),
            watching: field_bool(linked, &["watching"]),
        });
    }
    Some(ThreadObservation {
        thread_id: thread_id.to_string(),
        active_run_id: field_text(thread, &["activeRunId"]),
        latest_run_id: field_text(thread, &["latestRunId", "runId"]),
        status: RunStatus::parse(
            &field_text(thread, &["status"]).unwrap_or_else(|| "idle".to_string()),
        ),
        pending_request_count: field_u64(thread, &["pendingRequestCount"])
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
        branch: field_text(thread, &["branch"]),
        worktree_path: field_text(thread, &["worktreePath"]),
        link: field_text(thread, &["link"]).or_else(|| field_text(payload, &["link"])),
        project_id: field_text(thread, &["projectId"]),
        run_count: field_u64(thread, &["runCount"]).unwrap_or(0),
        messages,
        pull_requests,
        error,
    })
}

/// `t3 thread send <id> - --json`: the message goes verbatim to UTF-8 stdin. No shell sees it, so
/// newlines, quotes, Unicode and percent signs are preserved exactly as the user typed them.
pub fn send(thread_id: &str, data_dir: &str, message: &str) -> Result<SendOutcome, CliError> {
    let arguments = vec![
        "thread".to_string(),
        "send".to_string(),
        thread_id.to_string(),
        "-".to_string(),
        "--json".to_string(),
        "--base-dir".to_string(),
        data_dir.to_string(),
    ];
    let output = run_with(None, &arguments, Some(message), data_dir, QUERY_TIMEOUT)?;
    Ok(classify_send(&output))
}

/// Pure translation of a finished send. Only recognised pre-send diagnostics release the draft for
/// an explicit retry; a timeout, lost or malformed result fences sends until `show` confirms it.
fn classify_send(output: &Output) -> SendOutcome {
    if output.timed_out {
        return SendOutcome::Uncertain("The send timed out; T3 may still have queued the message.");
    }
    if output.status_success {
        return parse_send(&output.stdout).map_or_else(
            || SendOutcome::Uncertain("T3 returned an unreadable send result."),
            SendOutcome::Accepted,
        );
    }
    let text = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if text.contains("no running t3 code server") || text.contains("start t3 code") {
        SendOutcome::KnownFailure(SEND_SERVER_STOPPED)
    } else if text.contains("threadnotfound")
        || text.contains("thread not found")
        || text.contains("unknown thread")
    {
        SendOutcome::KnownFailure(SEND_THREAD_MISSING)
    } else if text.contains("update the t3 cli")
        || text.contains("unknown option")
        || text.contains("unrecognized option")
        || text.contains("unknown command")
        || text.contains("usage: t3")
    {
        SendOutcome::KnownFailure(SEND_UNSUPPORTED)
    } else {
        SendOutcome::Uncertain("T3 rejected the send with an unrecognised error.")
    }
}

fn parse_send(stdout: &[u8]) -> Option<SendResponse> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    let payload = value.get("result").unwrap_or(&value);
    Some(SendResponse {
        thread_id: field_text(payload, &["threadId"]),
        run_id: field_text(payload, &["runId"]),
        run_count: field_u64(payload, &["runCount"]),
        status: field_text(payload, &["status"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(success: bool, stdout: &str, stderr: &str, timed_out: bool) -> Output {
        Output {
            status_success: success,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            timed_out,
        }
    }

    #[test]
    fn arguments_reject_quotes_and_double_a_trailing_backslash() {
        assert_eq!(
            prepare_arg(r"C:\My & Folder\%PATH%").unwrap(),
            r"C:\My & Folder\%PATH%"
        );
        assert_eq!(prepare_arg(r"C:\").unwrap(), r"C:\\");
        assert_eq!(prepare_arg("a|b<c>d(e)^f!g").unwrap(), "a|b<c>d(e)^f!g");
        assert!(prepare_arg("say \"hi\"").is_err());
        assert!(prepare_arg("line\nbreak").is_err());
    }

    #[test]
    fn start_outcomes_distinguish_known_failures_from_uncertain_results() {
        assert_eq!(
            classify_start(&output(
                false,
                "",
                "No running T3 Code server uses C:\\Users\\x",
                false
            )),
            LaunchClass::KnownFailure(SERVER_STOPPED)
        );
        assert_eq!(
            classify_start(&output(
                false,
                "",
                "ThreadProjectNotFoundError: C:\\Secret",
                false
            )),
            LaunchClass::KnownFailure(PROJECT_NOT_FOUND)
        );
        assert_eq!(
            classify_start(&output(false, "", "secret-token in stderr", false)),
            LaunchClass::Uncertain("T3 rejected the start with an unrecognised error.")
        );
        assert!(matches!(
            classify_start(&output(true, "not json", "", false)),
            LaunchClass::Uncertain(_)
        ));
    }

    #[test]
    fn start_show_and_list_identities_parse_from_json() {
        let started = r#"{"threadId":"t-1","runId":"r-1","link":"https://app.t3.codes/threads/e/t-1","projectId":"p-1"}"#;
        match classify_start(&output(true, started, "", false)) {
            LaunchClass::Responded(response) => assert_eq!(response.thread_id, "t-1"),
            other => panic!("expected a start response, got {other:?}"),
        }
        let listed = r#"{"threads":[{"threadId":"t-1","title":"Relay Task 1","projectId":"p-1"}]}"#;
        let threads = parse_threads(listed.as_bytes()).unwrap();
        assert_eq!(threads[0].thread_id, "t-1");
    }

    #[test]
    fn show_decodes_run_state_question_branch_and_pull_requests() {
        let raw = r#"{"thread":{
          "threadId":"t-1","activeRunId":"r-2","latestRunId":"r-2","status":"waiting",
          "pendingRequestCount":1,"branch":"smoke/branch","worktreePath":"C:\\wt\\smoke",
          "link":"https://app.t3.codes/threads/e/t-1","projectId":"p-1","runCount":2,
          "pullRequests":[{"url":"https://github.com/sample/app/pull/142","state":"open","source":"agent","watching":true}]
        },"messages":[
          {"role":"user","runId":"r-1","text":"brief"},
          {"role":"assistant","runId":"r-2","text":"RELAY_RETURNED"}
        ],"error":null}"#;
        let observed = parse_observation("t-1", raw.as_bytes()).unwrap();
        assert_eq!(observed.status, RunStatus::Waiting);
        assert_eq!(observed.pending_request_count, 1);
        assert_eq!(observed.observed_run_id(), Some("r-2"));
        assert_eq!(observed.branch.as_deref(), Some("smoke/branch"));
        assert_eq!(observed.worktree_path.as_deref(), Some(r"C:\wt\smoke"));
        assert_eq!(observed.run_count, 2);
        assert_eq!(observed.pull_requests.len(), 1);
        assert_eq!(observed.pull_requests[0].state.as_deref(), Some("open"));
        assert_eq!(
            relay_core::t3_progress::assistant_reply(&observed.messages, Some("r-2")).as_deref(),
            Some("RELAY_RETURNED")
        );
        // A response for a different thread is never attributed to this Task.
        assert!(parse_observation("other", raw.as_bytes()).is_none());
    }

    #[test]
    fn show_decodes_a_failed_run_reason_and_a_subset_of_links() {
        let raw = r#"{"result":{"thread":{"threadId":"t-1","status":"failed",
          "latestRunId":"r-1","runCount":1,"pullRequests":[]},
          "messages":[],"error":"provider refused the request"}}"#;
        let observed = parse_observation("t-1", raw.as_bytes()).unwrap();
        assert_eq!(observed.status, RunStatus::Failed);
        assert_eq!(
            observed.error.as_deref(),
            Some("provider refused the request")
        );
        assert!(observed.pull_requests.is_empty());
    }

    #[test]
    fn send_decodes_a_run_id_and_classifies_rejections() {
        let accepted = parse_send(br#"{"runId":"r-2","status":"queued","runCount":2}"#).unwrap();
        assert_eq!(accepted.run_id.as_deref(), Some("r-2"));
        assert_eq!(accepted.run_count, Some(2));
        assert!(parse_send(b"not json").is_none());
        assert_eq!(
            classify_send(&output(false, "", "No running T3 Code server", false)),
            SendOutcome::KnownFailure(SEND_SERVER_STOPPED)
        );
        assert_eq!(
            classify_send(&output(false, "", "ThreadNotFoundError: secret", false)),
            SendOutcome::KnownFailure(SEND_THREAD_MISSING)
        );
        assert!(matches!(
            classify_send(&output(true, "not json", "", false)),
            SendOutcome::Uncertain(_)
        ));
        assert!(matches!(
            classify_send(&output(false, "", "secret token", false)),
            SendOutcome::Uncertain(_)
        ));
    }

    /// Real-process argv proof. A disposable `.cmd` with spaces, `&`, `^`, `!` and literal
    /// `%PATH%` in its own path invokes a Python argv fixture with `%*`; one argument ends in a
    /// backslash and the stdin payload is Unicode. This asserts the child's actual argv, not the
    /// escaping helper. Skipped only when Python is not on PATH.
    #[test]
    #[cfg(windows)]
    fn cmd_wrapper_preserves_argv_and_utf8_stdin() {
        use std::env;
        use std::path::PathBuf;

        fn find_python() -> Option<PathBuf> {
            let path = env::var_os("PATH")?;
            env::split_paths(&path)
                .flat_map(|dir| ["python.exe", "python3.exe"].map(|name| dir.join(name)))
                .find(|candidate| candidate.is_file())
        }
        let Some(python) = find_python() else {
            eprintln!("SKIP: python is not on PATH");
            return;
        };
        let base = env::temp_dir().join(format!(
            "relay-t3-argv-proof-{}",
            relay_core::model::new_id()
        ));
        let dir = base.join("Command & literal %PATH% ^ !");
        std::fs::create_dir_all(&dir).unwrap();
        let script = base.join("argv.py");
        std::fs::write(
            &script,
            "import json,sys\nprint(json.dumps({'argv':sys.argv[1:],'stdin':sys.stdin.buffer.read().decode('utf-8')},ensure_ascii=True))\n",
        )
        .unwrap();
        let direct_shim = dir.join("direct.cmd");
        std::fs::write(
            &direct_shim,
            format!(
                "@echo off\r\nsetlocal EnableExtensions DisableDelayedExpansion\r\n\"{}\" \"{}\" %*\r\nexit /b %ERRORLEVEL%\r\n",
                python.display(),
                script.display()
            ),
        )
        .unwrap();
        // The wrapper shim has no quoted absolute executable, so `bundled_entrypoint` declines and
        // the static `cmd.exe /D /V:OFF /S /C` env-expansion path is exercised instead.
        let wrapper_shim = dir.join("wrapper.cmd");
        std::fs::write(
            &wrapper_shim,
            format!(
                "@echo off\r\nsetlocal EnableExtensions DisableDelayedExpansion\r\npython \"{}\" %*\r\nexit /b %ERRORLEVEL%\r\n",
                script.display()
            ),
        )
        .unwrap();

        let tricky = dir
            .join("a path with & ^ ! and %PATH%")
            .to_string_lossy()
            .to_string();
        let args = vec![
            "thread".to_string(),
            "start".to_string(),
            tricky.clone(),
            "-".to_string(),
            "C:\\".to_string(),
            "--title".to_string(),
            "Relay Task ünicode & %PATH%".to_string(),
        ];
        let brief =
            "Goal: keep `%PATH%`, &, ^, ! and Unicode 日本語 intact.\nSecond line \"quoted\".";
        let data_dir = dir.to_string_lossy().to_string();
        for shim in [&direct_shim, &wrapper_shim] {
            let result =
                run_with(Some(shim), &args, Some(brief), &data_dir, QUERY_TIMEOUT).unwrap();
            assert!(
                result.status_success,
                "fixture {} failed: {}",
                shim.display(),
                String::from_utf8_lossy(&result.stderr)
            );
            let parsed: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            let argv: Vec<String> = serde_json::from_value(parsed["argv"].clone()).unwrap();
            assert_eq!(argv, args, "child argv must be byte-for-byte identical");
            assert_eq!(parsed["stdin"].as_str().unwrap(), brief);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The real-CLI lost-response acceptance case is opt-in because it needs a running T3 Code
    /// server and a disposable registered project. Run with `--ignored --exact --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a running T3 Code server and a disposable registered test project"]
    fn windows_lost_start_response() {
        use relay_core::model::new_id;
        use relay_core::t3_dispatch::{
            DispatchStart, DispatchState, bind, claim as claim_dispatch,
            recover as recover_dispatch,
        };
        use relay_core::v2::{
            AgentMode, NewV2Project, NoteWrite, PreparedLine, PreparedNoteWrite,
            PreparedTaskIntent, T3Target, TaskStatus, V2Store, begin_attempt, complete_attempt,
        };

        let Some(launcher) = resolve_launcher() else {
            eprintln!("SKIP: T3 launcher is not installed");
            return;
        };
        eprintln!("launcher {}", launcher.display());
        let Some(data_dir) = t3_home() else {
            eprintln!("SKIP: no T3 data directory");
            return;
        };
        let data_dir = data_dir.to_string_lossy().to_string();
        let selector = std::env::var("RELAY_T3_TEST_PROJECT").unwrap_or_default();
        if selector.is_empty() {
            eprintln!("SKIP: set RELAY_T3_TEST_PROJECT to a disposable registered project");
            return;
        }
        let kind = std::env::var("RELAY_T3_TEST_PROJECT_KIND").unwrap_or_else(|_| "id".into());
        let Ok(target) = T3Target::parse(&kind, &selector) else {
            eprintln!("SKIP: invalid RELAY_T3_TEST_PROJECT selector");
            return;
        };
        let probe_arguments = vec![
            "thread".to_string(),
            "list".to_string(),
            "--project".to_string(),
            target.value().to_string(),
            "--json".to_string(),
            "--base-dir".to_string(),
            data_dir.clone(),
        ];
        if run_with(None, &probe_arguments, None, &data_dir, QUERY_TIMEOUT).is_err() {
            eprintln!("SKIP: T3 server or test project is unavailable");
            return;
        }

        let dir = std::env::temp_dir().join(format!("relay-36-lost-{}", new_id()));
        let path = dir.join("relay-v2.db");
        let (attempt_id, task_id, title, brief) = {
            let store = V2Store::open(&path).unwrap();
            let project = store
                .register_project(NewV2Project {
                    name: "Dispatch smoke".into(),
                    slug: "dispatch-smoke".into(),
                    repo_path: None,
                    owner_repo: None,
                    t3_target: None,
                })
                .unwrap();
            store.set_t3_target(&project.id, Some(target)).unwrap();
            let note = store
                .save_note(PreparedNoteWrite {
                    write: NoteWrite::Create {
                        submission_key: format!("lost-response-{}", new_id()),
                    },
                    source: "smoke".into(),
                    lines: vec![PreparedLine {
                        line_id: None,
                        text: "! Read-only dispatch smoke".into(),
                        references: vec![],
                        task_intent: Some(PreparedTaskIntent {
                            agent_mode: AgentMode::Act,
                            project_id: Some(project.id.clone()),
                        }),
                    }],
                })
                .unwrap();
            let task = note.tasks[0].clone();
            let input = begin_attempt(&store, &task.id).unwrap().unwrap();
            let output = r#"{"goal":"Reply OK","decisions":["Reply only"],"constraints":["Read only"],"openRisks":["None"]}"#;
            complete_attempt(&store, &task.id, &input.attempt_id, output).unwrap();
            let claim = claim_dispatch(
                &store,
                &DispatchStart {
                    task_id: task.id.clone(),
                    data_dir: data_dir.clone(),
                    base_ref: None,
                },
            )
            .unwrap()
            .expect("a ready Task must claim one attempt");
            (
                claim.attempt.attempt_id,
                task.id,
                claim.attempt.title,
                claim.brief,
            )
        };

        // The real CLI starts the thread. Its response is deliberately discarded at the test
        // boundary: the production app needs no fault switch and no partial identity is saved.
        let started = start(&selector, &data_dir, &title, &brief, None).unwrap();
        assert!(
            matches!(started, LaunchClass::Responded(_)),
            "the disposable start must succeed"
        );

        // Reopen the store as a restart would, then reconcile from list + show alone.
        let reopened = V2Store::open(&path).unwrap();
        let recovery = recover_dispatch(&reopened).unwrap();
        assert_eq!(recovery.reconcile.len(), 1);
        assert_eq!(recovery.reconcile[0].attempt_id, attempt_id);
        assert_eq!(
            recovery.reconcile[0].state,
            DispatchState::ReconcileRequired
        );
        let selector_value = T3Target::from_storage(&recovery.reconcile[0].project_selector)
            .unwrap()
            .value()
            .to_string();
        let exact: Vec<ListedThread> = list(&selector_value, &data_dir)
            .unwrap()
            .into_iter()
            .filter(|thread| thread.title.as_deref() == Some(title.as_str()))
            .collect();
        assert_eq!(exact.len(), 1, "exactly one exact attempt-title match");
        let identity = show(&exact[0].thread_id, &data_dir).unwrap();
        assert_eq!(identity.thread_id, exact[0].thread_id);
        let bound = bind(&reopened, &attempt_id, &identity)
            .unwrap()
            .expect("the unique thread must be adopted");
        assert_eq!(bound.state, DispatchState::Bound);
        let after = reopened.workspace_snapshot().unwrap();
        let saved = after.tasks.iter().find(|task| task.id == task_id).unwrap();
        assert_eq!(saved.status, TaskStatus::Dispatched);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
