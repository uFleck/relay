//! Background recovery worker. Core owns the schedule and reason policy; this module runs it
//! against the v2 database, resolves credentials, and announces a successful read to both
//! windows. Every SQLite read returns owned rows and drops the connection before a network await.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use relay_core::model::now;
use relay_core::provider::ProviderFailure;
use relay_core::v2::{
    MAX_IN_FLIGHT, ReferenceKey, V2Store, decide_failure, decide_success, read_exact,
};
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::live::{self, REFERENCE_EVENT};
use crate::settings;

const TICK: std::time::Duration = std::time::Duration::from_secs(5);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Starts the worker at Tauri startup against the v2 store. A provider-status change triggers
/// an immediate pass so a paused target is re-evaluated as soon as Settings changes.
pub fn start(app: AppHandle) {
    let (wake_tx, mut wake_rx) = tokio::sync::mpsc::channel::<()>(1);
    let listener = app.clone();
    let listener_tx = wake_tx.clone();
    tauri::async_runtime::spawn(async move {
        let listener_id = listener.listen("relay://settings-status", move |_| {
            let _ = listener_tx.try_send(());
        });
        std::future::pending::<()>().await;
        listener.unlisten(listener_id);
    });

    let ticker = wake_tx.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            if ticker.try_send(()).is_err() {
                // A pass is already queued; no need to pile up more wakeups.
            }
        }
    });

    tauri::async_runtime::spawn(async move {
        while wake_rx.recv().await.is_some() {
            run_once(&app).await;
        }
    });
}

async fn run_once(app: &AppHandle) {
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let at = now();
    let ms = now_ms();

    // A changed credential fingerprint unpauses target work; an unchanged one keeps the pause
    // so no request is issued against a known sign-in or configuration failure.
    let versions = [
        (
            "github".to_string(),
            settings::credential_fingerprint("github").await,
        ),
        (
            "clickup".to_string(),
            settings::credential_fingerprint("clickup").await,
        ),
    ];
    for (provider, version) in &versions {
        let _ = store.unpause_changed_config(provider, version, &at, ms);
    }

    let due = match store.due_recoveries(ms, MAX_IN_FLIGHT) {
        Ok(due) => due,
        Err(_) => return,
    };
    let creds = live::live_credentials(&store).await;
    let mut in_flight = HashSet::new();

    for target in due {
        if !in_flight.insert(target.key.clone()) {
            continue;
        }
        let expected = target.updated_at.clone();
        let key = target.key.clone();
        let version = versions
            .iter()
            .find(|(provider, _)| provider == &target.provider)
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let parsed = ReferenceKey::from_storage(
            &target.kind,
            &target.target_id,
            Some(&target.canonical_url),
        );
        let outcome = match parsed {
            Some(identity) => read_exact(&identity, &creds, &at).await,
            None => Err(ProviderFailure::Unavailable),
        };
        let (update, snapshot_json, recovered) = match outcome {
            Ok(row) => (
                decide_success(),
                row.snapshot.map(|snapshot| snapshot.to_json()),
                true,
            ),
            Err(failure) => (
                decide_failure(Some(&target), &failure, ms, &version),
                None,
                false,
            ),
        };
        let applied = store
            .record_recovery(&key, &expected, &update, snapshot_json.as_deref(), &now())
            .unwrap_or(false);
        in_flight.remove(&key);
        if applied && recovered {
            let _ = app.emit(REFERENCE_EVENT, key);
            let _ = app.emit("relay://v2-change", ());
        }
    }
}
