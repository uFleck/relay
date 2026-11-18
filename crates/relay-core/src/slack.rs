//! Slack Web API client for DM capture (PLAN 3.1). HTTP only: the Socket Mode loop
//! lives in the app, because it owns the runtime and the reconnect policy.
//!
//! Tokens are only sent in the `Authorization` header. They are never logged, never
//! part of an error, and redacted by [`SlackClient`]'s `Debug`.

use std::fmt;

use serde_json::{Value, json};

use crate::model::{RawSlackMessage, slack_ts_to_rfc3339};

pub const DEFAULT_BASE_URL: &str = "https://slack.com/api";
/// Requests give up after this many seconds unless [`SlackClient::with_base`] says otherwise.
const TIMEOUT_SECS: u64 = 15;
/// `conversations.list` is paginated; this cap stops a broken cursor from looping forever.
const MAX_PAGES: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("slack request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// `{"ok": false, "error": "invalid_auth"}` with HTTP 200.
    #[error("slack refused the call: {0}")]
    Api(String),
    #[error("slack response is missing {0}")]
    Missing(&'static str),
    #[error("missing slack token: {0}")]
    MissingToken(&'static str),
    #[error("slack json: {0}")]
    Json(#[from] serde_json::Error),
}

/// What `auth.test` reports; the setup sheet shows the workspace and the user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackIdentity {
    pub team: String,
    pub team_id: String,
    pub user: String,
    pub user_id: String,
    pub url: Option<String>,
}

/// One 1:1 DM channel. `user_id` is the peer, not the account owner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmChannel {
    pub id: String,
    pub user_id: String,
}

/// A message as the Socket Mode payload reports it, before the store assigns an episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundEvent {
    /// A DM message or an edited one. `deleted` marks a deletion.
    Message(RawSlackMessage),
    /// A frame Relay does not capture, with the reason for the log.
    Ignored(&'static str),
}

pub struct SlackClient {
    user_token: String,
    app_token: Option<String>,
    http: reqwest::Client,
    base: String,
}

impl Clone for SlackClient {
    fn clone(&self) -> Self {
        Self {
            user_token: self.user_token.clone(),
            app_token: self.app_token.clone(),
            http: self.http.clone(),
            base: self.base.clone(),
        }
    }
}

impl fmt::Debug for SlackClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackClient")
            .field("user_token", &"<redacted>")
            .field("app_token", &self.app_token.as_ref().map(|_| "<redacted>"))
            .field("base", &self.base)
            .finish()
    }
}

impl SlackClient {
    pub fn new(user_token: String, app_token: Option<String>) -> Result<Self, SlackError> {
        Self::with_base(user_token, app_token, DEFAULT_BASE_URL.to_string())
    }

