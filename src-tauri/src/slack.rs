//! Slack capture runtime (PLAN 3.1, P2): credential storage, Socket Mode, catch-up on
//! reconnect, and the JEV job that follows an episode.
//!
//! The socket delivers `message.im` frames; Relay acks each envelope first and then
//! stores the message through `relay-core`. The store owns episode segmentation, peer
//! exclusion and retention; this module owns the connection and the JEV call.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use relay_core::analysis::{self, CandidateTask, ProjectChoice, TranscriptLine};
use relay_core::jev::{self, JevClient, Route, Thresholds};
use relay_core::model::{
    AnalysisJob, EpisodeDetail, EpisodeSummary, NewJob, RawSlackMessage, SlackPeer, StatusCategory,
    TaskFilter,
};
use relay_core::slack::{self, InboundEvent, SlackClient, SlackIdentity, SocketFrame};
use relay_core::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use crate::hover::HoverState;

const USER_TOKEN_KEY: &str = "slack-user-token";
const APP_TOKEN_KEY: &str = "slack-app-token";
const JEV_KEY: &str = "jev-api-key";
/// 14 days of DM history on the first connect (PLAN 3.1).
const BACKFILL_DAYS: i64 = 14;
/// How many changed episodes one catch-up run may send to JEV.
const CATCHUP_ANALYSES: usize = 20;
/// The status event the Settings view listens to.
pub const STATUS_EVENT: &str = "slack://status";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackStatus {
    pub configured: bool,
    pub connected: bool,
    pub team: Option<String>,
    pub user: Option<String>,
    pub error: Option<String>,
    pub last_event_at: Option<String>,
    pub capture_paused: bool,
    pub peers: usize,
    pub open_episodes: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevStatus {
    pub configured: bool,
    pub model: Option<String>,
    pub error: Option<String>,
}

struct Inner {
    status: SlackStatus,
    stop: Option<watch::Sender<bool>>,
    names: HashMap<String, String>,
}

pub struct SlackState {
    store: Arc<Store>,
    inner: Mutex<Inner>,
    jev: Mutex<Option<Arc<JevClient>>>,
}

impl SlackState {
    fn new(store: Arc<Store>) -> Self {
        let jev = read_secret(JEV_KEY).and_then(|key| JevClient::new(key).ok());
        Self {
            store,
            inner: Mutex::new(Inner {
                status: SlackStatus::default(),
                stop: None,
                names: HashMap::new(),
            }),
            jev: Mutex::new(jev.map(Arc::new)),
        }
    }

    pub fn status(&self) -> SlackStatus {
        self.inner.lock().expect("slack state").status.clone()
    }

    fn update(&self, change: impl FnOnce(&mut SlackStatus)) -> SlackStatus {
        let mut inner = self.inner.lock().expect("slack state");
        change(&mut inner.status);
        inner.status.clone()
    }

    fn set_error(&self, error: Option<String>) -> SlackStatus {
        self.update(|status| status.error = error)
    }

    fn set_connected(&self, connected: bool, error: Option<String>) -> SlackStatus {
        self.update(|status| {
            status.connected = connected;
            status.error = error;
        })
    }

    fn name_cache(&self, user_id: &str) -> Option<String> {
        self.inner
            .lock()
            .expect("slack state")
            .names
            .get(user_id)
            .cloned()
    }

    fn remember_name(&self, user_id: &str, name: &str) {
        self.inner
            .lock()
            .expect("slack state")
            .names
            .insert(user_id.to_string(), name.to_string());
    }

    fn jev(&self) -> Option<Arc<JevClient>> {
        self.jev.lock().expect("jev state").clone()
    }

    fn take_loop(&self) -> watch::Receiver<bool> {
        let mut inner = self.inner.lock().expect("slack state");
        if let Some(previous) = inner.stop.take() {
            let _ = previous.send(true);
        }
        let (tx, rx) = watch::channel(false);
        inner.stop = Some(tx);
        rx
    }

    fn stop_loop(&self) {
        let mut inner = self.inner.lock().expect("slack state");
        if let Some(previous) = inner.stop.take() {
            let _ = previous.send(true);
        }
    }
}

/// Installs the runtime and reconnects when tokens are already stored.
pub fn start(app: AppHandle, store: Arc<Store>) {
    let state = SlackState::new(store);
    app.manage(state);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some((user_token, app_token)) = load_tokens() else {
            return;
        };
        let state = app.state::<SlackState>();
        let _ = state.set_error(None);
        if let Err(error) = connect(&app, &state, user_token, app_token).await {
            publish(&app, &state.set_error(Some(error)));
        }
    });
}

