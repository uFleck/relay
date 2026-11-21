//! Cache-first Slack thread resolution. Core owns the permalink root, the one-call-per-attempt
//! contract, the immediate retry, the parent-with-replies parser and the read-only Lens shape.
//! The Tauri adapter only supplies a `Runner` process and a wall clock; no token or Slack
//! response body ever leaves this module as raw text.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude_slack::{
    self, ClaudeError, RunRequest, Runner, Tool, ToolCall, build_prompt, parse_stream,
};
use crate::provider::ProviderFailure;

use super::reference::{
    ReferenceAvailability, ReferenceKey, ReferenceSnapshot, V2PickerGroup, V2PickerRow,
};
use super::store::V2Store;

/// One on-demand thread attempt gets 30 seconds, independent of the 90-second directory scan.
pub const THREAD_TIMEOUT: Duration = Duration::from_secs(30);
/// A pending connector, a timeout and a skipped tool call each get exactly one immediate retry.
const MAX_ATTEMPTS: usize = 2;

/// A message as stored in the v2 cache. Only read-only context is persisted: no Slack token,
/// no raw tool payload and no v1 episode segmentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedMessage {
    pub ts: String,
    pub author: String,
    pub text: String,
}

/// A verified thread. `parent` is required for a store hit; a parent-only thread has zero replies
/// and is still a valid hit. `fetched_at` is Relay's UTC clock, not Slack's message time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackThread {
    pub channel_id: String,
    pub thread_ts: String,
    pub parent: CachedMessage,
    pub replies: Vec<CachedMessage>,
    pub fetched_at: String,
}

/// Why an attempt did not produce a complete thread. The reason is safe for the Lens; no raw
/// Slack error is carried. `NotFound` ends the current attempt without another call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadFailure {
    /// `channel_not_found` or a thread the account cannot see.
    NotFound,
    /// Connector `pending`, a run timeout, or a skipped `slack_read_thread` call.
    Pending,
    /// `claude` is not installed: Settings recovery, pause until it appears on PATH.
    MissingCli,
    /// The connector is down or signed out: pause until Settings changes.
    Configuration,
    /// A malformed or truncated response that could not establish the requested parent.
    Malformed(String),
}

