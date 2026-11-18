//! Claude Slack proxy transport: runs headless `claude -p` through `relay_core::claude_slack`
//! and feeds what it returns to the same ingestion and JEV path as the app-token transport.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use relay_core::claude_slack::{self as core, ClaudeError, RunRequest, Runner};
use relay_core::store::Store;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::{SlackState, SlackSyncReport, paused, publish};

const TRANSPORT_KEY: &str = "slack.transport";
const RUN_TIMEOUT: Duration = Duration::from_secs(90);
const INTERVAL: Duration = Duration::from_secs(10 * 60);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Claude,
    Token,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Claude => "claude",
            Transport::Token => "token",
        }
    }
}

/// The saved choice, else Claude when there is no Slack token and `claude` exists.
pub fn transport(store: &Store) -> Transport {
    match store.get_setting(TRANSPORT_KEY).ok().flatten().as_deref() {
        Some("claude") => Transport::Claude,
        Some("token") => Transport::Token,
        _ => {
            if super::read_secret(super::USER_TOKEN_KEY).is_none() && find_claude().is_some() {
                Transport::Claude
            } else {
                Transport::Token
            }
        }
    }
}

pub fn set_transport(store: &Store, value: &str) -> Result<(), String> {
    if value != "claude" && value != "token" {
        return Err("transport is claude or token".into());
    }
    store
        .set_setting(TRANSPORT_KEY, value)
        .map_err(|error| error.to_string())
}

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
        let mut command = Command::new(&self.claude);
        command
            .args(core::cli_args(request, &settings))
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
                Ok(None) if started.elapsed() > RUN_TIMEOUT => {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatus {
    pub found: bool,
    pub path: Option<String>,
    pub running: bool,
    pub last_run_at: Option<String>,
    pub last_messages: usize,
    pub last_cost_usd: f64,
    pub cost_today_usd: f64,
    pub last_duration_ms: u64,
    pub last_error: Option<String>,
}

pub fn status(store: &Store, running: bool) -> ClaudeStatus {
    let path = find_claude();
    let runs = core::load_runs(store);
    let last = runs.last();
    ClaudeStatus {
        found: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        running,
        last_run_at: last.map(|run| run.at.clone()),
        last_messages: last.map_or(0, |run| run.messages),
        last_cost_usd: last.map_or(0.0, |run| run.cost_usd),
        cost_today_usd: core::cost_today(&runs, &relay_core::model::now()),
        last_duration_ms: last.map_or(0, |run| run.duration_ms),
        last_error: last.and_then(|run| run.error.clone()),
    }
}

/// One sync: claude calls, ingest, then JEV over the changed episodes. Never overlaps another
/// Slack check (the guard is shared with the app-token transport).
pub async fn sync(app: &AppHandle) -> Result<SlackSyncReport, String> {
    if paused(app) {
        return Err("Capture is paused. Resume it, then check again.".into());
    }
    let state = app.state::<SlackState>();
    let _guard = state.begin_sync()?;
    let claude = find_claude().ok_or_else(|| ClaudeError::NotFound.to_string())?;
    let cwd = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("claude-slack");
    std::fs::create_dir_all(&cwd).map_err(|error| error.to_string())?;

    let store: Arc<Store> = state.store.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let runner = ProcessRunner { claude, cwd };
    let outcome = tokio::task::spawn_blocking(move || core::sync(&store, &runner, now))
        .await
        .map_err(|error| error.to_string())?;

    let mut report = SlackSyncReport {
        conversations: outcome.channels,
        messages: outcome.messages,
        episodes: outcome.episodes.len(),
        analyzed: 0,
        error: outcome.error.clone(),
    };
    if state.jev().is_some() {
        for (episode_id, historical) in outcome.episodes.into_iter().take(super::CATCHUP_ANALYSES) {
            match super::analyze(app, &state, &episode_id, historical).await {
                Ok(_) => report.analyzed += 1,
                Err(error) => {
                    report.error.get_or_insert(error);
                }
            }
        }
    }
    let status = state.update(|status| {
        status.configured = true;
        status.last_event_at = Some(relay_core::model::now());
        status.open_episodes = state
            .store
            .list_slack_episodes(Some("open"), 200)
            .map(|episodes| episodes.len())
            .unwrap_or(0);
        status.error = report.error.clone();
    });
    publish(app, &status);
    let _ = app.emit("relay://change", ());
    match report.error.clone() {
        Some(error) if report.messages == 0 => Err(error),
        _ => Ok(report),
    }
}

/// Every 10 minutes while the app runs and the Claude transport is active.
pub fn start_schedule(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let store = app.state::<SlackState>().store.clone();
            if transport(&store) == Transport::Claude && find_claude().is_some() {
                let _ = sync(&app).await;
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual smoke against the real connector: `cargo test -p relay --lib real_smoke -- --ignored
    /// --nocapture`. Syncs the last day into an in-memory store; prints counts and cost only.
    #[test]
    #[ignore]
    fn real_smoke() {
        let claude = find_claude().expect("claude not found");
        let cwd = std::env::temp_dir().join("relay-claude-slack-smoke");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = Store::open_in_memory().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        store
            .set_setting("claude_slack.watermark", &now.to_string())
            .unwrap();
        let out = core::sync(&store, &ProcessRunner { claude, cwd }, now);
        println!(
            "SMOKE channels={} messages={} episodes={} calls={} invocations={} cost_usd={:.4} seconds={:.1} error={:?}",
            out.channels,
            out.messages,
            out.episodes.len(),
            out.calls,
            out.invocations,
            out.cost_usd,
            out.duration_ms as f64 / 1000.0,
            out.error
        );
        println!(
            "SMOKE disallow_entries={}",
            core::disallow_list(&store).len()
        );
    }
}