async fn connect(
    app: &AppHandle,
    state: &SlackState,
    user_token: String,
    app_token: String,
) -> Result<(), String> {
    let client =
        SlackClient::new(user_token, Some(app_token)).map_err(|error| error.to_string())?;
    let identity = client
        .auth_test()
        .await
        .map_err(|error| error.to_string())?;
    client
        .open_socket()
        .await
        .map_err(|error| error.to_string())?;

    let status = state.update(|status| {
        status.configured = true;
        status.connected = false;
        status.error = None;
        status.team = Some(identity.team.clone());
        status.user = Some(identity.user.clone());
    });
    publish(app, &status);

    let stop = state.take_loop();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_socket(app, client, identity, stop).await;
    });
    Ok(())
}

async fn run_socket(
    app: AppHandle,
    client: SlackClient,
    identity: SlackIdentity,
    mut stop: watch::Receiver<bool>,
) {
    let mut backoff = 1u64;
    loop {
        if *stop.borrow() {
            return;
        }
        let state = app.state::<SlackState>();
        match client.open_socket().await {
            Ok(url) => match tokio_tungstenite::connect_async(&url).await {
                Ok((mut socket, _)) => {
                    backoff = 1;
                    let status = state.set_connected(true, None);
                    publish(&app, &status);
                    loop {
                        tokio::select! {
                            changed = stop.changed() => {
                                if changed.is_ok() {
                                    let _ = socket.close(None).await;
                                    return;
                                }
                            }
                            frame = socket.next() => match frame {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    if !handle_frame(&app, &client, &identity, &mut socket, text.as_str(), &stop).await {
                                        break;
                                    }
                                }
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(payload))) => {
                                    let _ = socket.send(tokio_tungstenite::tungstenite::Message::Pong(payload)).await;
                                }
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break,
                                Some(Err(error)) => {
                                    eprintln!("relay: slack socket: {error}");
                                    break;
                                }
                                _ => {}
                            },
                        }
                    }
                    let status = state.set_connected(false, None);
                    publish(&app, &status);
                }
                Err(error) => {
                    let status = state.set_error(Some(format!("socket connect failed: {error}")));
                    publish(&app, &status);
                }
            },
            Err(error) => {
                let status =
                    state.set_error(Some(format!("apps.connections.open failed: {error}")));
                publish(&app, &status);
            }
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
        }
        backoff = (backoff * 2).min(60);
    }
}

/// Returns false when the socket should reconnect.
async fn handle_frame(
    app: &AppHandle,
    client: &SlackClient,
    identity: &SlackIdentity,
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    text: &str,
    stop: &watch::Receiver<bool>,
) -> bool {
    match slack::parse_socket_frame(text) {
        Ok(SocketFrame::Hello) => {
            let app = app.clone();
            let client = client.clone();
            let identity = identity.clone();
            let stop = stop.clone();
            tauri::async_runtime::spawn(async move {
                catch_up(app, client, identity, stop).await;
            });
            true
        }
        Ok(SocketFrame::Event {
            envelope_id,
            payload,
        }) => {
            // Ack first: Slack redelivers an envelope Relay does not ack in time.
            let ack = json!({ "envelope_id": envelope_id }).to_string();
            let _ = socket
                .send(tokio_tungstenite::tungstenite::Message::Text(ack.into()))
                .await;
            let app = app.clone();
            let client = client.clone();
            tauri::async_runtime::spawn(async move {
                handle_event(app, client, payload).await;
            });
            true
        }
        Ok(SocketFrame::Disconnect { reason }) => {
            eprintln!("relay: slack socket disconnect: {reason}");
            false
        }
        Ok(SocketFrame::Ignored) => true,
        Err(error) => {
            eprintln!("relay: slack frame: {error}");
            true
        }
    }
}

