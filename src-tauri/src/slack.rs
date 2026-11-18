//! Slack capture runtime (PLAN 3.1, P2): credential storage, Socket Mode, catch-up on
//! reconnect, and the JEV job that follows an episode.
//!
//! The socket delivers `message.im` frames; Relay acks each envelope first and then
//! stores the message through `relay-core`. The store owns episode segmentation, peer
//! exclusion and retention; this module owns the connection and the JEV call.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
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
use relay_core::view::{SyncSource, SyncState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use crate::hover::HoverState;

mod claude;

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
    /// `claude` (claude.ai Slack connector) or `token` (Slack app token).
    pub transport: String,
    pub claude: Option<claude::ClaudeStatus>,
}

/// What one on-demand sync did, for the Settings button and the hotkey notification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackSyncReport {
    /// DMs listed, not only the ones with new messages.
    pub conversations: usize,
    /// Messages stored or updated in this run.
    pub messages: usize,
    /// Conversations with at least one new message.
    pub episodes: usize,
    /// Episodes JEV ran on without an error.
    pub analyzed: usize,
    /// The first failure; the full text also goes to the status error.
    pub error: Option<String>,
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
    /// DM channel -> the peer, so a message from the account owner is still attributed
    /// to the conversation and stored as outbound.
    channels: HashMap<String, String>,
}

pub struct SlackState {
    store: Arc<Store>,
    inner: Mutex<Inner>,
    jev: Mutex<Option<Arc<JevClient>>>,
    /// One manual sync at a time; the hotkey and the Settings button share it.
    syncing: AtomicBool,
}

/// Clears the sync flag on every exit path, errors included.
struct SyncGuard<'a>(&'a AtomicBool);

impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
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
                channels: HashMap::new(),
            }),
            jev: Mutex::new(jev.map(Arc::new)),
            syncing: AtomicBool::new(false),
        }
    }

    fn begin_sync(&self) -> Result<SyncGuard<'_>, String> {
        if self.syncing.swap(true, Ordering::SeqCst) {
            return Err("A Slack check is already running.".into());
        }
        Ok(SyncGuard(&self.syncing))
    }

    /// The Sources rows for Slack and JEV. Slack is optional: an agent can bring DMs
    /// another way, so "not configured" is neutral, never an error.
    pub fn sync_sources(&self) -> [SyncSource; 2] {
        let status = self.status();
        let (state, reason) = if claude::transport(&self.store) == claude::Transport::Claude {
            let claude = claude::status(&self.store, self.syncing.load(Ordering::SeqCst));
            if !claude.found {
                (SyncState::Down, Some("Claude Code not found".to_string()))
            } else if let Some(error) = claude.last_error.clone() {
                (SyncState::Down, Some(format!("Claude proxy: {error}")))
            } else if let Some(at) = claude.last_run_at.clone() {
                (
                    SyncState::Ok,
                    Some(format!(
                        "via Claude · last sync {} · ${:.3}",
                        at.get(11..16).unwrap_or(&at),
                        claude.last_cost_usd
                    )),
                )
            } else {
                (
                    SyncState::Retrying,
                    Some("via Claude · waiting for first sync".to_string()),
                )
            }
        } else if status.connected {
            (
                SyncState::Ok,
                status.user.map(|user| format!("connected as {user}")),
            )
        } else if let Some(error) = status.error {
            (SyncState::Down, Some(error))
        } else if status.configured {
            (SyncState::Retrying, Some("connecting".to_string()))
        } else {
            (SyncState::Off, Some("not connected".to_string()))
        };
        let slack = SyncSource {
            name: "Slack".into(),
            short: "Slack".into(),
            state,
            reason,
        };
        let jev = if self.jev().is_some() {
            (SyncState::Ok, Some("key stored".to_string()))
        } else {
            (SyncState::Off, Some("not connected".to_string()))
        };
        [
            slack,
            SyncSource {
                name: "JEV".into(),
                short: "JEV".into(),
                state: jev.0,
                reason: jev.1,
            },
        ]
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

    fn peer_for(&self, channel: &str) -> Option<String> {
        self.inner
            .lock()
            .expect("slack state")
            .channels
            .get(channel)
            .cloned()
    }

    fn remember_channel(&self, channel: &str, peer: &str) {
        self.inner
            .lock()
            .expect("slack state")
            .channels
            .insert(channel.to_string(), peer.to_string());
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

    // Retention and episode closing. Neither had a caller before the review.
    let retention = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let store = retention.state::<Arc<Store>>().inner().clone();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            let _ = store.prune_slack_messages(now - 30 * 86_400);
            let closed = store.close_stale_slack_episodes(90).unwrap_or_default();
            let state = retention.state::<SlackState>();
            if state.jev().is_some() {
                for id in closed {
                    // The plan's "rescreen at episode close": middle-band episodes get a
                    // second look, and a replay of the same watermark is a no-op.
                    let _ = analyze(&retention, &state, &id, false).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });

    claude::start_schedule(app.clone());

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<SlackState>();
        if claude::transport(&state.store) == claude::Transport::Claude {
            return;
        }
        let Some((user_token, app_token)) = load_tokens() else {
            return;
        };
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
    if paused(&app) {
        return;
    }
    // A reconnect must not overlap a manual check: both would call JEV for the same episode.
    let Ok(_guard) = state.begin_sync() else {
        return;
    };
    let report = run_sync(&app, &client, &stop).await;
    let status = state.update(|status| {
        status.team = Some(identity.team.clone());
        status.user = Some(identity.user.clone());
        status.last_event_at = Some(relay_core::model::now());
        status.open_episodes = state
            .store
            .list_slack_episodes(Some("open"), 200)
            .map(|episodes| episodes.len())
            .unwrap_or(0);
        // A channel error used to be cleared by this update right after it was published.
        if report.error.is_none() {
            status.error = None;
        }
    });
    publish(&app, &status);
    let _ = app.emit("relay://change", ());
}

/// Lists the 1:1 DMs, stores what is newer than each channel's checkpoint, and hands the
/// changed episodes to JEV. Shared by the socket's catch-up and the on-demand sync.
async fn run_sync(
    app: &AppHandle,
    client: &SlackClient,
    stop: &watch::Receiver<bool>,
) -> SlackSyncReport {
    let state = app.state::<SlackState>();
    let mut report = SlackSyncReport::default();
    let dms = match client.list_dms().await {
        Ok(dms) => dms,
        Err(error) => {
            let message = format!("conversations.list failed: {error}");
            let status = state.set_error(Some(message.clone()));
            publish(app, &status);
            report.error = Some(message);
            return report;
        }
    };
    report.conversations = dms.len();

    let mut changed: Vec<(String, bool)> = Vec::new();
    for dm in dms {
        if *stop.borrow() || paused(app) {
            break;
        }
        state.remember_channel(&dm.id, &dm.user_id);
        let cursor = state.store.slack_channel_cursor(&dm.id).unwrap_or(None);
        let historical = cursor.is_none();
        // A long offline gap can exceed one page; walk `oldest` forward until a short page.
        let mut messages = Vec::new();
        let mut oldest = cursor.unwrap_or_else(|| slack_ts_days_ago(BACKFILL_DAYS));
        for _ in 0..10 {
            match client.history(&dm.id, Some(&oldest), 200).await {
                Ok(batch) => {
                    if batch.is_empty() {
                        break;
                    }
                    let short = batch.len() < 200;
                    let newest = batch.last().map(|message| message.ts.clone());
                    messages.extend(batch);
                    match newest {
                        Some(ts) if ts != oldest && !short => oldest = ts,
                        _ => break,
                    }
                }
                Err(error) => {
                    let message = format!("conversations.history failed: {error}");
                    let status = state.set_error(Some(message.clone()));
                    publish(app, &status);
                    report.error.get_or_insert(message);
                    break;
                }
            }
        }
        if messages.is_empty() {
            continue;
        }
        let _name = resolve_name(&state, client, &dm.user_id).await;
        for mut message in messages {
            message.historical = historical;
            if let Some(episode_id) = capture(&state, client, &message).await {
                report.messages += 1;
                if !changed.iter().any(|(id, _)| id == &episode_id) {
                    changed.push((episode_id, historical));
                }
            }
        }
    }

    let episodes: Vec<_> = changed.into_iter().take(CATCHUP_ANALYSES).collect();
    report.episodes = episodes.len();
    for (episode_id, historical) in episodes {
        if *stop.borrow() {
            break;
        }
        match analyze(app, &state, &episode_id, historical).await {
            Ok(_) => report.analyzed += 1,
            Err(error) => {
                let status = state.set_error(Some(error.clone()));
                publish(app, &status);
                report.error.get_or_insert(error);
            }
        }
    }
    report
}

/// Runs the pull sync now. It only needs the user token, so a dead socket or a restarting
/// supervisor does not block it; when the socket is up it does the same work continuously.
async fn sync_manual(app: &AppHandle) -> Result<SlackSyncReport, String> {
    let store = app.state::<SlackState>().store.clone();
    if claude::transport(&store) == claude::Transport::Claude {
        return claude::sync(app).await;
    }
    if paused(app) {
        return Err("Capture is paused. Resume it, then check again.".into());
    }
    let user_token = read_secret(USER_TOKEN_KEY)
        .ok_or("Slack is not configured. Paste your user token in Settings → Slack.")?;
    let state = app.state::<SlackState>();
    // The guard lives until this function returns, so a second press cannot overlap.
    let _guard = state.begin_sync()?;
    let client = SlackClient::new(user_token, None).map_err(|error| error.to_string())?;
    let identity = client
        .auth_test()
        .await
        .map_err(|error| format!("Slack rejected the token: {error}"))?;
    // A manual run has no supervisor telling it to stop.
    let (_never, stop) = watch::channel(false);
    let report = run_sync(app, &client, &stop).await;
    let status = state.update(|status| {
        status.configured = true;
        status.team = Some(identity.team.clone());
        status.user = Some(identity.user.clone());
        status.last_event_at = Some(relay_core::model::now());
        status.open_episodes = state
            .store
            .list_slack_episodes(Some("open"), 200)
            .map(|episodes| episodes.len())
            .unwrap_or(0);
        if report.error.is_none() {
            status.error = None;
        }
    });
    publish(app, &status);
    let _ = app.emit("relay://change", ());
    Ok(report)
}

fn sync_summary(report: &SlackSyncReport) -> String {
    if report.messages == 0 {
        return "Slack: nothing new.".to_string();
    }
    let message_word = if report.messages == 1 {
        "message"
    } else {
        "messages"
    };
    let conversation_word = if report.episodes == 1 {
        "conversation"
    } else {
        "conversations"
    };
    let mut text = format!(
        "Slack: {} new {} in {} {}",
        report.messages, message_word, report.episodes, conversation_word
    );
    if report.analyzed > 0 {
        text.push_str(&format!(" · {} analyzed", report.analyzed));
    }
    if report.error.is_some() {
        text.push_str(" · some checks failed");
    }
    text
}

fn notify(app: &AppHandle, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("Relay")
        .body(body)
        .show();
}

/// The tray item and `Ctrl+Alt+S` both land here. Without a token it opens the setup page.
pub(crate) fn sync_in_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let store = app.state::<SlackState>().store.clone();
        if claude::transport(&store) == claude::Transport::Token
            && read_secret(USER_TOKEN_KEY).is_none()
        {
            crate::open_route(&app, "/settings");
            return;
        }
        match sync_manual(&app).await {
            Ok(report) => notify(&app, &sync_summary(&report)),
            Err(error) => notify(&app, &format!("Slack check failed: {error}")),
        }
    });
}

