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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use relay_core::t3_dispatch::{ListedThread, ThreadIdentity};
use serde::Deserialize;

const LAUNCHERS: &[&str] = &["t3.cmd", "t3.exe", "t3"];
const START_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTPUT: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(20);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
            command.creation_flags(CREATE_NO_WINDOW);
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
        command.creation_flags(CREATE_NO_WINDOW);
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
    status_success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
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
    let Prepared { mut command } = command_for(&launcher, arguments, data_dir)?;
    command
        .stdin(if brief.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    let (mut child, mut job) = spawn_with_job(command)?;
    #[cfg(not(windows))]
    let mut child = command.spawn().map_err(|_| CliError::LauncherMissing)?;

    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or(CliError::BadOutput("no stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(CliError::BadOutput("no stderr"))?;
    let payload = brief.unwrap_or("").as_bytes().to_vec();
    let out_thread = std::thread::spawn(move || read_bounded(stdout));
    let err_thread = std::thread::spawn(move || read_bounded(stderr));
    let in_thread = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            stdin.write_all(&payload)?;
            stdin.flush()?;
        }
        Ok::<(), std::io::Error>(())
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            #[cfg(windows)]
            if let Some(job) = job.take() {
                job.terminate();
            }
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(POLL);
    };
    // The wrapper has exited. Close its Job before joining the pipe readers: a leftover grandchild
    // in the same Job must not keep the pipes open and let a read outlive the deadline.
    #[cfg(windows)]
    if let Some(job) = job.take() {
        job.terminate();
    }
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    let _ = in_thread.join();
    let status_success = status.is_some_and(|status| status.success());
    if timed_out || status.is_none() {
        return Err(CliError::Timeout);
    }
    Ok(Output {
        status_success,
        stdout,
        stderr,
        timed_out,
    })
}

fn read_bounded<R: std::io::Read>(mut reader: R) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if buffer.len() + count > MAX_OUTPUT {
                    buffer.truncate(MAX_OUTPUT);
                    break;
                }
                buffer.extend_from_slice(&chunk[..count]);
            }
        }
    }
    buffer
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

/// `t3 thread list --project <selector> --json` in the frozen data directory.
pub fn list(selector: &str, data_dir: &str) -> Result<Vec<ListedThread>, CliError> {
    let arguments = vec![
        "thread".to_string(),
        "list".to_string(),
        "--project".to_string(),
        selector.to_string(),
        "--json".to_string(),
        "--base-dir".to_string(),
        data_dir.to_string(),
    ];
    let output = run_with(None, &arguments, None, data_dir, QUERY_TIMEOUT)?;
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

#[cfg(windows)]
fn spawn_with_job(
    mut command: Command,
) -> Result<(std::process::Child, Option<JobObject>), CliError> {
    // Spawn normally, then attach the job immediately. The direct Electron entrypoint has no
    // grandchild, and the static cmd wrapper's grandchild starts only after cmd reads its script.
    // This avoids the fragile suspend/resume of the initial thread on newer Windows builds.
    let mut child = command.spawn().map_err(|_| CliError::LauncherMissing)?;
    let Some(job) = attach_job_object(&child) else {
        let _ = child.kill();
        return Err(CliError::LauncherMissing);
    };
    Ok((child, Some(job)))
}

#[cfg(windows)]
struct JobObject(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl JobObject {
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn attach_job_object(child: &std::process::Child) -> Option<JobObject> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        ) == 0
            || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
        {
            windows_sys::Win32::Foundation::CloseHandle(job);
            return None;
        }
        Some(JobObject(job))
    }
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

        let Some(_launcher) = resolve_launcher() else {
            eprintln!("SKIP: T3 launcher is not installed");
            return;
        };
        let Some(data_dir) = t3_home() else {
            eprintln!("SKIP: no T3 data directory");
            return;
        };
        let selector = std::env::var("RELAY_T3_TEST_PROJECT").unwrap_or_default();
        if selector.is_empty() {
            eprintln!("SKIP: set RELAY_T3_TEST_PROJECT to a disposable registered project");
            return;
        }
        let probe_arguments = vec![
            "thread".to_string(),
            "list".to_string(),
            "--project".to_string(),
            selector.clone(),
            "--json".to_string(),
            "--base-dir".to_string(),
            data_dir.to_string_lossy().to_string(),
        ];
        let probe = run_with(
            None,
            &probe_arguments,
            None,
            &data_dir.to_string_lossy(),
            QUERY_TIMEOUT,
        );
        if probe.is_err() {
            eprintln!("SKIP: T3 server or test project is unavailable");
            return;
        }
        let title = format!("Relay Task {}", new_id());
        let brief = "Read-only smoke: reply with the single word OK. Do not edit any files.";
        let started = start(&selector, &data_dir.to_string_lossy(), &title, brief, None).unwrap();
        assert!(
            matches!(started, LaunchClass::Responded(_)),
            "the disposable start must succeed"
        );
        // Discard the response at the test boundary and reconcile from list + show alone.
        let candidates: Vec<ListedThread> = list(&selector, &data_dir.to_string_lossy())
            .unwrap()
            .into_iter()
            .filter(|thread| thread.title.as_deref() == Some(title.as_str()))
            .collect();
        assert_eq!(candidates.len(), 1, "exactly one exact attempt-title match");
        let identity = show(&candidates[0].thread_id, &data_dir.to_string_lossy()).unwrap();
        assert_eq!(identity.thread_id, candidates[0].thread_id);
        assert_eq!(identity.project_id, selector);
    }
}