async fn handle_event(app: AppHandle, client: SlackClient, payload: serde_json::Value) {
    let state = app.state::<SlackState>();
    if paused(&app) {
        return;
    }
    match slack::event_from_payload(&payload) {
        InboundEvent::Message(message) => {
            if let Some(episode_id) = capture(&state, &client, &message).await {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<SlackState>();
                    match analyze(&app, &state, &episode_id, false).await {
                        Ok(_) => {
                            let status = state.update(|status| {
                                status.last_event_at = Some(relay_core::model::now());
                            });
                            publish(&app, &status);
                            let _ = app.emit("relay://change", ());
                        }
                        Err(error) => {
                            let status = state.set_error(Some(error));
                            publish(&app, &status);
                        }
                    }
                });
            }
        }
        InboundEvent::Ignored(reason) => {
            eprintln!("relay: slack event ignored: {reason}");
        }
    }
}

async fn catch_up(
    app: AppHandle,
    client: SlackClient,
    identity: SlackIdentity,
    stop: watch::Receiver<bool>,
) {
    let state = app.state::<SlackState>();
    let dms = match client.list_dms().await {
        Ok(dms) => dms,
        Err(error) => {
            let status = state.set_error(Some(format!("conversations.list failed: {error}")));
            publish(&app, &status);
            return;
        }
    };

    let mut changed: Vec<(String, bool)> = Vec::new();
    for dm in dms {
        if *stop.borrow() {
            return;
        }
        let cursor = state.store.slack_channel_cursor(&dm.id).unwrap_or(None);
        let historical = cursor.is_none();
        let oldest = cursor
            .clone()
            .unwrap_or_else(|| slack_ts_days_ago(BACKFILL_DAYS));
        let messages = match client.history(&dm.id, Some(&oldest), 200).await {
            Ok(messages) => messages,
            Err(error) => {
                let status =
                    state.set_error(Some(format!("conversations.history failed: {error}")));
                publish(&app, &status);
                continue;
            }
        };
        if messages.is_empty() {
            continue;
        }
        let _name = resolve_name(&state, &client, &dm.user_id).await;
        for mut message in messages {
            message.historical = historical;
            if let Some(episode_id) = capture(&state, &client, &message).await
                && !changed.iter().any(|(id, _)| id == &episode_id)
            {
                changed.push((episode_id, historical));
            }
        }
    }

    for (episode_id, historical) in changed.into_iter().take(CATCHUP_ANALYSES) {
        if *stop.borrow() {
            return;
        }
        let state = app.state::<SlackState>();
        if let Err(error) = analyze(&app, &state, &episode_id, historical).await {
            let status = state.set_error(Some(error));
            publish(&app, &status);
        }
    }

    let status = state.update(|status| {
        status.team = Some(identity.team.clone());
        status.user = Some(identity.user.clone());
        status.error = None;
        status.last_event_at = Some(relay_core::model::now());
        status.open_episodes = state
            .store
            .list_slack_episodes(Some("open"), 200)
            .map(|episodes| episodes.len())
            .unwrap_or(0);
    });
    publish(&app, &status);
    let _ = app.emit("relay://change", ());
}

async fn capture(
    state: &SlackState,
    client: &SlackClient,
    message: &RawSlackMessage,
) -> Option<String> {
    let peer_id = message.user_id.clone()?;
    let name = resolve_name(state, client, &peer_id).await;
    match state
        .store
        .capture_slack_message(&peer_id, name.as_deref(), message, 90)
    {
        Ok(Some(stored)) => Some(stored.episode_id),
        Ok(None) => None,
        Err(error) => {
            eprintln!("relay: slack capture: {error}");
            None
        }
    }
}

async fn resolve_name(state: &SlackState, client: &SlackClient, user_id: &str) -> Option<String> {
    if let Some(name) = state.name_cache(user_id) {
        return Some(name);
    }
    match client.user_name(user_id).await {
        Ok(Some(name)) => {
            state.remember_name(user_id, &name);
            Some(name)
        }
        Ok(None) => None,
        Err(error) => {
            eprintln!("relay: users.info {user_id}: {error}");
            None
        }
    }
}

fn paused(app: &AppHandle) -> bool {
    app.try_state::<HoverState>()
        .is_some_and(|state| state.snapshot().paused)
}