async fn capture(
    state: &SlackState,
    client: &SlackClient,
    message: &RawSlackMessage,
) -> Option<String> {
    let channel = message.channel_id.as_str();
    let peer_id = match state.peer_for(channel) {
        Some(peer) => peer,
        None => match client.dm_peer(channel).await {
            Ok(Some(peer)) => {
                state.remember_channel(channel, &peer);
                peer
            }
            Ok(None) => return None,
            Err(error) => {
                eprintln!("relay: conversations.info {channel}: {error}");
                return None;
            }
        },
    };
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

    let (job, inserted) = state
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
    if !inserted {
        // A replay of the same watermark must not reset a finished job or rerun the reviewer.
        return Ok(Some(job));
    }

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
    status.transport = claude::transport(&state.store).as_str().to_string();
    status.claude = Some(claude::status(
        &state.store,
        state.syncing.load(Ordering::SeqCst),
    ));
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

/// On-demand check: list the DMs, pull what is new, and run JEV. The same code powers
/// the socket's catch-up, the tray item and `Ctrl+Alt+S`.
#[tauri::command]
pub async fn slack_sync_now(app: AppHandle) -> Result<SlackSyncReport, String> {
    sync_manual(&app).await
}

/// Picks the Slack transport: `claude` (claude.ai connector) or `token` (app token).
#[tauri::command]
pub fn slack_set_transport(
    app: AppHandle,
    state: State<'_, SlackState>,
    transport: String,
) -> Result<SlackStatus, String> {
    claude::set_transport(&state.store, &transport)?;
    let status = slack_status(app.clone(), state);
    publish(&app, &status);
    Ok(status)
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
    // Reviewer logs contain DM text; clearing capture must clear them too.
    if let Ok(dir) = app.path().app_data_dir()
        && let Ok(entries) = std::fs::read_dir(dir.join("analyst"))
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("run-") && name.ends_with(".log") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = app.emit("relay://change", ());
    Ok(deleted)
}

/// Requeues a failed or stuck job and starts the reviewer again.
#[tauri::command]
pub fn job_retry(
    app: AppHandle,
    state: State<'_, SlackState>,
    id: String,
) -> Result<AnalysisJob, String> {
    let job = state
        .store
        .get_job(&id)
        .map_err(|error| error.to_string())?;
    if !matches!(job.state.as_str(), "failed" | "pending_agent") {
        return Err(format!(
            "job {id} is {}; only failed or queued agent jobs retry (a review job needs a human)",
            job.state
        ));
    }
    if state
        .store
        .job_has_ops(&id)
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "job {id} already wrote; resume or inspect it instead of rerunning"
        ));
    }
    let job = state
        .store
        .set_job_status(&id, "pending_agent", None, None)
        .map_err(|error| error.to_string())?;
    crate::analyst::spawn_job(app, id);
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_summary_uses_singulars_and_counts_the_work() {
        assert_eq!(
            sync_summary(&SlackSyncReport::default()),
            "Slack: nothing new."
        );
        let one = SlackSyncReport {
            messages: 1,
            episodes: 1,
            analyzed: 1,
            ..Default::default()
        };
        assert_eq!(
            sync_summary(&one),
            "Slack: 1 new message in 1 conversation · 1 analyzed"
        );
        let many = SlackSyncReport {
            messages: 3,
            episodes: 2,
            analyzed: 0,
            ..Default::default()
        };
        assert_eq!(
            sync_summary(&many),
            "Slack: 3 new messages in 2 conversations"
        );
    }
}
