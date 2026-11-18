//! The headless reviewer (PLAN 3.3): one `opencode run` per analysis job, in an isolated
//! config that only loads the analyst MCP, with a 3-minute timeout and a Win32 Job Object
//! that kills the whole process tree.
//!
//! The job row is the source of truth: the agent must call `jobs_complete`; if it exits
//! without doing so, the job is marked failed instead of being retried blindly.

use std::fs;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use relay_core::store::Store;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::mcp;

const TIMEOUT: Duration = Duration::from_secs(180);
/// The mechanical model the user picked for reviewer work.
const MODEL: &str = "opencode-go/deepseek-v4.1-flash";
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts one job unless another reviewer run is already active. A job that is skipped
/// stays `pending_agent` for the next capture or a manual run.
pub fn spawn_job(app: AppHandle, job_id: String) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = run(&app, &job_id);
        RUNNING.store(false, Ordering::SeqCst);
        finish(&app, &job_id, result);
    });
}

fn finish(app: &AppHandle, job_id: &str, result: Result<(), String>) {
    let store = app.state::<std::sync::Arc<Store>>();
    if let Ok(job) = store.get_job(job_id)
        && job.state == "done"
    {
        let _ = app.emit("relay://change", ());
        return;
    }
    if let Err(error) = result {
        let _ = store.set_job_status(job_id, "failed", None, Some(&error));
    } else if let Ok(job) = store.get_job(job_id)
        && job.state != "done"
    {
        let _ = store.set_job_status(
            job_id,
            "failed",
            None,
            Some("the reviewer exited without jobs_complete"),
        );
    }
    let _ = app.emit("relay://change", ());
    run_next(app);
}

/// Picks up the next queued job after a run, so a burst does not strand jobs.
fn run_next(app: &AppHandle) {
    let store = app.state::<std::sync::Arc<Store>>();
    if let Ok(jobs) = store.list_jobs(Some("pending_agent"), 1)
        && let Some(job) = jobs.first()
    {
        spawn_job(app.clone(), job.id.clone());
    }
}

fn run(app: &AppHandle, job_id: &str) -> Result<(), String> {
    let store = app.state::<std::sync::Arc<Store>>();
    let job = store.get_job(job_id).map_err(|error| error.to_string())?;
    if !matches!(job.state.as_str(), "pending" | "pending_agent") {
        return Err(format!("job {job_id} is {}, not runnable", job.state));
    }
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let dir = base.join("analyst");
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let token = fs::read_to_string(base.join("mcp-analyst.token"))
        .map_err(|error| format!("cannot read the analyst token: {error}"))?
        .trim()
        .to_string();
    if token.is_empty() || !mcp_ready(store.as_ref()) {
        return Err("the MCP server is not running".into());
    }

    let config = dir.join("opencode.json");
    fs::write(&config, config_json(&token)).map_err(|error| error.to_string())?;
    let log_path = dir.join(format!("run-{job_id}.log"));
    let log = fs::File::create(&log_path).map_err(|error| error.to_string())?;
    let log_err = log.try_clone().map_err(|error| error.to_string())?;

    store
        .set_job_status(job_id, "running", None, None)
        .map_err(|error| error.to_string())?;

    let mut command = Command::new("cmd");
    command
        .arg("/C")
        .arg("opencode")
        .arg("--standalone")
        .arg("run")
        .arg("--agent")
        .arg("relay-analyst")
        .arg("--model")
        .arg(MODEL)
        .arg("--archived")
        .arg("--auto")
        .arg("--format")
        .arg("json")
        .arg("--title")
        .arg(format!("relay:{job_id}"))
        .arg(prompt(job_id))
        .current_dir(&dir)
        .env("OPENCODE_CONFIG", &config)
        // A private server plus an empty config home: OpenCode must not merge the user's
        // global config (which loads Executor and other MCP servers) into the reviewer.
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start opencode: {error}"))?;
    #[cfg(windows)]
    let job_object = attach_job_object(&child);

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(format!("cannot wait for opencode: {error}")),
        }
        if started.elapsed() >= TIMEOUT {
            #[cfg(windows)]
            if let Some(job) = &job_object {
                job.terminate();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "the reviewer timed out after {}s; log: {}",
                TIMEOUT.as_secs(),
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    };

    if !status.success() {
        return Err(format!(
            "opencode exited with {status}; log: {}",
            log_path.display()
        ));
    }
    // Tripwire: the reviewer must never reach Executor or another app's MCP server.
    let log_text = fs::read_to_string(&log_path).unwrap_or_default();
    for tool in ["executor", "hopper", "spawner"] {
        if log_text.contains(tool) {
            return Err(format!(
                "the reviewer reached the {tool} MCP server; its config is not isolated (log: {})",
                log_path.display()
            ));
        }
    }
    Ok(())
}

fn prompt(job_id: &str) -> String {
    format!(
        "Relay reviewer job {job_id}. Call jobs_get, apply the change with the analyst tools, and finish with jobs_complete."
    )
}

fn config_json(token: &str) -> String {
    let config = json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": {
            "servers": {
                "relay-analyst": {
                    "type": "remote",
                    "url": mcp::analyst_url(mcp::DEFAULT_PORT),
                    "headers": { "Authorization": format!("Bearer {token}") }
                }
            }
        },
        "agents": {
            "relay-analyst": {
                "description": "Relay's Slack reviewer",
                "mode": "primary",
                "model": MODEL,
                "system": crate::mcp::analyst_instructions(),
                "permissions": [
                    { "action": "relay-analyst_*", "resource": "*", "effect": "allow" },
                    { "action": "relay-analyst:*", "resource": "*", "effect": "allow" },
                    { "action": "shell", "resource": "*", "effect": "deny" },
                    { "action": "edit", "resource": "*", "effect": "deny" },
                    { "action": "read", "resource": "*", "effect": "deny" },
                    { "action": "glob", "resource": "*", "effect": "deny" },
                    { "action": "grep", "resource": "*", "effect": "deny" },
                    { "action": "webfetch", "resource": "*", "effect": "deny" },
                    { "action": "websearch", "resource": "*", "effect": "deny" },
                    { "action": "subagent", "resource": "*", "effect": "deny" }
                ]
            }
        }
    });
    serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".into())
}

fn mcp_ready(store: &Store) -> bool {
    store.list_jobs(None, 1).is_ok()
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
fn attach_job_object(child: &Child) -> Option<JobObject> {
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
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        );
        AssignProcessToJobObject(job, child.as_raw_handle() as _);
        Some(JobObject(job))
    }
}