/// Runs JEV over one episode. `shadow` stores the answers but takes no action.
async fn analyze(
    app: &AppHandle,
    state: &SlackState,
    conversation_id: &str,
    shadow: bool,
) -> Result<Option<AnalysisJob>, String> {
    let jev = state.jev().ok_or("JEV is not configured")?;
    let detail = state
        .store
        .get_slack_episode(conversation_id)
        .map_err(|error| error.to_string())?;
    let peer_name = detail
        .peer
        .name
        .clone()
        .unwrap_or_else(|| detail.peer.user_id.clone());
    let lines: Vec<TranscriptLine> = detail
        .messages
        .iter()
        .filter(|message| !message.deleted)
        .map(|message| TranscriptLine {
            author: if message.direction == "in" {
                peer_name.clone()
            } else {
                "me".to_string()
            },
            text: message.text.clone(),
        })
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }
    let transcript = analysis::transcript(&lines);
    let thresholds = Thresholds::default();

    let screen = jev
        .evaluate(serde_json::json!(&transcript), analysis::screen_questions())
        .await
        .map_err(|error| format!("JEV screen failed: {error}"))?;
    let worth = screen.noul(jev::ids::WORTH_REVIEWING).unwrap_or(0.0);
    let mut answers = json!({ "screen": screen.answers });
    let watermark = detail
        .messages
        .last()
        .map(|message| message.ts.clone())
        .unwrap_or_default();

    let (kind, route_name, route, candidates) = if worth < thresholds.screen_no {
        ("screen", "ignore", Route::Ignore, Vec::new())
    } else {
        let candidates = candidates(&state.store, &transcript);
        let projects: Vec<ProjectChoice> = state
            .store
            .list_projects()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|project| ProjectChoice {
                slug: project.slug,
                name: project.name,
            })
            .collect();
        let candidates_json: Vec<serde_json::Value> = candidates
            .iter()
            .map(|candidate| {
                json!({
                    "id": candidate.id,
                    "title": candidate.title,
                    "status": candidate.status,
                })
            })
            .collect();
        let state_json = json!({
            "transcript": transcript,
            "candidates": candidates_json,
            "previous": screen.answers,
        });
        let classify = jev
            .evaluate(
                state_json,
                analysis::classify_questions(&candidates, &projects),
            )
            .await
            .map_err(|error| format!("JEV classify failed: {error}"))?;
        let combined = analysis::merge_answers(&screen, &classify);
        let route = jev::route(&combined, thresholds, false);
        answers["classify"] = json!(classify.answers);
        let route_name = match &route {
            Route::Ignore => "ignore",
            Route::Rescreen { .. } => "rescreen",
            Route::Review { .. } => "review",
            Route::Change { .. } => "change",
            Route::NewTask { .. } => "new_task",
        };
        (
            "classify",
            route_name,
            route,
            candidates
                .into_iter()
                .map(|candidate| candidate.id)
                .collect(),
        )
    };

    // A rescreen is not a job: the episode stays open and is looked at again on close.
    if matches!(route, Route::Rescreen { .. }) {
        return Ok(None);
    }

    let project_id = match &route {
        Route::NewTask { project } => project
            .as_deref()
            .and_then(|slug| {
                state
                    .store
                    .list_projects()
                    .ok()?
                    .into_iter()
                    .find(|project| project.slug == slug)
            })
            .map(|project| project.id),
        _ => None,
    };

    let job = state
        .store
        .create_job(NewJob {
            conversation_id: conversation_id.to_string(),
            watermark,
            kind: kind.to_string(),
            project_id,
            route: Some(route_name.to_string()),
            jev_json: Some(answers.to_string()),
            candidates: candidates.clone(),
        })
        .map_err(|error| error.to_string())?;

    let (job_state, summary) = if shadow {
        ("shadow", "backfill: answers stored, no action".to_string())
    } else {
        match &route {
            Route::Ignore => ("done", "screened: not work".to_string()),
            Route::Review { reason } => ("pending", format!("needs a human: {reason}")),
            Route::Change { .. } => ("pending_agent", "append to an existing task".to_string()),
            Route::NewTask { .. } => ("pending_agent", "create one new task".to_string()),
            Route::Rescreen { .. } => ("done", "rescreen at close".to_string()),
        }
    };
    let job = state
        .store
        .set_job_status(&job.id, job_state, Some(&summary), None)
        .map_err(|error| error.to_string())?;
    if job_state == "pending_agent" {
        crate::analyst::spawn_job(app.clone(), job.id.clone());
    }

    let episode_state = if shadow {
        "analyzed"
    } else {
        match &route {
            Route::Ignore => "analyzed",
            Route::Review { .. } => "needs_review",
            Route::Change { .. } => "waiting",
            Route::NewTask { .. } => "closed",
            Route::Rescreen { .. } => "open",
        }
    };
    let _ = state
        .store
        .set_slack_episode_state(conversation_id, episode_state);
    Ok(Some(job))
}

