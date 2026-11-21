//! Tauri-side on-demand Slack thread reads. All policy stays in `relay_core::v2::slack_thread`;
//! this module only chooses a process runner, keeps the CLI off the async thread, and persists the
//! read-only snapshot through #30's shared recovery row. A paused link resumes when Settings
//! publishes a changed connector status, never because a scheduled pass probed authentication.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use relay_core::claude_slack::{self as core, ClaudeError, Runner};
use relay_core::model::now;
use relay_core::v2::{
    NewRecovery, ReferenceKey, ThreadFailure, ThreadResolution, V2Store, decide_failure,
};
use tauri::{AppHandle, Emitter, Manager};

use super::claude::{ProcessRunner, find_claude};
use crate::live::REFERENCE_EVENT;
use crate::settings::{self, ProviderCache};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Used only when `claude` is absent. The core resolver answers a SQLite hit before calling the
/// runner, so cached content still shows while the binary is missing; a miss maps to
/// `ThreadFailure::MissingCli` and pauses recovery until the install changes.
struct MissingCli;

impl Runner for MissingCli {
    fn run(&self, _request: &core::RunRequest) -> Result<String, ClaudeError> {
        Err(ClaudeError::NotFound)
    }
}

/// Runs one cache-first resolution on a blocking thread. The process runner is built here so a
/// missing binary is a typed configuration failure, not an exception in the resolver.
pub async fn resolve(
    app: &AppHandle,
    store: &Arc<V2Store>,
    key: &ReferenceKey,
) -> ThreadResolution {
    let now = now();
    let runner: Box<dyn Runner + Send> = match find_claude() {
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
    };
    let store = store.clone();
    let key = key.clone();
    let task_key = key.clone();
    let task_now = now.clone();
    tokio::task::spawn_blocking(move || {
        relay_core::v2::resolve_thread(&store, &*runner, &task_key, &task_now)
    })
    .await
    .unwrap_or_else(|_| {
        relay_core::v2::slack_thread::failed_resolution(&key, &ThreadFailure::Pending, &now)
    })
}

/// Writes the read-only Lens snapshot, announces it to both windows and applies #30's recovery
/// decision. A fresh failure enrols one spaced-retry row; an unreadable link with no cached copy
/// also gets a reason snapshot so reopening it after restart shows the linked ID and why.
pub async fn persist(
    app: &AppHandle,
    store: &Arc<V2Store>,
    key: &ReferenceKey,
    resolution: &ThreadResolution,
) {
    let at = now();
    let ms = now_ms();
    if let Some(snapshot) = &resolution.row.snapshot {
        let keep_cached = resolution.thread.is_some()
            || store
                .reference_snapshot_json(key.kind(), &key.target_id())
                .ok()
                .flatten()
                .is_none();
        if keep_cached {
            let _ = store.save_reference_snapshot(key, snapshot);
        }
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
