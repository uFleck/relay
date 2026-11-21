//! Headless `claude -p` transport for v2's on-demand Slack reads. The core owns the
//! prompt, tool allowlist and stream parsing; this module only locates the CLI, builds
//! its isolated settings and runs it off the async thread. No token is read or stored.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use relay_core::claude_slack::{self as core, ClaudeError, RunRequest, Runner};

const RUN_TIMEOUT: Duration = Duration::from_secs(90);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// `claude` on PATH, else `%USERPROFILE%\.local\bin\claude.exe`.
pub fn find_claude() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["claude.exe", "claude.cmd"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let fallback = user_home()?.join(".local").join("bin").join("claude.exe");
    fallback.is_file().then_some(fallback)
}

/// Every plugin the user enabled, to switch off for the run. Read only, never written.
fn enabled_plugins() -> Vec<String> {
    let Some(home) = user_home() else {
        return Vec::new();
    };
    std::fs::read_to_string(home.join(".claude").join("settings.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("enabledPlugins")
                .and_then(|p| p.as_object().cloned())
        })
        .map(|plugins| plugins.keys().cloned().collect())
        .unwrap_or_default()
}

pub struct ProcessRunner {
    pub claude: PathBuf,
    pub cwd: PathBuf,
}

impl Runner for ProcessRunner {
    fn run(&self, request: &RunRequest) -> Result<String, ClaudeError> {
        let settings = core::run_settings_json(&enabled_plugins());
        self.run_claude(
            core::cli_args(request, &settings),
            &core::RUN_ENV,
            request.timeout.unwrap_or(RUN_TIMEOUT),
        )
    }
}

impl ProcessRunner {
    fn run_claude(
        &self,
        args: Vec<String>,
        env: &[(&str, &str)],
        deadline: Duration,
    ) -> Result<String, ClaudeError> {
        let mut command = Command::new(&self.claude);
        command
            .args(args)
            .envs(env.iter().copied())
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|error| ClaudeError::Cli(format!("could not start claude: {error}")))?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let out = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let err = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ClaudeError::Timeout);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(error) => return Err(ClaudeError::Cli(error.to_string())),
            }
        }
        let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
        if stdout.trim().is_empty() {
            let detail = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
            let detail: String = detail.trim().chars().take(200).collect();
            return Err(ClaudeError::Cli(if detail.is_empty() {
                "claude produced no output".into()
            } else {
                detail
            }));
        }
        Ok(stdout)
    }
}