impl ThreadFailure {
    pub fn reason_text(&self) -> String {
        match self {
            Self::NotFound => "Not found or not visible to you".to_string(),
            Self::Pending => "Slack is still connecting. Try again shortly".to_string(),
            Self::MissingCli => {
                "Claude Code not found. Install it, then sign in once with `claude`.".to_string()
            }
            Self::Configuration => {
                "Claude Slack connector is not connected or signed out".to_string()
            }
            Self::Malformed(detail) => {
                let short: String = detail.chars().take(120).collect();
                if short.is_empty() {
                    "Slack returned an unreadable thread".to_string()
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

    /// Maps onto #30's shared recovery reasons so a background pass pauses for setup problems
    /// and retries transient or unknown unreadability on the fixed schedule.
    pub fn to_provider_failure(&self) -> ProviderFailure {
        match self {
            Self::NotFound => ProviderFailure::NotFound,
            Self::Pending | Self::Malformed(_) => ProviderFailure::Unavailable,
            Self::MissingCli | Self::Configuration => ProviderFailure::Configuration,
        }
    }

    /// Only a pending connector, a timeout or a skipped call earns another immediate attempt.
    fn immediate_retry(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// The outcome of one resolution. `row` is always present so the Lens has a linked ID even when
/// the target is unreadable; `thread` is present only on a full hit.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadResolution {
    pub row: V2PickerRow,
    pub thread: Option<SlackThread>,
    /// True when SQLite answered before any CLI call was started.
    pub from_cache: bool,
    /// True when another attempt for the same thread already held the guard.
    pub coalesced: bool,
    pub failure: Option<ThreadFailure>,
}

/// The tool result of a `slack_read_thread` attempt, parsed with the parent kept alongside the
/// ordered replies. A truncated result or a parent that is not the requested root is not a hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedThread {
    pub parent: CachedMessage,
    pub replies: Vec<CachedMessage>,
    pub truncated: bool,
}

/// The single `slack_read_thread` call for a thread root. The model sees only this tool, gets one
/// exact argument object and a 30-second deadline.
pub fn thread_request(channel_id: &str, thread_ts: &str) -> RunRequest {
    let call = ToolCall::read_thread(channel_id, thread_ts);
    RunRequest {
        calls: vec![call.clone()],
        prompt: build_prompt(std::slice::from_ref(&call)),
        max_turns: 2,
        disallow: Vec::new(),
        timeout: Some(THREAD_TIMEOUT),
        tools: Some(vec![Tool::ReadThread.full_name()]),
    }
}

/// Parse a `slack_read_thread` result, keeping the parent. This is the v2 reader; v1's
/// `claude_slack::parse_thread` still returns replies only and is unchanged.
pub fn parse_thread_full(text: &str, root_ts: &str) -> Result<ParsedThread, ClaudeError> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|_| {
        let head: String = text.chars().take(120).collect();
        ClaudeError::Tool(head)
    })?;
    let body = value
        .get("messages")
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeError::Parse("thread result has no messages field".into()))?;
    let lines: Vec<&str> = body.lines().collect();
    let truncated = body.to_ascii_lowercase().contains("truncat")
        || value
            .get("pagination_info")
            .and_then(Value::as_str)
            .is_some_and(|info| !info.contains("no more"));

    let header = |line: &&str| line.starts_with("=== THREAD");
    let parent_at = lines
        .iter()
        .position(|line| header(line) && line.contains("PARENT"));
    let replies_at = lines
        .iter()
        .position(|line| header(line) && !line.contains("PARENT"));
    let Some(parent_header) = parent_at else {
        return Err(ClaudeError::Parse(
            "thread result has no parent message".into(),
        ));
    };
    let parent_end = replies_at.unwrap_or(lines.len()).max(parent_header + 1);
    let parents = parse_blocks(&lines, parent_header + 1, parent_end);
    let parent = parents
        .into_iter()
        .next()
        .ok_or_else(|| ClaudeError::Parse("thread parent has no message body".into()))?;
    if !same_root(&parent.ts, root_ts) {
        return Err(ClaudeError::Parse(
            "thread parent is not the requested message".into(),
        ));
    }
    let replies = match replies_at {
        Some(at) => parse_blocks(&lines, at + 1, lines.len()),
        None => Vec::new(),
    };
    Ok(ParsedThread {
        parent,
        replies,
        truncated,
    })
}

fn seconds_part(ts: &str) -> &str {
    ts.trim().split('.').next().unwrap_or("")
}

fn same_root(found: &str, requested: &str) -> bool {
    !found.trim().is_empty() && seconds_part(found) == seconds_part(requested)
}

fn is_start(lines: &[&str], i: usize) -> bool {
    lines[i].starts_with("From:")
        && lines
            .get(i + 1)
            .is_some_and(|line| line.starts_with("Time:"))
        && lines
            .get(i + 2)
            .is_some_and(|line| line.starts_with("Message TS:"))
}

fn parse_blocks(lines: &[&str], from: usize, to: usize) -> Vec<CachedMessage> {
    if from >= to {
        return Vec::new();
    }
    let starts: Vec<usize> = (from..to).filter(|&i| is_start(lines, i)).collect();
    let mut out = Vec::new();
    for (n, &at) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(to);
        let (author, _) = claude_slack::sender_from_header(lines[at]["From:".len()..].trim());
        let ts = lines[at + 2]["Message TS:".len()..].trim().to_string();
        let mut tail: Vec<&str> = lines[at + 3..end].to_vec();
        while tail
            .last()
            .is_some_and(|line| line.trim().is_empty() || line.trim() == "---")
        {
            tail.pop();
        }
        while tail
            .first()
            .is_some_and(|line| line.trim().is_empty() || line.trim() == "---")
        {
            tail.remove(0);
        }
        let text = claude_slack::body_and_marker(&tail).0;
        out.push(CachedMessage {
            ts,
            author: if author.is_empty() {
                "unknown".to_string()
            } else {
                author
            },
            text,
        });
    }
    out
}

