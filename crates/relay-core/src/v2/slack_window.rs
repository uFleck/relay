//! #33: immutable conversation windows. A pick freezes the UTC selection time, reads up to the
//! latest 20 messages from the preceding day with the original bounds, and keeps that first
//! successful result for every later read. The Tauri adapter only supplies a runner and a clock.

use std::time::Duration;

use crate::claude_slack::{
    ClaudeError, RunRequest, Runner, Tool, ToolCall, build_prompt, parse_channel, parse_stream,
};
use crate::model::{format_time, parse_time};
use crate::provider::ProviderFailure;

use super::reference::{
    ReferenceAvailability, ReferenceKey, ReferenceSnapshot, V2PickerGroup, V2PickerRow,
};
use super::slack_directory::SlackWindow;
use super::slack_thread::CachedMessage;
use super::store::V2Store;

/// A window read gets the same 30-second deadline as a thread read, independent of the 60-second
/// directory scan. A skipped call or connector `pending` gets exactly one retry with the original
/// bounds (never a resumed cursor).
pub const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: usize = 2;
const MAX_PAGES: usize = 10;
pub const WINDOW_LIMIT: usize = 20;
pub const WINDOW_HOURS: i64 = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowFailure {
    Pending,
    MissingCli,
    Configuration,
    NotFound,
    Malformed(String),
}

impl WindowFailure {
    pub fn reason_text(&self) -> String {
        match self {
            Self::Pending => "Slack is still connecting. Try again shortly".to_string(),
            Self::MissingCli => {
                "Claude Code not found. Install it, then sign in once with `claude`.".to_string()
            }
            Self::Configuration => {
                "Claude Slack connector is not connected or signed out".to_string()
            }
            Self::NotFound => "Not found or not visible to you".to_string(),
            Self::Malformed(detail) => {
                let short: String = detail.chars().take(120).collect();
                if short.is_empty() {
                    "Slack returned an unreadable window".to_string()
                } else {
                    short
                }
            }
        }
    }

    pub fn settings_destination(&self) -> Option<String> {
        matches!(self, Self::MissingCli | Self::Configuration)
            .then(|| "Settings → Slack".to_string())
    }