fn candidates(store: &Store, transcript: &str) -> Vec<CandidateTask> {
    let mut tasks = Vec::new();
    let words = keywords(transcript);
    if !words.is_empty()
        && let Ok(found) = store.search_tasks(&words.join(" "), 15)
    {
        tasks.extend(found);
    }
    if let Ok(active) = store.list_tasks(TaskFilter {
        category: Some(StatusCategory::Active),
        limit: Some(5),
        ..TaskFilter::default()
    }) {
        tasks.extend(active);
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for task in tasks {
        if !seen.insert(task.id.clone()) {
            continue;
        }
        let status = store
            .get_task_detail(&task.id, None)
            .map(|detail| detail.status.name)
            .unwrap_or_else(|_| "unknown".to_string());
        out.push(CandidateTask {
            id: task.id,
            title: task.title,
            status,
        });
        if out.len() == 20 {
            break;
        }
    }
    out
}

fn keywords(text: &str) -> Vec<String> {
    const STOP: [&str; 16] = [
        "that", "this", "with", "have", "from", "your", "about", "there", "would", "could",
        "should", "they", "what", "when", "will", "just",
    ];
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for word in text.split(|ch: char| !ch.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.len() < 4 || STOP.contains(&word.as_str()) || !seen.insert(word.clone()) {
            continue;
        }
        out.push(word);
        if out.len() == 5 {
            break;
        }
    }
    out
}

fn slack_ts_days_ago(days: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    format!("{}.000000", now - days * 86_400)
}

fn publish(app: &AppHandle, status: &SlackStatus) {
    let _ = app.emit(STATUS_EVENT, status);
}

fn read_secret(key: &str) -> Option<String> {
    crate::secrets::read(key)
}

fn write_secret(key: &str, value: &str) -> Result<(), String> {
    crate::secrets::write(key, value)
}

fn delete_secret(key: &str) {
    crate::secrets::delete(key);
}

fn load_tokens() -> Option<(String, String)> {
    let user = read_secret(USER_TOKEN_KEY)?;
    let app = read_secret(APP_TOKEN_KEY)?;
    Some((user, app))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn slack_status(app: AppHandle, state: State<'_, SlackState>) -> SlackStatus {
    let mut status = state.status();
    status.capture_paused = paused(&app);
    status.peers = state
        .store
        .list_slack_peers()
        .map(|peers| peers.len())
        .unwrap_or(0);
    status.open_episodes = state
        .store
        .list_slack_episodes(None, 200)
        .map(|episodes| {
            episodes
                .iter()
                .filter(|summary| {
                    matches!(
                        summary.episode.state.as_str(),
                        "open" | "waiting" | "needs_review"
                    )
                })
                .count()
        })
        .unwrap_or(0);
    status
}

#[tauri::command]
pub async fn slack_connect(
    app: AppHandle,
    state: State<'_, SlackState>,
    user_token: String,
    app_token: String,
) -> Result<SlackStatus, String> {
    let user_token = user_token.trim().to_string();
    let app_token = app_token.trim().to_string();
    if !user_token.starts_with("xoxp-") {
        return Err("the user token starts with xoxp-".into());
    }
    if !app_token.starts_with("xapp-") {
        return Err("the app-level token starts with xapp-".into());
    }
    connect(&app, &state, user_token.clone(), app_token.clone()).await?;
    write_secret(USER_TOKEN_KEY, &user_token)?;
    write_secret(APP_TOKEN_KEY, &app_token)?;
    let status = state.status();
    publish(&app, &status);
    Ok(status)
}

#[tauri::command]
pub fn slack_disconnect(app: AppHandle, state: State<'_, SlackState>) -> SlackStatus {
    state.stop_loop();
    delete_secret(USER_TOKEN_KEY);
    delete_secret(APP_TOKEN_KEY);
    let status = state.update(|status| {
        status.configured = false;
        status.connected = false;
        status.team = None;
        status.user = None;
        status.error = None;
    });
    publish(&app, &status);
    status
}

#[tauri::command]
pub fn slack_peers(state: State<'_, SlackState>) -> Result<Vec<SlackPeer>, String> {
    state
        .store
        .list_slack_peers()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn slack_set_peer_excluded(
    app: AppHandle,
    state: State<'_, SlackState>,
    peer_id: String,
    excluded: bool,
) -> Result<SlackPeer, String> {
    let peer = state
        .store
        .set_slack_peer_excluded(&peer_id, excluded)
        .map_err(|error| error.to_string())?;
    let _ = app.emit("relay://change", ());
    Ok(peer)
}

#[tauri::command]
pub fn episodes_list(
    state: State<'_, SlackState>,
    state_filter: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<EpisodeSummary>, String> {
    state
        .store
        .list_slack_episodes(state_filter.as_deref(), limit.unwrap_or(50))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn episode_get(state: State<'_, SlackState>, id: String) -> Result<EpisodeDetail, String> {
    state
        .store
        .get_slack_episode(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn jobs_list(
    state: State<'_, SlackState>,
    state_filter: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<AnalysisJob>, String> {
    state
        .store
        .list_jobs(state_filter.as_deref(), limit.unwrap_or(50))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn episode_analyze(
    app: AppHandle,
    state: State<'_, SlackState>,
    id: String,
) -> Result<Option<AnalysisJob>, String> {
    let job = analyze(&app, &state, &id, false).await?;
    let _ = app.emit("relay://change", ());
    Ok(job)
}

#[tauri::command]
pub fn jev_status(state: State<'_, SlackState>) -> JevStatus {
    let client = state.jev();
    JevStatus {
        configured: client.is_some(),
        model: client.map(|_| jev::DEFAULT_MODEL.to_string()),
        error: None,
    }
}

#[tauri::command]
pub async fn jev_set_key(state: State<'_, SlackState>, key: String) -> Result<JevStatus, String> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("the JEV API key is empty".into());
    }
    let client = JevClient::new(key.clone()).map_err(|error| error.to_string())?;
    // One cheap call proves the key before it is stored.
    let mut questions = std::collections::BTreeMap::new();
    questions.insert(
        jev::ids::WORTH_REVIEWING.to_string(),
        relay_core::jev::Question::noul("Is this a test?"),
    );
    client
        .evaluate(json!("hello"), questions)
        .await
        .map_err(|error| format!("JEV rejected the key: {error}"))?;
    write_secret(JEV_KEY, &key)?;
    *state.jev.lock().expect("jev state") = Some(Arc::new(client));
    Ok(JevStatus {
        configured: true,
        model: Some(jev::DEFAULT_MODEL.to_string()),
        error: None,
    })
}

#[tauri::command]
pub fn jev_clear_key(state: State<'_, SlackState>) -> JevStatus {
    delete_secret(JEV_KEY);
    *state.jev.lock().expect("jev state") = None;
    JevStatus::default()
}

/// Debug-only: inject a DM so the capture -> JEV -> reviewer loop can be tested without
/// Slack credentials. A release build refuses it.
#[tauri::command]
pub async fn slack_simulate_message(
    app: AppHandle,
    state: State<'_, SlackState>,
    peer: String,
    text: String,
) -> Result<String, String> {
    #[cfg(not(debug_assertions))]
    {
        let _ = (&app, &state, &peer, &text);
        return Err("slack_simulate_message is only available in a debug build".into());
    }
    #[cfg(debug_assertions)]
    {
        let _ = &app;
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let message = RawSlackMessage {
            channel_id: format!("D-{peer}"),
            ts: format!("{}.{:06}", millis / 1000, (millis % 1000) * 1000),
            thread_ts: None,
            user_id: Some(peer.clone()),
            text,
            permalink: None,
            edited_at: None,
            deleted: false,
            historical: false,
        };
        let stored = state
            .store
            .capture_slack_message(&peer, Some("Test peer"), &message, 90)
            .map_err(|error| error.to_string())?
            .ok_or("the test peer is excluded")?;
        Ok(stored.episode_id)
    }
}

/// Deletes captured DMs, episodes and jobs. Tasks created from them stay.
#[tauri::command]
pub fn slack_clear_capture(app: AppHandle, state: State<'_, SlackState>) -> Result<usize, String> {
    let deleted = state
        .store
        .clear_slack_capture()
        .map_err(|error| error.to_string())?;
    let _ = app.emit("relay://change", ());
    Ok(deleted)
}
