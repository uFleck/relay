//! Tauri-side fixed-window reads. Core owns the frozen bounds and content; this module only
//! chooses a process runner, keeps the CLI off the async thread and persists the read-only Lens
//! snapshot through #30's shared recovery row.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use relay_core::claude_slack::{self as core, ClaudeError, Runner};
use relay_core::model::{new_id, now};
use relay_core::v2::{
    NewRecovery, ReferenceKey, ReferenceSnapshot, V2PickerRow, V2Store, WindowFailure,
    WindowResolution, decide_failure, window_bounds,
};
use tauri::{AppHandle, Emitter, Manager, State};

use super::claude::{ProcessRunner, find_claude};
use crate::live::REFERENCE_EVENT;
use crate::settings::{self, ProviderCache};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

struct MissingCli;

impl Runner for MissingCli {
    fn run(&self, _request: &core::RunRequest) -> Result<String, ClaudeError> {
        Err(ClaudeError::NotFound)
    }
}

fn runner(app: &AppHandle) -> Box<dyn Runner + Send> {
    match find_claude() {
        Some(claude) => {
            let cwd = app
                .path()
                .app_data_dir()
                .map(|dir| dir.join("claude-slack"))
                .unwrap_or_else(|_| std::env::temp_dir().join("relay-claude-slack"));
            let _ = std::fs::create_dir_all(&cwd);
            Box::new(ProcessRunner { claude, cwd })
        }
        None => Box::new(MissingCli),
    }
}

/// Creates a distinct window Reference before any read starts. The chip can be inserted from the
/// returned row immediately; the Lens shows "fetching…" until `resolve` replaces it.
#[tauri::command]
pub fn v2_slack_window_start(
    channel_id: String,
    host: Option<String>,
    name: Option<String>,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<V2PickerRow, String> {
    let channel_id = channel_id.trim();
    if channel_id.is_empty() {
        return Err("A conversation needs a channel id.".into());
    }
    let host = host
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "slack.com".to_string());
    let at = now();
    let (oldest, latest) = window_bounds(&at).ok_or("The selection time could not be read.")?;
    let id = new_id();
    let link = format!("https://{host}/archives/{channel_id}");
    let window = relay_core::v2::SlackWindow {
        id: id.clone(),
        channel_id: channel_id.to_string(),
        selected_at: at.clone(),
        oldest,
        latest,
        messages: Vec::new(),
        link: link.clone(),
        ready: false,
    };
    store
        .save_window(&window)
        .map_err(|error| error.to_string())?;
    let key = ReferenceKey::SlackWindow {
        id: id.clone(),
        channel_id: channel_id.to_string(),
        host: host.clone(),
        name: name.clone(),
    };
    let snapshot = ReferenceSnapshot {
        title: Some("fetching…".to_string()),
        cached_at: Some(at.clone()),
        channel_name: name.clone(),
        ..ReferenceSnapshot::default()
    };
    store
        .save_reference_snapshot(&key, &snapshot)
        .map_err(|error| error.to_string())?;
    let alias = name.as_deref().filter(|name| !name.is_empty());
    let row = V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(alias),
        canonical_link: key.canonical_url(),
        availability: relay_core::v2::ReferenceAvailability::Ready,
        group: relay_core::v2::V2PickerGroup::Conversations,
        title: "fetching…".to_string(),
        state: None,
        metadata: Vec::new(),
        snapshot: Some(snapshot),
        unavailable_reason: None,
        settings_destination: None,
        updated_at: at,
        exact_match: true,
    };
    let _ = app.emit(REFERENCE_EVENT, row.key.clone());
    Ok(row)
}

/// Runs one window resolution on a blocking thread. The process runner is built here so a
/// missing binary is typed as a configuration failure, not an exception in the resolver.
pub async fn resolve(
    app: &AppHandle,
    store: &Arc<V2Store>,
    key: &ReferenceKey,
) -> WindowResolution {
    let now = now();
    let runner = runner(app);
    let store = store.clone();
    let key = key.clone();
    let task_key = key.clone();
    let task_now = now.clone();
    tokio::task::spawn_blocking(move || {
        relay_core::v2::resolve_window(&store, &*runner, &task_key, &task_now)
    })
    .await
    .unwrap_or_else(|_| {
        relay_core::v2::slack_window::failed_window(&key, &WindowFailure::Pending, &now)
    })
}

/// Writes the read-only Lens snapshot and applies #30's recovery decision. A frozen window keeps
/// its bounds and source link in the snapshot that #34 reads.
pub async fn persist(
    app: &AppHandle,
    store: &Arc<V2Store>,
    key: &ReferenceKey,
    resolution: &WindowResolution,
) {
    let at = now();
    let ms = now_ms();
    if let Some(snapshot) = &resolution.row.snapshot {
        store.save_reference_snapshot(key, snapshot).ok();
    }
    match &resolution.failure {
        None => {
            let _ = store.clear_recovery(&key.identity());
        }
        Some(failure) => {
            let _ = store.ensure_recovery(
                &[NewRecovery {
                    key: key.identity(),
                    kind: key.kind().to_string(),
                    target_id: key.target_id(),
                    canonical_url: key.canonical_url(),
                    provider: "slack".to_string(),
                }],
                &at,
                ms,
            );
            let target = store.recovery_target(&key.identity()).ok().flatten();
            let expected = target
                .as_ref()
                .map(|target| target.updated_at.clone())
                .unwrap_or_default();
            let version =
                settings::credential_fingerprint("slack", &app.state::<ProviderCache>()).await;
            let update = decide_failure(
                target.as_ref(),
                &failure.to_provider_failure(),
                ms,
                &version,
            );
            let _ = store.record_recovery(&key.identity(), &expected, &update, None, &at);
        }
    }
    let _ = app.emit(REFERENCE_EVENT, key.identity());
    let _ = app.emit("relay://v2-change", ());
}