/// Runs a cache-first resolution. A SQLite hit answers before any CLI call; a miss takes the
/// per-thread guard, then makes at most two one-call attempts. The guard is dropped on every
/// return path and no SQLite connection is held across a `Runner::run` call.
pub fn resolve_thread(
    store: &V2Store,
    runner: &dyn Runner,
    key: &ReferenceKey,
    now: &str,
) -> ThreadResolution {
    let (channel_id, thread_ts) = match key {
        ReferenceKey::Slack {
            channel_id,
            thread_ts,
            ..
        } => (channel_id.clone(), thread_ts.clone()),
        _ => {
            return unavailable(
                key,
                &ThreadFailure::Malformed("not a Slack thread reference".into()),
                now,
                false,
            );
        }
    };

    match store.slack_thread(&channel_id, &thread_ts) {
        Ok(Some(thread)) => return hit(key, &thread, now, true),
        _ => {}
    }
    let Some(_guard) = store.try_begin_thread(&channel_id, &thread_ts) else {
        return loading(key, now);
    };

    let request = thread_request(&channel_id, &thread_ts);
    let mut failure = ThreadFailure::Pending;
    for _ in 0..MAX_ATTEMPTS {
        let stdout = match runner.run(&request) {
            Ok(stdout) => stdout,
            Err(error) => {
                failure = classify_error(&error);
                if !failure.immediate_retry() {
                    break;
                }
                continue;
            }
        };
        let data = match parse_stream(&stdout, &request.calls) {
            Ok(data) => data,
            Err(error) => {
                failure = classify_error(&error);
                if !failure.immediate_retry() {
                    break;
                }
                continue;
            }
        };
        if data.connector_status.as_deref() != Some("connected") {
            failure = if data.connector_status.as_deref() == Some("pending") {
                ThreadFailure::Pending
            } else {
                ThreadFailure::Configuration
            };
            if !failure.immediate_retry() {
                break;
            }
            continue;
        }
        if data.tool_use_count == 0 {
            // The connector was up but the model skipped the requested call.
            failure = ThreadFailure::Pending;
            continue;
        }
        if data.tool_use_count != 1 || data.duplicate_tool_use {
            failure = ThreadFailure::Malformed(
                "the run made more than one or a duplicate slack_read_thread call".into(),
            );
            break;
        }
        let Some(output) = data.results.first().and_then(|slot| slot.as_ref()) else {
            failure = ThreadFailure::Pending;
            continue;
        };
        if output.failed() {
            let text = output.text.to_ascii_lowercase();
            failure = if text.contains("channel_not_found") || text.contains("not_in_channel") {
                ThreadFailure::NotFound
            } else if text.contains("invalid_auth") || text.contains("not_authed") {
                ThreadFailure::Configuration
            } else {
                ThreadFailure::Malformed("slack_read_thread returned a tool error".into())
            };
            break;
        }
        match parse_thread_full(&output.text, &thread_ts) {
            Ok(parsed) if parsed.truncated => {
                failure = ThreadFailure::Malformed("Slack reported a truncated thread".into());
                break;
            }
            Ok(parsed) => {
                let thread = SlackThread {
                    channel_id: channel_id.clone(),
                    thread_ts: thread_ts.clone(),
                    parent: parsed.parent,
                    replies: parsed.replies,
                    fetched_at: now.to_string(),
                };
                // A failed or incomplete write leaves a miss: the transaction either commits the
                // whole thread or none of it, and a parent is required before anything is written.
                if store.save_slack_thread(&thread).is_err() {
                    failure =
                        ThreadFailure::Malformed("could not persist the verified thread".into());
                    break;
                }
                return hit(key, &thread, now, false);
            }
            Err(error) => {
                failure = classify_error(&error);
                break;
            }
        }
    }
    unavailable(key, &failure, now, false)
}