    fn immediate_retry(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Maps onto #30's shared recovery reasons: setup problems pause, transient unreadability
    /// retries on the fixed schedule.
    pub fn to_provider_failure(&self) -> ProviderFailure {
        match self {
            Self::NotFound => ProviderFailure::NotFound,
            Self::Pending | Self::Malformed(_) => ProviderFailure::Unavailable,
            Self::MissingCli | Self::Configuration => ProviderFailure::Configuration,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowResolution {
    pub row: V2PickerRow,
    /// The frozen content and bounds, exposed to #34 through this shared resolved-Reference shape.
    pub window: Option<SlackWindow>,
    pub from_cache: bool,
    pub coalesced: bool,
    pub failure: Option<WindowFailure>,
}

/// The immutable bounds of a pick: `latest` is the UTC selection time, `oldest` exactly 24 hours
/// earlier. Produced once at selection and reused verbatim for recovery and every later read.
pub fn bounds(selected_at: &str) -> Option<(String, String)> {
    let at = parse_time(selected_at)?;
    Some((
        format_time(at - time::Duration::hours(WINDOW_HOURS)),
        format_time(at),
    ))
}

fn window_request(channel: &str, oldest: &str, cursor: Option<&str>) -> RunRequest {
    let call = ToolCall::read_window(channel, oldest, cursor);
    RunRequest {
        calls: vec![call.clone()],
        prompt: build_prompt(std::slice::from_ref(&call)),
        max_turns: 2,
        disallow: Vec::new(),
        timeout: Some(WINDOW_TIMEOUT),
        tools: Some(vec![Tool::ReadChannel.full_name()]),
    }
}

/// Slack timestamps compare as `(seconds, zero-padded microseconds)` without floats.
fn ts_key(ts: &str) -> (u64, String) {
    let (seconds, micros) = ts.trim().split_once('.').unwrap_or((ts.trim(), ""));
    (seconds.parse::<u64>().unwrap_or(0), format!("{micros:0<6}"))
}

/// Inclusive upper bound, exclusive lower bound: `(selection − 24h, selection]`. Slack message
/// timestamps are epoch strings while the frozen bounds are RFC 3339, so both are parsed into
/// instants before comparison (no float conversion, no loss of microsecond precision).
fn in_window(ts: &str, oldest: &str, latest: &str) -> bool {
    let (Some(at), Some(lower), Some(upper)) =
        (ts_instant(ts), parse_time(oldest), parse_time(latest))
    else {
        return false;
    };
    at > lower && at <= upper
}

fn ts_instant(ts: &str) -> Option<time::OffsetDateTime> {
    let (seconds, micros) = ts.trim().split_once('.').unwrap_or((ts.trim(), ""));
    let seconds = seconds.parse::<i64>().ok()?;
    let micros = if micros.is_empty() {
        0
    } else {
        micros.parse::<i64>().ok()? * 10i64.pow(9 - micros.len().min(9) as u32)
    };
    Some(
        time::OffsetDateTime::from_unix_timestamp(seconds).ok()?
            + time::Duration::nanoseconds(micros),
    )
}

fn classify(error: &ClaudeError) -> WindowFailure {
    match error {
        ClaudeError::Timeout => WindowFailure::Pending,
        ClaudeError::ConnectorDown(status) if status == "pending" => WindowFailure::Pending,
        ClaudeError::NotFound => WindowFailure::MissingCli,
        ClaudeError::ConnectorDown(_) => WindowFailure::Configuration,
        ClaudeError::Tool(text) if text.contains("channel_not_found") => WindowFailure::NotFound,
        ClaudeError::Tool(text) if text.contains("invalid_auth") || text.contains("not_authed") => {
            WindowFailure::Configuration
        }
        ClaudeError::Rejected(_)
        | ClaudeError::Cli(_)
        | ClaudeError::Tool(_)
        | ClaudeError::Parse(_) => WindowFailure::Malformed(error.to_string()),
        ClaudeError::Store(_) => WindowFailure::Malformed("the cache is unavailable".into()),
    }
}

/// Reads one window from scratch using its frozen bounds. A retry starts over from `oldest` with
/// no cursor, never halfway through a failed scan. The first successful result is committed with
/// `ready = true`, including a legitimate empty window.
pub fn freeze_window(
    store: &V2Store,
    runner: &dyn Runner,
    window: &SlackWindow,
    now: &str,
) -> WindowResolution {
    let Some(_guard) = store.try_begin_thread("window", &window.id) else {
        return loading(&window.id, window, now, true);
    };
    let mut failure = WindowFailure::Pending;
    for _ in 0..MAX_ATTEMPTS {
        let mut collected: Vec<CachedMessage> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        let mut settled = false;
        for _ in 0..MAX_PAGES {
            let request = window_request(&window.channel_id, &window.oldest, cursor.as_deref());
            let stdout = match runner.run(&request) {
                Ok(stdout) => stdout,
                Err(error) => {
                    failure = classify(&error);
                    break;
                }
            };
            let data = match parse_stream(&stdout, &request.calls) {
                Ok(data) => data,
                Err(error) => {
                    failure = classify(&error);
                    break;
                }
            };
            if data.connector_status.as_deref() != Some("connected") {
                failure = if data.connector_status.as_deref() == Some("pending") {
                    WindowFailure::Pending
                } else {
                    WindowFailure::Configuration
                };
                break;
            }
            if data.tool_use_count == 0 {
                failure = WindowFailure::Pending;
                break;
            }
            if data.tool_use_count != 1 || data.duplicate_tool_use {
                failure = WindowFailure::Malformed(
                    "the run made more than one or a duplicate slack_read_channel call".into(),
                );
                break;
            }
            let Some(output) = data.results.first().and_then(|slot| slot.as_ref()) else {
                failure = WindowFailure::Pending;
                break;
            };
            if output.failed() {
                failure = classify(&ClaudeError::Tool(output.text.chars().take(160).collect()));
                break;
            }
            let page = match parse_channel(&output.text) {
                Ok(page) => page,
                Err(error) => {
                    failure = classify(&error);
                    break;
                }
            };
            for message in page.messages {
                if !in_window(&message.ts, &window.oldest, &window.latest) {
                    continue;
                }
                if !seen.insert(message.ts.clone()) {
                    continue;
                }
                collected.push(CachedMessage {
                    ts: message.ts,
                    author: if message.name.is_empty() {
                        message.user_id
                    } else {
                        message.name
                    },
                    text: message.text,
                });
            }
            if collected.len() >= WINDOW_LIMIT {
                settled = true;
                break;
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => {
                    settled = true;
                    break;
                }
            }
        }
        if !settled {
            if !failure.immediate_retry() {
                break;
            }
            continue;
        }
        collected.sort_by(|a, b| ts_key(&a.ts).cmp(&ts_key(&b.ts)));
        if collected.len() > WINDOW_LIMIT {
            let drop = collected.len() - WINDOW_LIMIT;
            collected.drain(..drop);
        }
        let frozen = SlackWindow {
            id: window.id.clone(),
            channel_id: window.channel_id.clone(),
            selected_at: window.selected_at.clone(),
            oldest: window.oldest.clone(),
            latest: window.latest.clone(),
            messages: collected,
            link: window.link.clone(),
            ready: true,
        };
        if store.save_window(&frozen).is_err() {
            failure = WindowFailure::Malformed("could not persist the frozen window".into());
            break;
        }
        return hit(&frozen, now, false);
    }
    unavailable(window, &failure, now, false)
}

/// Resolves a window Reference. A ready row answers without any CLI call; a pending row takes the
/// per-window guard and reads with its stored bounds. Once ready, the content is never refreshed.
pub fn resolve_window(
    store: &V2Store,
    runner: &dyn Runner,
    key: &ReferenceKey,
    now: &str,
) -> WindowResolution {
    let ReferenceKey::SlackWindow { id, .. } = key else {
        return unavailable_for(
            key,
            &WindowFailure::Malformed("not a window reference".into()),
            now,
            false,
        );
    };
    match store.slack_window(id) {
        Ok(Some(window)) if window.ready => return hit(&window, now, true),
        Ok(Some(window)) => return freeze_window(store, runner, &window, now),
        Ok(None) => {
            return unavailable_for(
                key,
                &WindowFailure::Malformed("the window was not created".into()),
                now,
                false,
            );
        }
        Err(error) => {
            return unavailable_for(
                key,
                &WindowFailure::Malformed(error.to_string()),
                now,
                false,
            );
        }
    }
}

/// An adapter with no `claude` binary cannot start a run; a ready window still resolves from SQLite.
pub fn failed_window(key: &ReferenceKey, failure: &WindowFailure, now: &str) -> WindowResolution {
    unavailable_for(key, failure, now, false)
}

fn hit(window: &SlackWindow, now: &str, from_cache: bool) -> WindowResolution {
    let count = window.messages.len();
    let title = if count == 0 {
        "Empty window".to_string()
    } else {
        window.messages.last().map_or_else(
            || "Slack window".to_string(),
            |message| first_line(&message.text, 80),
        )
    };
    let content = window
        .messages
        .iter()
        .map(|message| format!("{}: {}", message.author, message.text))
        .collect::<Vec<_>>()
        .join("\n");
    let snapshot = ReferenceSnapshot {
        title: Some(title.clone()),
        state: Some(format!("{count} messages")),
        metadata: vec![
            format!("window {} → {}", window.oldest, window.latest),
            format!("channel {}", window.channel_id),
        ],
        cached_at: Some(window.selected_at.clone()),
        reason: None,
        content: Some(content),
        channel_name: None,
    };
    WindowResolution {
        row: row_for(
            window,
            snapshot,
            ReferenceAvailability::Ready,
            title,
            None,
            None,
            now,
        ),
        window: Some(window.clone()),
        from_cache,
        coalesced: false,
        failure: None,
    }
}

fn loading(id: &str, window: &SlackWindow, now: &str, coalesced: bool) -> WindowResolution {
    let snapshot = ReferenceSnapshot {
        title: Some("fetching…".to_string()),
        ..ReferenceSnapshot::default()
    };
    WindowResolution {
        row: V2PickerRow {
            key: format!("slackwindow:{id}"),
            kind: "slackwindow".to_string(),
            display_id: format!("@{}", window.channel_id),
            canonical_link: window.link.clone(),
            availability: ReferenceAvailability::Ready,
            group: V2PickerGroup::Conversations,
            title: "fetching…".to_string(),
            state: None,
            metadata: Vec::new(),
            snapshot: Some(snapshot),
            unavailable_reason: None,
            settings_destination: None,
            updated_at: now.to_string(),
            exact_match: false,
        },
        window: None,
        from_cache: false,
        coalesced,
        failure: None,
    }
}

fn unavailable(
    window: &SlackWindow,
    failure: &WindowFailure,
    now: &str,
    coalesced: bool,
) -> WindowResolution {
    let key = ReferenceKey::SlackWindow {
        id: window.id.clone(),
        channel_id: window.channel_id.clone(),
        host: "slack.com".to_string(),
        name: None,
    };
    unavailable_for(&key, failure, now, coalesced)
}

fn unavailable_for(
    key: &ReferenceKey,
    failure: &WindowFailure,
    now: &str,
    coalesced: bool,
) -> WindowResolution {
    let reason = failure.reason_text();
    let row = V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(None),
        canonical_link: key.canonical_url(),
        availability: ReferenceAvailability::Unavailable,
        group: V2PickerGroup::Conversations,
        title: String::new(),
        state: None,
        metadata: Vec::new(),
        snapshot: Some(ReferenceSnapshot {
            reason: Some(reason.clone()),
            ..ReferenceSnapshot::default()
        }),
        unavailable_reason: Some(reason),
        settings_destination: failure.settings_destination(),
        updated_at: now.to_string(),
        exact_match: false,
    };
    WindowResolution {
        row,
        window: None,
        from_cache: false,
        coalesced,
        failure: Some(failure.clone()),
    }
}

#[allow(clippy::too_many_arguments)]
fn row_for(
    window: &SlackWindow,
    snapshot: ReferenceSnapshot,
    availability: ReferenceAvailability,
    title: String,
    unavailable_reason: Option<String>,
    settings_destination: Option<String>,
    now: &str,
) -> V2PickerRow {
    V2PickerRow {
        key: format!("slackwindow:{}", window.id),
        kind: "slackwindow".to_string(),
        display_id: format!("@{}", window.channel_id),
        canonical_link: window.link.clone(),
        availability,
        group: V2PickerGroup::Conversations,
        title,
        state: snapshot.state.clone(),
        metadata: snapshot.metadata.clone(),
        snapshot: Some(snapshot),
        unavailable_reason,
        settings_destination,
        updated_at: now.to_string(),
        exact_match: false,
    }
}

fn first_line(text: &str, max: usize) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_slack;
    use std::cell::RefCell;

    const SELECTED: &str = "2026-11-21T12:00:00.000Z";

    fn channel_text(messages: &[String], cursor: Option<&str>) -> String {
        let mut body = String::from("Channel: #general (C1)\n\n");
        for ts in messages {
            body.push_str(&format!(
                "=== Message from Pat <p@example.test> (U2) at 2026-11-21 10:00:00 -03 ===\nMessage TS: {ts}\nbody {ts}\n\n"
            ));
        }
        let info = match cursor {
            Some(cursor) => {
                format!("There are more messages. To view the next page, use cursor: `{cursor}`")
            }
            None => "There are no more messages available.".to_string(),
        };
        serde_json::json!({ "messages": body, "pagination_info": info }).to_string()
    }

    fn stream(call: &ToolCall, text: &str, connector: &str) -> String {
        let mut lines = vec![
            serde_json::json!({
                "type": "system", "subtype": "init", "tools": [Tool::ReadChannel.full_name()],
                "mcp_servers": [{"name": claude_slack::SERVER_NAME, "status": connector}]
            })
            .to_string(),
        ];
        lines.push(
            serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"t1","name":call.tool.full_name(),"input":call.input}]}})
            .to_string(),
        );
        lines.push(
            serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":text}]}]}})
            .to_string(),
        );
        lines.push(serde_json::json!({"type":"result","total_cost_usd":0.002,"num_turns":2,"is_error":false}).to_string());
        lines.join("\n")
    }

    struct Fake {
        responses: RefCell<Vec<Result<String, ClaudeError>>>,
        calls: RefCell<usize>,
    }

    impl Fake {
        fn new(responses: Vec<Result<String, ClaudeError>>) -> Self {
            Self {
                responses: RefCell::new(responses),
                calls: RefCell::new(0),
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, request: &RunRequest) -> Result<String, ClaudeError> {
            assert_eq!(request.calls.len(), 1);
            assert_eq!(request.calls[0].tool, Tool::ReadChannel);
            assert_eq!(request.calls[0].input["limit"], 20);
            assert_eq!(request.timeout, Some(WINDOW_TIMEOUT));
            *self.calls.borrow_mut() += 1;
            self.responses.borrow_mut().remove(0)
        }
    }

    fn window() -> SlackWindow {
        let (oldest, latest) = bounds(SELECTED).unwrap();
        SlackWindow {
            id: "01JZTESTWINDOW000000000001".into(),
            channel_id: "C1".into(),
            selected_at: SELECTED.into(),
            oldest,
            latest,
            messages: Vec::new(),
            link: "https://acme.slack.com/archives/C1".into(),
            ready: false,
        }
    }

    fn epoch(seconds: u64) -> String {
        format!("{seconds}.000000")
    }

    #[test]
    fn bounds_are_exactly_twenty_four_hours_before_selection() {
        let (oldest, latest) = bounds(SELECTED).unwrap();
        assert_eq!(latest, SELECTED);
        assert_eq!(oldest, "2026-11-20T12:00:00.000Z");
    }

    #[test]
    fn freezers_exclude_messages_after_the_pick_and_keep_the_latest_twenty() {
        let store = V2Store::open_in_memory().unwrap();
        let selected = parse_time(SELECTED).unwrap().unix_timestamp() as u64;
        let mut ts: Vec<String> = (0..25).map(|n| epoch(selected - 3600 + n)).collect();
        ts.push(epoch(selected + 60)); // arrives after the pick; must be excluded
        let text = channel_text(&ts, None);
        let request = window_request("C1", &window().oldest, None);
        let fake = Fake::new(vec![Ok(stream(&request.calls[0], &text, "connected"))]);
        let result = freeze_window(&store, &fake, &window(), SELECTED);
        assert_eq!(result.failure, None);
        let frozen = result.window.unwrap();
        assert_eq!(frozen.messages.len(), 20);
        assert!(
            frozen
                .messages
                .iter()
                .all(|m| in_window(&m.ts, &frozen.oldest, &frozen.latest))
        );
        assert_eq!(
            frozen.messages.last().unwrap().ts,
            epoch(selected - 3600 + 24)
        );
        assert_eq!(*fake.calls.borrow(), 1);
    }

    #[test]
    fn follow_a_cursor_until_twenty_eligible_messages_are_collected() {
        let store = V2Store::open_in_memory().unwrap();
        let selected = parse_time(SELECTED).unwrap().unix_timestamp() as u64;
        let first: Vec<String> = (0..12).map(|n| epoch(selected - 2000 + n)).collect();
        let second: Vec<String> = (12..20).map(|n| epoch(selected - 2000 + n)).collect();
        let first_text = channel_text(&first, Some("page-2"));
        let second_text = channel_text(&second, None);
        let r1 = window_request("C1", &window().oldest, None);
        let r2 = window_request("C1", &window().oldest, Some("page-2"));
        let fake = Fake::new(vec![
            Ok(stream(&r1.calls[0], &first_text, "connected")),
            Ok(stream(&r2.calls[0], &second_text, "connected")),
        ]);
        let result = freeze_window(&store, &fake, &window(), SELECTED);
        assert_eq!(result.failure, None);
        assert_eq!(result.window.unwrap().messages.len(), 20);
        assert_eq!(*fake.calls.borrow(), 2);
    }

    #[test]
    fn an_empty_window_is_committed_ready_and_a_reopen_makes_no_call() {
        let store = V2Store::open_in_memory().unwrap();
        let request = window_request("C1", &window().oldest, None);
        let text = channel_text(&[], None);
        let fake = Fake::new(vec![Ok(stream(&request.calls[0], &text, "connected"))]);
        let first = freeze_window(&store, &fake, &window(), SELECTED);
        assert!(first.window.as_ref().unwrap().ready);
        assert!(first.window.as_ref().unwrap().messages.is_empty());
        let key = ReferenceKey::SlackWindow {
            id: window().id,
            channel_id: "C1".into(),
            host: "acme.slack.com".into(),
            name: None,
        };
        let reopened = resolve_window(&store, &fake, &key, SELECTED);
        assert!(reopened.from_cache);
        assert_eq!(*fake.calls.borrow(), 1);
    }

    #[test]
    fn a_pending_call_retries_once_with_the_original_bounds() {
        let store = V2Store::open_in_memory().unwrap();
        let request = window_request("C1", &window().oldest, None);
        let selected = parse_time(SELECTED).unwrap().unix_timestamp() as u64;
        let text = channel_text(&[epoch(selected - 60)], None);
        let pending = stream(&request.calls[0], "", "pending");
        let ok = stream(&request.calls[0], &text, "connected");
        let fake = Fake::new(vec![Ok(pending), Ok(ok)]);
        let result = freeze_window(&store, &fake, &window(), SELECTED);
        assert_eq!(result.failure, None);
        assert_eq!(*fake.calls.borrow(), 2);
        let frozen = result.window.unwrap();
        assert_eq!(frozen.oldest, window().oldest);
    }

    #[test]
    fn an_active_directory_scan_does_not_block_a_window_read() {
        let store = V2Store::open_in_memory().unwrap();
        let _directory_scan = store.try_begin_directory().unwrap();
        assert!(
            store.try_begin_thread("C1", "1731000000.000001").is_some(),
            "the thread/window gate is independent of the directory gate"
        );
        let selected = parse_time(SELECTED).unwrap().unix_timestamp() as u64;
        let text = channel_text(&[epoch(selected - 120)], None);
        let request = window_request("C1", &window().oldest, None);
        let fake = Fake::new(vec![Ok(stream(&request.calls[0], &text, "connected"))]);
        let result = freeze_window(&store, &fake, &window(), SELECTED);
        assert_eq!(result.failure, None);
        assert_eq!(result.window.unwrap().messages.len(), 1);
    }

    #[test]
    fn stale_content_and_bounds_are_exposed_to_the_resolved_interface() {
        let store = V2Store::open_in_memory().unwrap();
        let selected = parse_time(SELECTED).unwrap().unix_timestamp() as u64;
        let text = channel_text(&[epoch(selected - 120)], None);
        let request = window_request("C1", &window().oldest, None);
        let fake = Fake::new(vec![Ok(stream(&request.calls[0], &text, "connected"))]);
        let result = freeze_window(&store, &fake, &window(), SELECTED);
        let snapshot = result.row.snapshot.unwrap();
        assert!(snapshot.content.unwrap().contains("body"));
        assert!(
            result
                .row
                .metadata
                .iter()
                .any(|line| line.contains("window "))
        );
        assert_eq!(
            result.row.canonical_link,
            "https://acme.slack.com/archives/C1"
        );
    }
}