    /// An explicit base URL, for tests and for a mock server.
    pub fn with_base(
        user_token: String,
        app_token: Option<String>,
        base: String,
    ) -> Result<Self, SlackError> {
        let user_token = user_token.trim();
        if user_token.is_empty() {
            return Err(SlackError::MissingToken("user token (xoxp-)"));
        }
        if let Some(token) = &app_token
            && token.trim().is_empty()
        {
            return Err(SlackError::MissingToken("app token (xapp-)"));
        }
        // The same ring provider the rest of the workspace uses; installing it twice is safe.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()?;
        Ok(Self {
            user_token: user_token.to_string(),
            app_token: app_token.map(|token| token.trim().to_string()),
            http,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    /// `auth.test` with the user token: proves the token and names the workspace.
    pub async fn auth_test(&self) -> Result<SlackIdentity, SlackError> {
        let value = self.post("auth.test", &self.user_token, json!({})).await?;
        Ok(SlackIdentity {
            team: string(&value, "team")?,
            team_id: string(&value, "team_id")?,
            user: string(&value, "user")?,
            user_id: string(&value, "user_id")?,
            url: value.get("url").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// `apps.connections.open` with the app token: one fresh Socket Mode URL.
    pub async fn open_socket(&self) -> Result<String, SlackError> {
        let app_token = self
            .app_token
            .as_deref()
            .ok_or(SlackError::MissingToken("app token (xapp-)"))?;
        let value = self
            .post("apps.connections.open", app_token, json!({}))
            .await?;
        string(&value, "url")
    }

    /// Every 1:1 DM channel the user is in, following `next_cursor`.
    pub async fn list_dms(&self) -> Result<Vec<DmChannel>, SlackError> {
        let mut dms = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut body = json!({"types": "im", "limit": 200, "exclude_archived": true});
            if let Some(cursor) = &cursor {
                body["cursor"] = json!(cursor);
            }
            let value = self
                .post("conversations.list", &self.user_token, body)
                .await?;
            let channels = value
                .get("channels")
                .and_then(Value::as_array)
                .ok_or(SlackError::Missing("channels"))?;
            for channel in channels {
                let Some(id) = channel.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(user_id) = channel.get("user").and_then(Value::as_str) else {
                    continue;
                };
                if channel.get("is_im").and_then(Value::as_bool) == Some(false) {
                    continue;
                }
                dms.push(DmChannel {
                    id: id.to_string(),
                    user_id: user_id.to_string(),
                });
            }
            cursor = value
                .pointer("/response_metadata/next_cursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(dms)
    }

    /// Messages newer than `oldest` (a Slack ts), oldest first. Slack returns newest first.
    pub async fn history(
        &self,
        channel: &str,
        oldest: Option<&str>,
        limit: u32,
    ) -> Result<Vec<RawSlackMessage>, SlackError> {
        let mut body = json!({
            "channel": channel,
            "limit": limit.clamp(1, 200),
            "inclusive": false,
        });
        if let Some(oldest) = oldest {
            body["oldest"] = json!(oldest);
        }
        let value = self
            .post("conversations.history", &self.user_token, body)
            .await?;
        let messages = value
            .get("messages")
            .and_then(Value::as_array)
            .ok_or(SlackError::Missing("messages"))?;
        let mut out: Vec<RawSlackMessage> = messages
            .iter()
            .filter_map(|message| message_from_value(channel, message))
            .collect();
        out.sort_by(|a, b| a.ts.cmp(&b.ts));
        Ok(out)
    }

    /// The peer of a 1:1 DM channel. Slack reports it as `channel.user`.
    pub async fn dm_peer(&self, channel: &str) -> Result<Option<String>, SlackError> {
        let value = self
            .post(
                "conversations.info",
                &self.user_token,
                json!({"channel": channel}),
            )
            .await?;
        Ok(value
            .pointer("/channel/user")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// The display name for a user id, when the workspace exposes it.
    pub async fn user_name(&self, user_id: &str) -> Result<Option<String>, SlackError> {
        let value = self
            .post("users.info", &self.user_token, json!({"user": user_id}))
            .await?;
        let user = value.get("user").ok_or(SlackError::Missing("user"))?;
        let profile = user.get("profile");
        Ok(profile
            .and_then(|profile| profile.get("display_name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .or_else(|| user.get("real_name").and_then(Value::as_str))
            .or_else(|| user.get("name").and_then(Value::as_str))
            .map(str::to_string))
    }

    async fn post(&self, method: &str, token: &str, body: Value) -> Result<Value, SlackError> {
        let value: Value = self
            .http
            .post(format!("{}/{}", self.base, method))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            let error = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error");
            return Err(SlackError::Api(error.to_string()));
        }
        Ok(value)
    }
}

/// The one frame parser for Socket Mode. Unknown frames are ignored, never fatal.
pub fn parse_socket_frame(text: &str) -> Result<SocketFrame, SlackError> {
    let value: Value = serde_json::from_str(text)?;
    match value.get("type").and_then(Value::as_str) {
        Some("hello") => Ok(SocketFrame::Hello),
        Some("disconnect") => Ok(SocketFrame::Disconnect {
            reason: value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        }),
        Some("events_api") => {
            let envelope_id = value
                .get("envelope_id")
                .and_then(Value::as_str)
                .ok_or(SlackError::Missing("envelope_id"))?
                .to_string();
            let payload = value
                .get("payload")
                .cloned()
                .ok_or(SlackError::Missing("payload"))?;
            Ok(SocketFrame::Event {
                envelope_id,
                payload,
            })
        }
        _ => Ok(SocketFrame::Ignored),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SocketFrame {
    Hello,
    Event { envelope_id: String, payload: Value },
    Disconnect { reason: String },
    Ignored,
}

/// Turns an `events_api` payload into a capture event. Only 1:1 DMs with text are
/// kept; bots, edits by others and channel traffic are ignored.
pub fn event_from_payload(payload: &Value) -> InboundEvent {
    let event = match payload.get("event") {
        Some(event) => event,
        None => return InboundEvent::Ignored("payload has no event"),
    };
    let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
    if kind != "message" {
        return InboundEvent::Ignored("not a message event");
    }
    let channel_type = event.get("channel_type").and_then(Value::as_str);
    if !matches!(channel_type, Some("im") | Some("mpim")) {
        return InboundEvent::Ignored("not a DM");
    }
    let channel_id = match event.get("channel").and_then(Value::as_str) {
        Some(channel) => channel,
        None => return InboundEvent::Ignored("message has no channel"),
    };
    let subtype = event.get("subtype").and_then(Value::as_str);
    match subtype {
        Some("message_changed") => {
            let Some(message) = event.get("message") else {
                return InboundEvent::Ignored("changed message has no body");
            };
            match message_from_value(channel_id, message) {
                Some(message) => InboundEvent::Message(message),
                None => InboundEvent::Ignored("changed message cannot be read"),
            }
        }
        Some("message_deleted") => {
            let Some(ts) = event.get("deleted_ts").and_then(Value::as_str) else {
                return InboundEvent::Ignored("deleted message has no ts");
            };
            InboundEvent::Message(RawSlackMessage {
                channel_id: channel_id.to_string(),
                ts: ts.to_string(),
                thread_ts: None,
                user_id: None,
                text: String::new(),
                permalink: None,
                edited_at: None,
                deleted: true,
                historical: false,
            })
        }
        Some(_) => InboundEvent::Ignored("message subtype is not captured"),
        None => {
            if event.get("bot_id").is_some() {
                return InboundEvent::Ignored("message is from a bot");
            }
            match message_from_value(channel_id, event) {
                Some(message) => InboundEvent::Message(message),
                None => InboundEvent::Ignored("message cannot be read"),
            }
        }
    }
}

fn message_from_value(channel_id: &str, value: &Value) -> Option<RawSlackMessage> {
    let ts = value.get("ts").and_then(Value::as_str)?;
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if text.is_empty() {
        return None;
    }
    let edited_at = value
        .pointer("/edited/ts")
        .and_then(Value::as_str)
        .and_then(slack_ts_to_rfc3339);
    Some(RawSlackMessage {
        channel_id: channel_id.to_string(),
        ts: ts.to_string(),
        thread_ts: value
            .get("thread_ts")
            .and_then(Value::as_str)
            .map(str::to_string),
        user_id: value
            .get("user")
            .and_then(Value::as_str)
            .map(str::to_string),
        text,
        permalink: value
            .get("permalink")
            .and_then(Value::as_str)
            .map(str::to_string),
        edited_at,
        deleted: false,
        historical: false,
    })
}

fn string(value: &Value, key: &'static str) -> Result<String, SlackError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(SlackError::Missing(key))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{HeaderMap, header},
        response::Response,
        routing::post,
    };
    use serde_json::json;

    use super::*;

    type MockCalls = Arc<Mutex<Vec<(String, Option<String>, Value)>>>;
    type MockState = (Vec<(&'static str, Value)>, MockCalls);

    struct Mock {
        base_url: String,
        calls: MockCalls,
    }

    async fn handle(
        State((answers, calls)): State<MockState>,
        headers: HeaderMap,
        request: axum::extract::Request,
    ) -> Response {
        let method = request.uri().path().trim_start_matches('/').to_string();
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim_start_matches("Bearer ").to_string());
        let body = axum::body::to_bytes(request.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let occurrence = {
            let mut calls = calls.lock().unwrap();
            let occurrence = calls.iter().filter(|(name, _, _)| name == &method).count();
            calls.push((method.clone(), authorization, body));
            occurrence
        };
        let answer = answers
            .iter()
            .filter(|(name, _)| *name == method)
            .nth(occurrence)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| json!({"ok": false, "error": "unknown_method"}));
        Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(answer.to_string()))
            .unwrap()
    }

    impl Mock {
        async fn start(answers: Vec<(&'static str, Value)>) -> Self {
            let calls: MockCalls = Arc::new(Mutex::new(Vec::new()));
            let state: MockState = (answers, calls.clone());
            let router = Router::new()
                .route("/{method}", post(handle))
                .with_state(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            Self {
                base_url: format!("http://{addr}"),
                calls,
            }
        }

        fn client(&self) -> SlackClient {
            SlackClient::with_base(
                "xoxp-test".into(),
                Some("xapp-test".into()),
                self.base_url.clone(),
            )
            .unwrap()
        }
    }

    #[tokio::test]
    async fn auth_test_and_open_socket_use_the_right_tokens() {
        let mock = Mock::start(vec![
            (
                "auth.test",
                json!({"ok": true, "team": "Acme", "team_id": "T1", "user": "bob", "user_id": "U0", "url": "https://acme.slack.com"}),
            ),
            ("apps.connections.open", json!({"ok": true, "url": "wss://wss.test/link/1"})),
        ])
        .await;
        let client = mock.client();
        let identity = client.auth_test().await.unwrap();
        assert_eq!(identity.team, "Acme");
        assert_eq!(identity.user_id, "U0");
        let socket = client.open_socket().await.unwrap();
        assert_eq!(socket, "wss://wss.test/link/1");
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls[0].1.as_deref(), Some("xoxp-test"));
        assert_eq!(calls[1].1.as_deref(), Some("xapp-test"));
    }

    #[tokio::test]
    async fn list_dms_follows_the_cursor() {
        let mock = Mock::start(vec![
            (
                "conversations.list",
                json!({"ok": true, "channels": [{"id": "D1", "user": "U1", "is_im": true}], "response_metadata": {"next_cursor": "page2"}}),
            ),
            (
                "conversations.list",
                json!({"ok": true, "channels": [{"id": "D2", "user": "U2", "is_im": true}], "response_metadata": {"next_cursor": ""}}),
            ),
        ])
        .await;
        // The mock answers repeated methods in order, so the cursor is exercised.
        let client = mock.client();
        let dms = client.list_dms().await.unwrap();
        assert_eq!(dms.len(), 2);
        assert_eq!(dms[1].id, "D2");
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].2["cursor"], "page2");
    }

    #[tokio::test]
    async fn history_maps_edits_and_sorts_oldest_first() {
        let mock = Mock::start(vec![(
            "conversations.history",
            json!({"ok": true, "messages": [
                {"ts": "1700000600.000100", "user": "U1", "text": "newer"},
                {"ts": "1700000000.000100", "user": "U1", "text": "older", "edited": {"ts": "1700000100.000100"}}
            ]}),
        )])
        .await;
        let messages = mock
            .client()
            .history("D1", Some("1699999999.000000"), 100)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "older");
        assert!(messages[0].edited_at.is_some());
        assert_eq!(messages[0].channel_id, "D1");
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls[0].2["oldest"], "1699999999.000000");
        assert_eq!(calls[0].2["channel"], "D1");
    }

    #[tokio::test]
    async fn an_error_body_is_a_typed_api_error() {
        let mock = Mock::start(vec![(
            "auth.test",
            json!({"ok": false, "error": "invalid_auth"}),
        )])
        .await;
        let error = mock.client().auth_test().await.unwrap_err();
        assert!(matches!(error, SlackError::Api(ref message) if message == "invalid_auth"));
        // The token never appears in the error.
        assert!(!error.to_string().contains("xoxp"));
    }

    #[test]
    fn socket_frames_parse_and_ignore_unknown_types() {
        assert_eq!(
            parse_socket_frame(r#"{"type":"hello"}"#).unwrap(),
            SocketFrame::Hello
        );
        let event = parse_socket_frame(
            r#"{"type":"events_api","envelope_id":"e1","payload":{"event":{"type":"message"}}}"#,
        )
        .unwrap();
        assert!(matches!(event, SocketFrame::Event { ref envelope_id, .. } if envelope_id == "e1"));
        assert_eq!(
            parse_socket_frame(r#"{"type":"disconnect","reason":"link_timeout"}"#).unwrap(),
            SocketFrame::Disconnect {
                reason: "link_timeout".into()
            }
        );
        assert_eq!(
            parse_socket_frame(r#"{"type":"other"}"#).unwrap(),
            SocketFrame::Ignored
        );
    }

    #[test]
    fn only_dm_messages_with_text_are_captured() {
        let dm = json!({"event": {"type": "message", "channel_type": "im", "channel": "D1", "user": "U1", "text": "hi", "ts": "1700000000.000100"}});
        match event_from_payload(&dm) {
            InboundEvent::Message(message) => {
                assert_eq!(message.channel_id, "D1");
                assert_eq!(message.user_id.as_deref(), Some("U1"));
                assert_eq!(message.text, "hi");
            }
            other => panic!("expected a message, got {other:?}"),
        }

        for ignored in [
            json!({"event": {"type": "message", "channel_type": "channel", "channel": "C1", "user": "U1", "text": "public", "ts": "1"}}),
            json!({"event": {"type": "message", "channel_type": "im", "channel": "D1", "user": "U1", "text": "", "ts": "1"}}),
            json!({"event": {"type": "message", "channel_type": "im", "channel": "D1", "bot_id": "B1", "text": "bot", "ts": "1"}}),
            json!({"event": {"type": "reaction_added"}}),
        ] {
            assert!(matches!(
                event_from_payload(&ignored),
                InboundEvent::Ignored(_)
            ));
        }

        let edited = json!({"event": {"type": "message", "subtype": "message_changed", "channel_type": "im", "channel": "D1", "message": {"user": "U1", "text": "fixed", "ts": "1700000000.000100", "edited": {"ts": "1700000100.000100"}}}});
        match event_from_payload(&edited) {
            InboundEvent::Message(message) => {
                assert_eq!(message.text, "fixed");
                assert!(message.edited_at.is_some());
            }
            other => panic!("expected an edit, got {other:?}"),
        }

        let deleted = json!({"event": {"type": "message", "subtype": "message_deleted", "channel_type": "im", "channel": "D1", "deleted_ts": "1700000000.000100"}});
        match event_from_payload(&deleted) {
            InboundEvent::Message(message) => assert!(message.deleted),
            other => panic!("expected a deletion, got {other:?}"),
        }
    }
}