fn classify_error(error: &ClaudeError) -> ThreadFailure {
    match error {
        ClaudeError::Timeout => ThreadFailure::Pending,
        ClaudeError::ConnectorDown(status) if status == "pending" => ThreadFailure::Pending,
        ClaudeError::NotFound => ThreadFailure::MissingCli,
        ClaudeError::ConnectorDown(_) => ThreadFailure::Configuration,
        ClaudeError::Tool(text) if text.contains("channel_not_found") => ThreadFailure::NotFound,
        ClaudeError::Rejected(_)
        | ClaudeError::Cli(_)
        | ClaudeError::Tool(_)
        | ClaudeError::Parse(_) => ThreadFailure::Malformed(error.to_string()),
        ClaudeError::Store(_) => ThreadFailure::Malformed("the cache is unavailable".into()),
    }
}

fn hit(key: &ReferenceKey, thread: &SlackThread, now: &str, from_cache: bool) -> ThreadResolution {
    let summary = first_line(&thread.parent.text, 80);
    let title = if summary.is_empty() {
        "Slack thread".to_string()
    } else {
        summary
    };
    let metadata = vec![
        format!("channel {}", thread.channel_id),
        format!(
            "{} {}",
            thread.replies.len(),
            if thread.replies.len() == 1 {
                "reply"
            } else {
                "replies"
            }
        ),
    ];
    let mut content = format!("{}: {}", thread.parent.author, thread.parent.text);
    for reply in &thread.replies {
        content.push_str(&format!("\n{}: {}", reply.author, reply.text));
    }
    let snapshot = ReferenceSnapshot {
        title: Some(title.clone()),
        state: Some("resolved".to_string()),
        metadata,
        cached_at: Some(thread.fetched_at.clone()),
        reason: None,
        content: Some(content),
        channel_name: None,
    };
    let row = row_for(
        key,
        snapshot,
        ReferenceAvailability::Ready,
        title,
        None,
        None,
        Some(thread.fetched_at.clone()),
        now,
    );
    ThreadResolution {
        row,
        thread: Some(thread.clone()),
        from_cache,
        coalesced: false,
        failure: None,
    }
}

fn loading(key: &ReferenceKey, now: &str) -> ThreadResolution {
    let snapshot = ReferenceSnapshot {
        title: Some("fetching…".to_string()),
        ..ReferenceSnapshot::default()
    };
    let row = row_for(
        key,
        snapshot,
        ReferenceAvailability::Ready,
        "fetching…".to_string(),
        None,
        None,
        None,
        now,
    );
    ThreadResolution {
        row,
        thread: None,
        from_cache: false,
        coalesced: true,
        failure: None,
    }
}

/// Builds the read-only unavailable row for an adapter that cannot start a run at all, such as
/// a missing `claude` binary. No CLI call has been attempted.
pub fn failed_resolution(
    key: &ReferenceKey,
    failure: &ThreadFailure,
    now: &str,
) -> ThreadResolution {
    unavailable(key, failure, now, false)
}

fn unavailable(
    key: &ReferenceKey,
    failure: &ThreadFailure,
    now: &str,
    coalesced: bool,
) -> ThreadResolution {
    let reason = failure.reason_text();
    let snapshot = ReferenceSnapshot {
        reason: Some(reason.clone()),
        ..ReferenceSnapshot::default()
    };
    let row = row_for(
        key,
        snapshot,
        ReferenceAvailability::Unavailable,
        String::new(),
        Some(reason),
        failure.settings_destination(),
        None,
        now,
    );
    ThreadResolution {
        row,
        thread: None,
        from_cache: false,
        coalesced,
        failure: Some(failure.clone()),
    }
}

#[allow(clippy::too_many_arguments)]
fn row_for(
    key: &ReferenceKey,
    snapshot: ReferenceSnapshot,
    availability: ReferenceAvailability,
    title: String,
    unavailable_reason: Option<String>,
    settings_destination: Option<String>,
    updated_at: Option<String>,
    now: &str,
) -> V2PickerRow {
    let alias = match key {
        ReferenceKey::Slack { channel_id, .. } => Some(channel_id.as_str()),
        _ => None,
    };
    V2PickerRow {
        key: key.identity(),
        kind: key.kind().to_string(),
        display_id: key.display_id(alias),
        canonical_link: key.canonical_url(),
        availability,
        group: V2PickerGroup::Conversations,
        title,
        state: snapshot.state.clone(),
        metadata: snapshot.metadata.clone(),
        snapshot: Some(snapshot),
        unavailable_reason,
        settings_destination,
        updated_at: updated_at.unwrap_or_else(|| now.to_string()),
        exact_match: false,
    }
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_slack::{SERVER_NAME, ToolOutput};
    use std::cell::RefCell;

    fn thread_text(parent_ts: &str, replies: &[(&str, &str, &str)]) -> String {
        let mut body = format!(
            "=== THREAD PARENT MESSAGE ===\nFrom: Pat Peer <pat@example.test> (U0PEER)\nTime: 2026-01-02 10:00:00 -03\nMessage TS: {parent_ts}\nparent text\n\n=== THREAD REPLIES ({}) total ===\n\n",
            replies.len()
        );
        for (uid, ts, text) in replies {
            body.push_str(&format!(
                "---\nFrom: Someone <s@example.test> ({uid})\nTime: 2026-01-02 11:00:00 -03\nMessage TS: {ts}\n{text}\n\n"
            ));
        }
        serde_json::json!({
            "messages": body,
            "pagination_info": "There are no more messages in this thread.\n"
        })
        .to_string()
    }

    struct Fake {
        responses: RefCell<Vec<Result<String, ClaudeError>>>,
        calls: RefCell<usize>,
        _connector: &'static str,
    }

    impl Fake {
        fn new(responses: Vec<Result<String, ClaudeError>>) -> Self {
            Self {
                responses: RefCell::new(responses),
                calls: RefCell::new(0),
                _connector: "connected",
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, request: &RunRequest) -> Result<String, ClaudeError> {
            assert_eq!(request.calls.len(), 1, "one call per attempt");
            assert_eq!(request.calls[0].tool, Tool::ReadThread);
            assert_eq!(request.timeout, Some(THREAD_TIMEOUT));
            assert_eq!(
                request.tools.as_deref(),
                Some(&[Tool::ReadThread.full_name()][..])
            );
            *self.calls.borrow_mut() += 1;
            self.responses.borrow_mut().remove(0)
        }
    }

    fn stream(connector: &str, call: &ToolCall, result: Option<ToolOutput>) -> String {
        let mut lines = vec![
            serde_json::json!({
                "type": "system", "subtype": "init",
                "tools": [Tool::ReadThread.full_name()],
                "mcp_servers": [{"name": SERVER_NAME, "status": connector}]
            })
            .to_string(),
        ];
        if let Some(output) = result {
            lines.push(
                serde_json::json!({"type":"assistant","message":{"content":[
                    {"type":"tool_use","id":"t1","name":call.tool.full_name(),"input":call.input}]}})
                .to_string(),
            );
            lines.push(
                serde_json::json!({"type":"user","message":{"content":[
                    {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":output.text}]}]}})
                .to_string(),
            );
        }
        lines.push(
            serde_json::json!({"type":"result","total_cost_usd":0.002,"num_turns":2,"is_error":false})
                .to_string(),
        );
        lines.join("\n")
    }

    fn slack_key() -> ReferenceKey {
        ReferenceKey::Slack {
            channel_id: "C123".into(),
            thread_ts: "1731000000.000001".into(),
            host: "acme.slack.com".into(),
            permalink_ts: None,
        }
    }

    #[test]
    fn parser_keeps_the_parent_and_ordered_replies() {
        let parsed = parse_thread_full(
            &thread_text(
                "1731000000.000001",
                &[
                    ("U2", "1731000001.000001", "first"),
                    ("U3", "1731000002.000001", "second"),
                ],
            ),
            "1731000000.000001",
        )
        .unwrap();
        assert!(!parsed.truncated);
        assert_eq!(parsed.parent.text, "parent text");
        assert_eq!(parsed.parent.ts, "1731000000.000001");
        assert_eq!(parsed.replies.len(), 2);
        assert_eq!(parsed.replies[0].text, "first");
        assert_eq!(parsed.replies[1].text, "second");
    }

    #[test]
    fn parser_rejects_missing_or_mismatched_parent_and_truncation() {
        assert!(
            parse_thread_full(&thread_text("1731000000.000001", &[]), "1731999999.000001").is_err()
        );
        let truncated = serde_json::json!({
            "messages": "=== THREAD PARENT MESSAGE ===\nFrom: Pat <p@example.test> (U1)\nTime: now\nMessage TS: 1731000000.000001\nparent\n\n=== THREAD REPLIES (0 total) ===\n",
            "pagination_info": "more messages available"
        })
        .to_string();
        assert!(
            parse_thread_full(&truncated, "1731000000.000001")
                .unwrap()
                .truncated
        );
    }

    #[test]
    fn cold_success_caches_and_a_reopen_hit_makes_no_call() {
        let store = V2Store::open_in_memory().unwrap();
        let key = slack_key();
        let request = thread_request("C123", "1731000000.000001");
        let text = thread_text("1731000000.000001", &[("U2", "1731000001.000001", "reply")]);
        let fake = Fake::new(vec![Ok(stream(
            "connected",
            &request.calls[0],
            Some(ToolOutput {
                text,
                is_error: false,
            }),
        ))]);
        let first = resolve_thread(&store, &fake, &key, "2026-11-21T00:00:00.000Z");
        assert!(!first.from_cache);
        assert_eq!(first.failure, None);
        assert_eq!(first.thread.as_ref().unwrap().replies.len(), 1);
        assert_eq!(*fake.calls.borrow(), 1);

        let second = resolve_thread(&store, &fake, &key, "2026-11-21T01:00:00.000Z");
        assert!(second.from_cache);
        assert_eq!(*fake.calls.borrow(), 1, "a SQLite hit starts no CLI call");
    }

    #[test]
    fn pending_timeout_and_skipped_call_retry_once() {
        for response in [
            Ok(stream(
                "pending",
                &ToolCall::read_thread("C123", "1731000000.000001"),
                None,
            )),
            Err(ClaudeError::Timeout),
            Ok(stream(
                "connected",
                &ToolCall::read_thread("C123", "1731000000.000001"),
                None,
            )),
        ] {
            let store = V2Store::open_in_memory().unwrap();
            let request = thread_request("C123", "1731000000.000001");
            let text = thread_text("1731000000.000001", &[]);
            let fake = Fake::new(vec![
                response,
                Ok(stream(
                    "connected",
                    &request.calls[0],
                    Some(ToolOutput {
                        text,
                        is_error: false,
                    }),
                )),
            ]);
            let outcome = resolve_thread(&store, &fake, &slack_key(), "2026-11-21T00:00:00.000Z");
            assert_eq!(*fake.calls.borrow(), 2, "exactly one immediate retry");
            assert_eq!(outcome.failure, None);
            assert!(outcome.thread.is_some());
        }
    }

    #[test]
    fn channel_not_found_ends_the_attempt_without_a_retry() {
        let store = V2Store::open_in_memory().unwrap();
        let call = ToolCall::read_thread("C123", "1731000000.000001");
        let fake = Fake::new(vec![Ok(stream(
            "connected",
            &call,
            Some(ToolOutput {
                text: "channel_not_found: channel does not exist".into(),
                is_error: true,
            }),
        ))]);
        let outcome = resolve_thread(&store, &fake, &slack_key(), "2026-11-21T00:00:00.000Z");
        assert_eq!(*fake.calls.borrow(), 1);
        assert_eq!(outcome.failure, Some(ThreadFailure::NotFound));
        assert!(
            outcome
                .row
                .unavailable_reason
                .unwrap()
                .contains("not visible")
        );
    }

    #[test]
    fn malformed_streams_are_rejected_without_a_cache_hit() {
        let store = V2Store::open_in_memory().unwrap();
        let call = ToolCall::read_thread("C123", "1731000000.000001");
        let fake = Fake::new(vec![Ok(stream(
            "connected",
            &call,
            Some(ToolOutput {
                text: "not json".into(),
                is_error: false,
            }),
        ))]);
        let outcome = resolve_thread(&store, &fake, &slack_key(), "2026-11-21T00:00:00.000Z");
        assert!(matches!(outcome.failure, Some(ThreadFailure::Malformed(_))));
        assert!(outcome.thread.is_none());
        assert!(
            store
                .slack_thread("C123", "1731000000.000001")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn duplicate_and_unexpected_tool_calls_are_rejected() {
        let store = V2Store::open_in_memory().unwrap();
        let call = ToolCall::read_thread("C123", "1731000000.000001");
        let init = serde_json::json!({
            "type": "system", "subtype": "init",
            "tools": [Tool::ReadThread.full_name(), Tool::UserProfile.full_name()],
            "mcp_servers": [{"name": SERVER_NAME, "status": "connected"}]
        })
        .to_string();
        let result = serde_json::json!({
            "type": "result", "total_cost_usd": 0.002, "num_turns": 2, "is_error": false
        })
        .to_string();

        let duplicate = [
            init.clone(),
            serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":call.tool.full_name(),"input":call.input},
                {"type":"tool_use","id":"b","name":call.tool.full_name(),"input":call.input}
            ]}})
            .to_string(),
            result.clone(),
        ]
        .join("\n");
        let fake = Fake::new(vec![Ok(duplicate)]);
        let outcome = resolve_thread(&store, &fake, &slack_key(), "now");
        assert!(matches!(outcome.failure, Some(ThreadFailure::Malformed(_))));
        assert_eq!(*fake.calls.borrow(), 1, "a rejected stream is not retried");

        let unexpected = [
            init,
            serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":Tool::UserProfile.full_name(),"input":{}}
            ]}})
            .to_string(),
            result,
        ]
        .join("\n");
        let fake = Fake::new(vec![Ok(unexpected)]);
        let outcome = resolve_thread(&store, &fake, &slack_key(), "now");
        assert!(matches!(outcome.failure, Some(ThreadFailure::Malformed(_))));
        assert_eq!(*fake.calls.borrow(), 1);
    }

    #[test]
    fn same_key_coalesces_and_other_keys_are_independent() {
        let store = V2Store::open_in_memory().unwrap();
        let key = slack_key();
        let first = store.try_begin_thread("C123", "1731000000.000001").unwrap();
        let coalesced = resolve_thread(&store, &Fake::new(vec![]), &key, "now");
        assert!(coalesced.coalesced);
        assert!(coalesced.thread.is_none());
        let independent = store.try_begin_thread("C999", "1731000000.000001").unwrap();
        drop(independent);
        drop(first);
        assert!(
            store
                .try_begin_thread("C123", "1731000000.000001")
                .is_some()
        );
    }
}
