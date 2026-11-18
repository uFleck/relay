//! Claude Slack proxy: Slack 1:1 and group DMs without a Slack app token.
//!
//! Relay shells out to headless Claude Code (`claude -p`, haiku) that has the user's claude.ai
//! Slack connector. Relay computes every tool call's exact arguments; the model only executes
//! them, and Relay reads the raw tool results from the stream-json output. This module is pure
//! (no process, no Tauri): the `Runner` trait is the seam, so tests inject a fake stream.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::model::{self, RawSlackMessage};
use crate::store::Store;

pub const SERVER_NAME: &str = "claude.ai Slack";
const PREFIX: &str = "mcp__claude_ai_Slack__";
pub const SYSTEM_PROMPT: &str =
    "You execute Slack MCP tool calls exactly as instructed and nothing else.";

const KEY_SELF: &str = "claude_slack.self_id";
const KEY_WATERMARK: &str = "claude_slack.watermark";
const KEY_DISALLOW: &str = "claude_slack.disallow";
const KEY_CHANNELS: &str = "claude_slack.channels";
const KEY_RUNS: &str = "claude_slack.runs";
const THREAD_CONNECTOR: &str = "claude-slack-thread";

pub const BATCH: usize = 8;
const ATTEMPTS: usize = 3;
const DISCOVERY_PAGES: usize = 5;
const CHANNEL_PAGES: usize = 5;
const THREADS_PER_RUN: usize = 24;
const CHANNEL_LIMIT: i64 = 100;
const MAX_RUNS_KEPT: usize = 50;
pub const FIRST_WINDOW_DAYS: i64 = 3;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ClaudeError {
    #[error("Claude Code was not found. Install it, then sign in once with `claude`.")]
    NotFound,
    #[error(
        "the claude.ai Slack connector is not connected ({0}). Connect it in claude.ai settings."
    )]
    ConnectorDown(String),
    #[error("the run used a tool outside the allowlist: {0}")]
    Rejected(String),
    #[error("claude did not finish in time")]
    Timeout,
    #[error("claude failed: {0}")]
    Cli(String),
    #[error("Slack tool error: {0}")]
    Tool(String),
    #[error("unexpected Slack output: {0}")]
    Parse(String),
    #[error("{0}")]
    Store(String),
}

impl From<crate::store::StoreError> for ClaudeError {
    fn from(error: crate::store::StoreError) -> Self {
        ClaudeError::Store(error.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Search,
    ReadChannel,
    ReadThread,
    UserProfile,
}

impl Tool {
    pub const ALL: [Tool; 4] = [
        Tool::Search,
        Tool::ReadChannel,
        Tool::ReadThread,
        Tool::UserProfile,
    ];

    pub fn full_name(self) -> String {
        let short = match self {
            Tool::Search => "slack_search_public_and_private",
            Tool::ReadChannel => "slack_read_channel",
            Tool::ReadThread => "slack_read_thread",
            Tool::UserProfile => "slack_read_user_profile",
        };
        format!("{PREFIX}{short}")
    }

    pub fn from_full_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.full_name() == name)
    }
}

/// Every tool the run may use. `slack_read_user_profile` is the self-id probe.
pub fn allowed_tools() -> Vec<String> {
    Tool::ALL.iter().map(|tool| tool.full_name()).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub tool: Tool,
    pub input: Value,
}

impl ToolCall {
    pub fn user_profile() -> Self {
        Self {
            tool: Tool::UserProfile,
            input: json!({}),
        }
    }

    /// Own messages in 1:1 and group DMs since `after_ts` (Unix seconds, inclusive).
    pub fn search(self_id: &str, after_secs: i64, cursor: Option<&str>) -> Self {
        let mut input = json!({
            "filters": format!("from:<@{self_id}>"),
            "after": after_secs.to_string(),
            "channel_types": "im,mpim",
            "sort": "timestamp",
            "natural_language_query": "",
            "include_context": false,
            "limit": 20,
        });
        if let Some(cursor) = cursor {
            input["cursor"] = json!(cursor);
        }
        Self {
            tool: Tool::Search,
            input,
        }
    }

    pub fn read_channel(channel: &str, oldest: &str, cursor: Option<&str>) -> Self {
        let mut input = json!({
            "channel_id": channel,
            "oldest": oldest,
            "limit": CHANNEL_LIMIT,
        });
        if let Some(cursor) = cursor {
            input["cursor"] = json!(cursor);
        }
        Self {
            tool: Tool::ReadChannel,
            input,
        }
    }

    pub fn read_thread(channel: &str, message_ts: &str) -> Self {
        Self {
            tool: Tool::ReadThread,
            input: json!({ "channel_id": channel, "message_ts": message_ts }),
        }
    }
}

/// The prompt for one invocation: the model copies these calls and replies "done".
pub fn build_prompt(calls: &[ToolCall]) -> String {
    let list: Vec<String> = calls
        .iter()
        .map(|call| format!("{} with {}", call.tool.full_name(), call.input))
        .collect();
    format!(
        "Make these {} calls in parallel, in a single turn, with exactly these arguments: {}. \
         Add no other parameters. Do not summarize. When they return, reply with the single word: done",
        calls.len(),
        list.join(", ")
    )
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub calls: Vec<ToolCall>,
    pub prompt: String,
    pub max_turns: u32,
    /// Tools and `mcp__<server>` prefixes to remove from the model's context.
    pub disallow: Vec<String>,
}

/// The `claude` argument list for a request. `settings_json` turns hooks and plugins off.
pub fn cli_args(request: &RunRequest, settings_json: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        request.prompt.as_str(),
        "--model",
        "haiku",
        "--effort",
        "low",
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--max-turns",
    ]
    .iter()
    .map(|part| part.to_string())
    .collect();
    args.push(request.max_turns.to_string());
    for part in [
        "--disable-slash-commands",
        "--system-prompt",
        SYSTEM_PROMPT,
        "--settings",
        settings_json,
        "--allowedTools",
    ] {
        args.push(part.to_string());
    }
    args.push(allowed_tools().join(" "));
    if !request.disallow.is_empty() {
        args.push("--disallowedTools".into());
        args.push(request.disallow.join(" "));
    }
    args
}

/// `--settings` JSON: no hooks, every user-enabled plugin switched off for this run.
pub fn run_settings_json(enabled_plugins: &[String]) -> String {
    let plugins: serde_json::Map<String, Value> = enabled_plugins
        .iter()
        .map(|name| (name.clone(), Value::Bool(false)))
        .collect();
    json!({ "disableAllHooks": true, "enabledPlugins": plugins }).to_string()
}

/// Runs one `claude` invocation and returns its stdout (the stream-json lines).
pub trait Runner {
    fn run(&self, request: &RunRequest) -> Result<String, ClaudeError>;
}

// ---------------------------------------------------------------------------
// Stream parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    /// Tool errors arrive as plain text; real results are JSON objects.
    fn failed(&self) -> bool {
        self.is_error || !self.text.trim_start().starts_with('{')
    }
}

#[derive(Debug, Clone, Default)]
pub struct StreamData {
    pub init_tools: Vec<String>,
    /// One slot per requested call; `None` when the model never ran it.
    pub results: Vec<Option<ToolOutput>>,
    pub cost_usd: f64,
    pub turns: u32,
    pub is_error: bool,
    pub result_text: Option<String>,
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Arguments without empty-string and null entries: the model sometimes adds `"user_id": ""`
/// to a call that takes none, which means the same thing.
fn normalized(input: &Value) -> Value {
    match input {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null() && v.as_str() != Some(""))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Matches tool results to `calls` by `tool_use_id`. Rejects the whole run when the model used
/// a tool outside the allowlist or arguments Relay did not ask for.
pub fn parse_stream(stdout: &str, calls: &[ToolCall]) -> Result<StreamData, ClaudeError> {
    let mut data = StreamData {
        results: vec![None; calls.len()],
        ..StreamData::default()
    };
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    let mut connector: Option<String> = None;
    let mut saw_init = false;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("system") if event.get("subtype").and_then(Value::as_str) == Some("init") => {
                saw_init = true;
                data.init_tools = event
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(|tools| {
                        tools
                            .iter()
                            .filter_map(|tool| tool.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                connector = event
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .and_then(|servers| {
                        servers.iter().find(|server| {
                            server.get("name").and_then(Value::as_str) == Some(SERVER_NAME)
                        })
                    })
                    .and_then(|server| server.get("status").and_then(Value::as_str))
                    .map(str::to_string);
            }
            Some("assistant") => {
                let blocks = event
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let Some(tool) = Tool::from_full_name(name) else {
                        return Err(ClaudeError::Rejected(name.to_string()));
                    };
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let Some(index) = calls.iter().position(|call| {
                        call.tool == tool && normalized(&call.input) == normalized(&input)
                    }) else {
                        return Err(ClaudeError::Rejected(format!(
                            "{name} with unrequested arguments"
                        )));
                    };
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                    uses.insert(id.to_string(), index);
                }
            }
            Some("user") => {
                let blocks = event
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(&index) = uses.get(id) {
                        let output = ToolOutput {
                            text: content_text(block.get("content").unwrap_or(&Value::Null)),
                            is_error: block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        };
                        // A model retry of the same call replaces a failed first try.
                        let keep = data.results[index]
                            .as_ref()
                            .is_some_and(|old| !old.failed() && output.failed());
                        if !keep {
                            data.results[index] = Some(output);
                        }
                    }
                }
            }
            Some("result") => {
                data.cost_usd = event
                    .get("total_cost_usd")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                data.turns = event.get("num_turns").and_then(Value::as_u64).unwrap_or(0) as u32;
                data.is_error = event
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                data.result_text = event
                    .get("result")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    if saw_init {
        match connector.as_deref() {
            Some("connected") => {}
            Some(status) => return Err(ClaudeError::ConnectorDown(status.to_string())),
            None => return Err(ClaudeError::ConnectorDown("not listed".into())),
        }
    } else {
        let detail = data
            .result_text
            .clone()
            .unwrap_or_else(|| "no init event in the output".into());
        return Err(ClaudeError::Cli(detail));
    }
    Ok(data)
}

/// Everything in the init `tools` list that is not allowed, collapsed to `mcp__<server>`
/// prefixes. The Slack server is listed per tool so the allowed ones stay.
pub fn learn_disallow(init_tools: &[String]) -> BTreeSet<String> {
    let allowed = allowed_tools();
    let mut out = BTreeSet::new();
    for tool in init_tools {
        if allowed.contains(tool) {
            continue;
        }
        match tool
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split("__").next())
        {
            Some("claude_ai_Slack") => {
                out.insert(tool.clone());
            }
            Some(server) => {
                out.insert(format!("mcp__{server}"));
            }
            None => {
                out.insert(tool.clone());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Result parsers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dm,
    Group,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub channel_id: String,
    pub kind: Kind,
    pub participants: Vec<Person>,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMarker {
    pub replies: u32,
    pub latest: String,
}

impl ThreadMarker {
    fn signature(&self) -> String {
        format!("{}|{}", self.replies, self.latest)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub ts: String,
    pub user_id: String,
    pub name: String,
    pub text: String,
    pub thread: Option<ThreadMarker>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChannelPage {
    pub kind: Option<Kind>,
    pub messages: Vec<ChatMessage>,
    pub next_cursor: Option<String>,
}

fn json_fields(text: &str) -> Result<Value, ClaudeError> {
    serde_json::from_str::<Value>(text.trim()).map_err(|_| {
        let head: String = text.chars().take(120).collect();
        ClaudeError::Tool(head)
    })
}

fn cursor_from(info: &str) -> Option<String> {
    let start = info.find('`')? + 1;
    let end = info[start..].find('`')? + start;
    let cursor = info[start..end].trim();
    (!cursor.is_empty()).then(|| cursor.to_string())
}

fn people(list: &str) -> Vec<Person> {
    let mut out = Vec::new();
    let mut rest = list;
    while let Some(at) = rest.find("(ID: ") {
        let name = rest[..at].trim().trim_start_matches(',').trim().to_string();
        let after = &rest[at + 5..];
        let Some(end) = after.find(')') else { break };
        out.push(Person {
            id: after[..end].trim().to_string(),
            name,
        });
        rest = &after[end + 1..];
    }
    out
}

fn id_in_parens(text: &str) -> Option<String> {
    let start = text.rfind('(')? + 1;
    let end = text[start..].find(')')? + start;
    let inner = text[start..end].trim();
    let inner = inner.strip_prefix("ID:").unwrap_or(inner).trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

fn kind_of(label: &str) -> Option<Kind> {
    let label = label.trim();
    if label.starts_with("Group DM") {
        Some(Kind::Group)
    } else if label.starts_with("DM") {
        Some(Kind::Dm)
    } else {
        None
    }
}

/// Parses a `slack_search_public_and_private` result. Only DM and group DM hits are kept.
pub fn parse_search(text: &str) -> Result<SearchPage, ClaudeError> {
    let value = json_fields(text)?;
    let body = value
        .get("results")
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeError::Parse("search result has no results field".into()))?;
    let mut page = SearchPage {
        next_cursor: value
            .get("pagination_info")
            .and_then(Value::as_str)
            .and_then(cursor_from),
        ..SearchPage::default()
    };
    for block in body.split("### Result ").skip(1) {
        let mut channel: Option<(String, Kind)> = None;
        let mut participants = Vec::new();
        let mut ts = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("Channel:") {
                if let (Some(kind), Some(id)) = (kind_of(rest), id_in_parens(rest)) {
                    channel = Some((id, kind));
                }
            } else if let Some(rest) = line.strip_prefix("Participants:") {
                participants = people(rest);
            } else if let Some(rest) = line.strip_prefix("Message_ts:") {
                ts = Some(rest.trim().to_string());
            } else if line.starts_with("Text:") {
                break;
            }
        }
        if let (Some((channel_id, kind)), Some(ts)) = (channel, ts) {
            page.hits.push(SearchHit {
                channel_id,
                kind,
                participants,
                ts,
            });
        }
    }
    Ok(page)
}

fn is_metadata(line: &str) -> bool {
    ["Thread:", "Reactions:", "Files:"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn thread_marker(line: &str) -> Option<ThreadMarker> {
    // `Thread: 2 replies (latest: 2026-11-10 13:00:57 -03)`
    let rest = line.strip_prefix("Thread:")?.trim();
    let replies = rest.split_whitespace().next()?.parse().ok()?;
    let latest = rest
        .split_once("latest:")
        .map(|(_, tail)| tail.trim().trim_end_matches(')').trim().to_string())
        .unwrap_or_default();
    Some(ThreadMarker { replies, latest })
}

/// Splits a block's body into text and trailing metadata lines.
fn body_and_marker(lines: &[&str]) -> (String, Option<ThreadMarker>) {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    let mut marker = None;
    while end > 0 && is_metadata(lines[end - 1]) {
        if marker.is_none() {
            marker = thread_marker(lines[end - 1]);
        }
        end -= 1;
    }
    (lines[..end].join("\n").trim().to_string(), marker)
}

fn sender_from_header(header: &str) -> (String, String) {
    // `name <email> (U123)`; the name may be missing for bots.
    let id = id_in_parens(header).unwrap_or_default();
    let name = header
        .split(" <")
        .next()
        .unwrap_or(header)
        .split(" (")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (id, name)
}

/// Parses a `slack_read_channel` result (newest first).
pub fn parse_channel(text: &str) -> Result<ChannelPage, ClaudeError> {
    let value = json_fields(text)?;
    let body = value
        .get("messages")
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeError::Parse("channel result has no messages field".into()))?;
    let more = value
        .get("pagination_info")
        .and_then(Value::as_str)
        .filter(|info| !info.contains("no more"))
        .and_then(cursor_from);
    let mut page = ChannelPage {
        kind: body
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("Channel:"))
            .and_then(kind_of),
        next_cursor: more,
        ..ChannelPage::default()
    };
    for chunk in body.split("=== Message from ").skip(1) {
        let lines: Vec<&str> = chunk.lines().collect();
        let Some(first) = lines.first() else { continue };
        let header = first.trim().trim_end_matches("===").trim();
        let header = header.rsplit_once(" at ").map_or(header, |(who, _)| who);
        let (user_id, name) = sender_from_header(header);
        let Some(ts_at) = lines
            .iter()
            .position(|line| line.starts_with("Message TS:"))
        else {
            continue;
        };
        let ts = lines[ts_at]["Message TS:".len()..].trim().to_string();
        let (text, thread) = body_and_marker(&lines[ts_at + 1..]);
        page.messages.push(ChatMessage {
            ts,
            user_id,
            name,
            text,
            thread,
        });
    }
    Ok(page)
}

/// Parses a `slack_read_thread` result into the replies (the parent is skipped).
pub fn parse_thread(text: &str) -> Result<Vec<ChatMessage>, ClaudeError> {
    let value = json_fields(text)?;
    let body = value
        .get("messages")
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeError::Parse("thread result has no messages field".into()))?;
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|line| line.starts_with("=== THREAD") && !line.contains("PARENT"))
    else {
        return Ok(Vec::new());
    };
    let is_start = |i: usize| {
        lines[i].starts_with("From:")
            && lines.get(i + 1).is_some_and(|l| l.starts_with("Time:"))
            && lines
                .get(i + 2)
                .is_some_and(|l| l.starts_with("Message TS:"))
    };
    let starts: Vec<usize> = (start + 1..lines.len()).filter(|&i| is_start(i)).collect();
    let mut out = Vec::new();
    for (n, &at) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(lines.len());
        let (user_id, name) = sender_from_header(lines[at]["From:".len()..].trim());
        let ts = lines[at + 2]["Message TS:".len()..].trim().to_string();
        let mut tail: Vec<&str> = lines[at + 3..end].to_vec();
        while tail
            .last()
            .is_some_and(|l| l.trim().is_empty() || l.trim() == "---")
        {
            tail.pop();
        }
        let (text, _) = body_and_marker(&tail);
        out.push(ChatMessage {
            ts,
            user_id,
            name,
            text,
            thread: None,
        });
    }
    Ok(out)
}

/// `User ID: U037…` from `slack_read_user_profile` with no arguments (the account owner).
pub fn parse_self_id(text: &str) -> Result<String, ClaudeError> {
    let value = json_fields(text)?;
    value
        .get("result")
        .and_then(Value::as_str)
        .and_then(|body| {
            body.lines()
                .find_map(|line| line.strip_prefix("User ID:"))
                .map(|id| id.trim().to_string())
        })
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ClaudeError::Parse("profile has no user id".into()))
}

// ---------------------------------------------------------------------------
// Persisted state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMeta {
    pub kind: Kind,
    pub participants: Vec<Person>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub at: String,
    /// Slack tool calls executed.
    pub calls: u32,
    /// `claude` invocations (retries included).
    pub invocations: u32,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub messages: usize,
    pub channels: usize,
    pub error: Option<String>,
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(store: &Store, key: &str) -> T {
    store
        .get_setting(key)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_json<T: Serialize>(store: &Store, key: &str, value: &T) -> Result<(), ClaudeError> {
    let raw =
        serde_json::to_string(value).map_err(|error| ClaudeError::Store(error.to_string()))?;
    store.set_setting(key, &raw)?;
    Ok(())
}

pub fn load_runs(store: &Store) -> Vec<RunRecord> {
    load_json(store, KEY_RUNS)
}

fn push_run(store: &Store, record: RunRecord) {
    let mut runs = load_runs(store);
    runs.push(record);
    if runs.len() > MAX_RUNS_KEPT {
        let drop = runs.len() - MAX_RUNS_KEPT;
        runs.drain(..drop);
    }
    let _ = save_json(store, KEY_RUNS, &runs);
}

/// Cost of the runs whose UTC date equals the date prefix of `now_rfc3339`.
pub fn cost_today(runs: &[RunRecord], now_rfc3339: &str) -> f64 {
    let day = now_rfc3339.get(..10).unwrap_or_default();
    runs.iter()
        .filter(|run| run.at.starts_with(day))
        .map(|run| run.cost_usd)
        .sum()
}

pub fn disallow_list(store: &Store) -> Vec<String> {
    load_json(store, KEY_DISALLOW)
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    pub channels: usize,
    pub calls: u32,
    pub invocations: u32,
    pub cost_usd: f64,
    pub messages: usize,
    /// Changed episode ids with whether they came from a first backfill.
    pub episodes: Vec<(String, bool)>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

struct Session<'a> {
    store: &'a Store,
    runner: &'a dyn Runner,
    out: SyncOutcome,
}

impl Session<'_> {
    /// Runs `calls` in batches. A call the model skipped is retried; after `ATTEMPTS` it stays
    /// `None`. A rejected run or a down connector aborts everything.
    fn execute(&mut self, calls: &[ToolCall]) -> Result<Vec<Option<ToolOutput>>, ClaudeError> {
        let mut results: Vec<Option<ToolOutput>> = vec![None; calls.len()];
        for chunk_start in (0..calls.len()).step_by(BATCH) {
            let chunk_end = (chunk_start + BATCH).min(calls.len());
            for _ in 0..ATTEMPTS {
                let pending: Vec<usize> = (chunk_start..chunk_end)
                    .filter(|&i| results[i].is_none())
                    .collect();
                if pending.is_empty() {
                    break;
                }
                let batch: Vec<ToolCall> = pending.iter().map(|&i| calls[i].clone()).collect();
                let request = RunRequest {
                    prompt: build_prompt(&batch),
                    max_turns: 4,
                    disallow: disallow_list(self.store),
                    calls: batch.clone(),
                };
                let stdout = self.runner.run(&request)?;
                self.out.invocations += 1;
                let data = parse_stream(&stdout, &batch)?;
                self.out.cost_usd += data.cost_usd;
                let mut learned: BTreeSet<String> = disallow_list(self.store).into_iter().collect();
                learned.extend(learn_disallow(&data.init_tools));
                let learned: Vec<String> = learned.into_iter().collect();
                save_json(self.store, KEY_DISALLOW, &learned)?;
                for (slot, output) in pending.iter().zip(data.results) {
                    if output.is_some() {
                        self.out.calls += 1;
                        results[*slot] = output;
                    }
                }
            }
        }
        Ok(results)
    }

    fn execute_one(&mut self, call: ToolCall) -> Result<String, ClaudeError> {
        match self.execute(&[call])?.remove(0) {
            Some(output) if !output.is_error => Ok(output.text),
            Some(output) => Err(ClaudeError::Tool(output.text.chars().take(160).collect())),
            None => Err(ClaudeError::Cli(
                "the model did not run the requested call".into(),
            )),
        }
    }
}

fn ts_secs(ts: &str) -> i64 {
    ts.split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn ts_after(a: &str, b: &str) -> bool {
    let key = |ts: &str| {
        let (s, f) = ts.split_once('.').unwrap_or((ts, ""));
        (s.parse::<u64>().unwrap_or(0), format!("{f:0<9}"))
    };
    key(a) > key(b)
}

fn group_name(participants: &[Person], self_id: &str) -> String {
    let others: Vec<&str> = participants
        .iter()
        .filter(|person| person.id != self_id)
        .map(|person| person.name.as_str())
        .collect();
    let shown = others
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if others.len() > 3 {
        format!("Group: {shown} +{}", others.len() - 3)
    } else {
        format!("Group: {shown}")
    }
}

/// Who the conversation is with, as the store models it: the peer's user id for a 1:1, the
/// channel id for a group (so every message from another member is "in").
fn peer_for(
    channel: &str,
    meta: &ChannelMeta,
    self_id: &str,
    messages: &[ChatMessage],
) -> Option<(String, String)> {
    match meta.kind {
        Kind::Group => Some((channel.to_string(), group_name(&meta.participants, self_id))),
        Kind::Dm => {
            let from_meta = meta.participants.iter().find(|p| p.id != self_id);
            let (id, name) = match from_meta {
                Some(person) => (person.id.clone(), person.name.clone()),
                None => {
                    let sender = messages
                        .iter()
                        .find(|m| m.user_id != self_id && !m.user_id.is_empty())?;
                    (sender.user_id.clone(), sender.name.clone())
                }
            };
            Some((id, name))
        }
    }
}

fn to_raw(
    channel: &str,
    message: &ChatMessage,
    kind: Kind,
    peer_id: &str,
    self_id: &str,
    thread_ts: Option<&str>,
    historical: bool,
) -> RawSlackMessage {
    let mine = message.user_id == self_id;
    let (user_id, text) = if mine {
        (self_id.to_string(), message.text.clone())
    } else if kind == Kind::Group {
        (
            peer_id.to_string(),
            format!("{}: {}", message.name, message.text),
        )
    } else {
        (message.user_id.clone(), message.text.clone())
    };
    RawSlackMessage {
        channel_id: channel.to_string(),
        ts: message.ts.clone(),
        thread_ts: thread_ts.map(str::to_string),
        user_id: Some(user_id),
        text,
        permalink: None,
        edited_at: None,
        deleted: false,
        historical,
    }
}

/// One full cycle: discover, fetch, read threads, ingest. Never panics on bad output; the
/// failure goes to `SyncOutcome::error` and the cursors stay where they were.
pub fn sync(store: &Store, runner: &dyn Runner, now_secs: i64) -> SyncOutcome {
    let started = Instant::now();
    let mut session = Session {
        store,
        runner,
        out: SyncOutcome::default(),
    };
    let result = run_cycle(&mut session, now_secs);
    let mut out = session.out;
    if let Err(error) = result {
        out.error = Some(error.to_string());
    }
    out.duration_ms = started.elapsed().as_millis() as u64;
    push_run(
        store,
        RunRecord {
            at: model::now(),
            calls: out.calls,
            invocations: out.invocations,
            cost_usd: out.cost_usd,
            duration_ms: out.duration_ms,
            messages: out.messages,
            channels: out.channels,
            error: out.error.clone(),
        },
    );
    out
}

fn run_cycle(session: &mut Session<'_>, now_secs: i64) -> Result<(), ClaudeError> {
    let store = session.store;
    let self_id = match store.get_setting(KEY_SELF)?.filter(|id| !id.is_empty()) {
        Some(id) => id,
        None => {
            let id = parse_self_id(&session.execute_one(ToolCall::user_profile())?)?;
            store.set_setting(KEY_SELF, &id)?;
            id
        }
    };
    let watermark = store
        .get_setting(KEY_WATERMARK)?
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(now_secs - FIRST_WINDOW_DAYS * 86_400);
    // The search `after` is inclusive; a day of slack covers clock skew and the 10-minute gap.
    let after = watermark - 86_400;

    // 1. Discover.
    let mut metas: BTreeMap<String, ChannelMeta> = load_json(store, KEY_CHANNELS);
    let mut earliest_hit: BTreeMap<String, i64> = BTreeMap::new();
    let mut cursor: Option<String> = None;
    for _ in 0..DISCOVERY_PAGES {
        let text = session.execute_one(ToolCall::search(&self_id, after, cursor.as_deref()))?;
        let page = parse_search(&text)?;
        for hit in page.hits {
            let secs = ts_secs(&hit.ts);
            let entry = earliest_hit.entry(hit.channel_id.clone()).or_insert(secs);
            *entry = (*entry).min(secs);
            let meta = metas.entry(hit.channel_id).or_insert(ChannelMeta {
                kind: hit.kind,
                participants: Vec::new(),
            });
            meta.kind = hit.kind;
            if !hit.participants.is_empty() {
                meta.participants = hit.participants;
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    // 2. Plan the fetch: discovered channels plus tracked channels with an open episode.
    let mut channels: BTreeSet<String> = earliest_hit.keys().cloned().collect();
    for state in ["open", "waiting"] {
        for summary in store.list_slack_episodes(Some(state), 200)? {
            if metas.contains_key(&summary.episode.channel_id) {
                channels.insert(summary.episode.channel_id);
            }
        }
    }
    session.out.channels = channels.len();
    let mut pages: BTreeMap<String, Vec<ChatMessage>> = BTreeMap::new();
    let mut historical: BTreeMap<String, bool> = BTreeMap::new();
    let mut plan: Vec<ToolCall> = Vec::new();
    for channel in &channels {
        let stored = store.slack_channel_cursor(channel)?;
        historical.insert(channel.clone(), stored.is_none());
        let oldest = stored.unwrap_or_else(|| {
            let from = earliest_hit
                .get(channel)
                .map_or(watermark, |hit| watermark.min(hit - 3_600));
            format!("{from}.000000")
        });
        plan.push(ToolCall::read_channel(channel, &oldest, None));
    }

    // 3. Fetch, following pagination while a page is full.
    let mut failures: Vec<String> = Vec::new();
    for _ in 0..CHANNEL_PAGES {
        if plan.is_empty() {
            break;
        }
        let results = session.execute(&plan)?;
        let mut next_plan = Vec::new();
        for (call, output) in plan.iter().zip(results) {
            let channel = call.input["channel_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let oldest = call.input["oldest"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let parsed = match output {
                Some(output) if !output.is_error => parse_channel(&output.text),
                Some(output) => Err(ClaudeError::Tool(output.text.chars().take(120).collect())),
                None => Err(ClaudeError::Cli("the model skipped a read".into())),
            };
            match parsed {
                Ok(page) => {
                    if let Some(kind) = page.kind
                        && let Some(meta) = metas.get_mut(&channel)
                    {
                        meta.kind = kind;
                    }
                    let full = page.messages.len() as i64 >= CHANNEL_LIMIT;
                    pages
                        .entry(channel.clone())
                        .or_default()
                        .extend(page.messages);
                    if full && let Some(cursor) = page.next_cursor {
                        next_plan.push(ToolCall::read_channel(&channel, &oldest, Some(&cursor)));
                    }
                }
                Err(error) => failures.push(format!("{channel}: {error}")),
            }
        }
        plan = next_plan;
    }

    // 4. Threads whose marker changed since the last look.
    let mut thread_plan: Vec<(ToolCall, String, String)> = Vec::new();
    for (channel, messages) in &pages {
        for message in messages {
            let Some(marker) = &message.thread else {
                continue;
            };
            let scope = format!("{channel}:{}", message.ts);
            let seen = store
                .get_sync_cursor(THREAD_CONNECTOR, &scope)?
                .and_then(|c| c.cursor);
            if seen.as_deref() != Some(marker.signature().as_str())
                && thread_plan.len() < THREADS_PER_RUN
            {
                thread_plan.push((
                    ToolCall::read_thread(channel, &message.ts),
                    scope,
                    marker.signature(),
                ));
            }
        }
    }
    let thread_calls: Vec<ToolCall> = thread_plan
        .iter()
        .map(|(call, _, _)| call.clone())
        .collect();
    let thread_results = if thread_calls.is_empty() {
        Vec::new()
    } else {
        session.execute(&thread_calls)?
    };

    // 5. Ingest oldest first, so episode segmentation sees time in order.
    struct Item {
        message: ChatMessage,
        thread_ts: Option<String>,
    }
    let mut per_channel: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    for (channel, messages) in &pages {
        let cursor = store.slack_channel_cursor(channel)?;
        for message in messages {
            // The boundary message is returned again; replays only churn episodes.
            if cursor.as_deref().is_some_and(|c| !ts_after(&message.ts, c)) {
                continue;
            }
            per_channel.entry(channel.clone()).or_default().push(Item {
                message: message.clone(),
                thread_ts: None,
            });
        }
    }
    let mut thread_marks: Vec<(String, String)> = Vec::new();
    for ((call, scope, signature), output) in thread_plan.iter().zip(thread_results) {
        let channel = call.input["channel_id"].as_str().unwrap_or_default();
        let parent = call.input["message_ts"].as_str().unwrap_or_default();
        let Some(output) = output.filter(|o| !o.is_error) else {
            failures.push(format!("{scope}: thread not read"));
            continue;
        };
        match parse_thread(&output.text) {
            Ok(replies) => {
                for reply in replies {
                    per_channel
                        .entry(channel.to_string())
                        .or_default()
                        .push(Item {
                            message: reply,
                            thread_ts: Some(parent.to_string()),
                        });
                }
                thread_marks.push((scope.clone(), signature.clone()));
            }
            Err(error) => failures.push(format!("{scope}: {error}")),
        }
    }
    for (channel, mut items) in per_channel {
        let Some(meta) = metas.get(&channel) else {
            continue;
        };
        let all: Vec<ChatMessage> = items.iter().map(|item| item.message.clone()).collect();
        let Some((peer_id, peer_name)) = peer_for(&channel, meta, &self_id, &all) else {
            continue;
        };
        items.sort_by(|a, b| {
            (ts_secs(&a.message.ts), &a.message.ts).cmp(&(ts_secs(&b.message.ts), &b.message.ts))
        });
        let first_time = historical.get(&channel).copied().unwrap_or(false);
        for item in items {
            let raw = to_raw(
                &channel,
                &item.message,
                meta.kind,
                &peer_id,
                &self_id,
                item.thread_ts.as_deref(),
                first_time,
            );
            if let Some(stored) =
                store.capture_slack_message(&peer_id, Some(&peer_name), &raw, 90)?
            {
                session.out.messages += 1;
                if !session
                    .out
                    .episodes
                    .iter()
                    .any(|(id, _)| id == &stored.episode_id)
                {
                    session.out.episodes.push((stored.episode_id, first_time));
                }
            }
        }
    }
    for (scope, signature) in thread_marks {
        store.upsert_sync_cursor(THREAD_CONNECTOR, &scope, Some(&signature), None)?;
    }
    save_json(store, KEY_CHANNELS, &metas)?;

    if failures.is_empty() {
        store.set_setting(KEY_WATERMARK, &now_secs.to_string())?;
        Ok(())
    } else {
        Err(ClaudeError::Cli(failures.join("; ")))
    }
}

// ---------------------------------------------------------------------------
// Tests (synthetic fixtures only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SELF: &str = "U0SELF0001";

    fn search_text(blocks: &[(&str, &str, &str, &str)], more: Option<&str>) -> String {
        let mut body = String::from("# Search Results for: \n\n## Messages\n\n");
        for (n, (label, id, ts, parts)) in blocks.iter().enumerate() {
            body.push_str(&format!(
                "### Result {} of {}\nChannel: {label} (ID: {id})\nParticipants: {parts}\nFrom: Sam Self <sam@example.test> (ID: {SELF})\nTime: 2026-01-02 10:00:00 -03\nMessage_ts: {ts}\nPermalink: [link](https://example.slack.test/archives/{id}/p1)\nText: \nhello\n\n---\n\n",
                n + 1,
                blocks.len()
            ));
        }
        let info = match more {
            Some(cursor) => format!("For the next page of results use cursor `{cursor}`\n"),
            None => "End of results - No more pages available.\n".to_string(),
        };
        json!({ "results": body, "pagination_info": info }).to_string()
    }

    fn channel_text(
        label: &str,
        id: &str,
        messages: &[(&str, &str, &str, &str, &str)],
        more: Option<&str>,
    ) -> String {
        let mut body = format!("Channel: {label} ({id})\n\n");
        for (name, uid, ts, text, extra) in messages {
            body.push_str(&format!(
                "=== Message from {name} <{name}@example.test> ({uid}) at 2026-01-02 10:00:00 -03 === \nMessage TS: {ts}\n{text}\n{extra}\n\n"
            ));
        }
        let info = match more {
            Some(cursor) => format!(
                "There are more messages available. To view the next page, use cursor: `{cursor}`\n"
            ),
            None => "There are no more messages available.\n".to_string(),
        };
        json!({ "messages": body, "pagination_info": info }).to_string()
    }

    fn thread_text(parent_ts: &str, replies: &[(&str, &str, &str)]) -> String {
        let mut body = format!(
            "=== THREAD PARENT MESSAGE ===\nFrom: Pat Peer <pat@example.test> (U0PEER0001)\nTime: 2026-01-02 10:00:00 -03\nMessage TS: {parent_ts}\nparent text\n\n=== THREAD REPLIES ({} total) ===\n\n",
            replies.len()
        );
        for (uid, ts, text) in replies {
            body.push_str(&format!(
                "---\nFrom: Someone <s@example.test> ({uid})\nTime: 2026-01-02 11:00:00 -03\nMessage TS: {ts}\n{text}\n\n"
            ));
        }
        json!({ "messages": body, "pagination_info": "There are no more messages in this thread.\n" }).to_string()
    }

    type Answer = Box<dyn Fn(&ToolCall) -> Option<String>>;

    /// A fake claude: answers each requested call from `answer`, with a plausible stream.
    struct Fake {
        answer: Answer,
        requests: RefCell<Vec<RunRequest>>,
        skip_first: RefCell<bool>,
        connector: &'static str,
    }

    impl Fake {
        fn new(answer: impl Fn(&ToolCall) -> Option<String> + 'static) -> Self {
            Self {
                answer: Box::new(answer),
                requests: RefCell::new(Vec::new()),
                skip_first: RefCell::new(false),
                connector: "connected",
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, request: &RunRequest) -> Result<String, ClaudeError> {
            self.requests.borrow_mut().push(request.clone());
            let mut lines = vec![json!({
                "type": "system", "subtype": "init",
                "tools": [
                    Tool::Search.full_name(), Tool::ReadChannel.full_name(),
                    Tool::ReadThread.full_name(), Tool::UserProfile.full_name(),
                    "mcp__claude_ai_Slack__slack_send_message", "mcp__local-mcp__file_read", "Bash"
                ],
                "mcp_servers": [{"name": SERVER_NAME, "status": self.connector}]
            })
            .to_string()];
            let skip = self.skip_first.replace(false);
            for (i, call) in request.calls.iter().enumerate() {
                if skip && i == 0 {
                    continue;
                }
                let id = format!("toolu_{i}");
                lines.push(
                    json!({"type":"assistant","message":{"content":[
                    {"type":"tool_use","id":id,"name":call.tool.full_name(),"input":call.input}]}})
                    .to_string(),
                );
                if let Some(text) = (self.answer)(call) {
                    lines.push(json!({"type":"user","message":{"content":[
                        {"type":"tool_result","tool_use_id":id,"content":[{"type":"text","text":text}]}]}}).to_string());
                }
            }
            lines.push(
                json!({"type":"result","total_cost_usd":0.003,"num_turns":2,"is_error":false})
                    .to_string(),
            );
            Ok(lines.join("\n"))
        }
    }

    #[test]
    fn stream_matches_results_by_id_even_out_of_order() {
        let calls = vec![
            ToolCall::read_channel("D1", "1.0", None),
            ToolCall::read_channel("D2", "1.0", None),
        ];
        let stdout = [
            json!({"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":SERVER_NAME,"status":"connected"}]}),
            json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":Tool::ReadChannel.full_name(),"input":calls[0].input},
                {"type":"tool_use","id":"b","name":Tool::ReadChannel.full_name(),"input":calls[1].input}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","content":"second"}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":[{"type":"text","text":"first"}]}]}}),
            json!({"type":"result","total_cost_usd":0.5,"num_turns":2,"is_error":false}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        let data = parse_stream(&stdout, &calls).unwrap();
        assert_eq!(data.results[0].as_ref().unwrap().text, "first");
        assert_eq!(data.results[1].as_ref().unwrap().text, "second");
        assert_eq!(data.cost_usd, 0.5);
    }

    #[test]
    fn stream_accepts_a_retry_that_drops_an_empty_argument() {
        let calls = vec![ToolCall::user_profile()];
        let stdout = [
            json!({"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":SERVER_NAME,"status":"connected"}]}),
            json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":Tool::UserProfile.full_name(),"input":{"user_id":""}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":"initialization_failed: bad id"}]}}),
            json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"b","name":Tool::UserProfile.full_name(),"input":{}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","content":"{\"result\":\"User ID: U1\"}"}]}}),
        ]
        .map(|v| v.to_string())
        .join("
");
        let data = parse_stream(&stdout, &calls).unwrap();
        assert!(data.results[0].as_ref().unwrap().text.contains("U1"));
    }

    #[test]
    fn stream_rejects_a_tool_outside_the_allowlist() {
        let calls = vec![ToolCall::user_profile()];
        let stdout = [
            json!({"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":SERVER_NAME,"status":"connected"}]}),
            json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":"mcp__claude_ai_Slack__slack_send_message","input":{}}]}}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        assert!(matches!(
            parse_stream(&stdout, &calls),
            Err(ClaudeError::Rejected(_))
        ));
    }

    #[test]
    fn stream_rejects_arguments_relay_did_not_ask_for() {
        let calls = vec![ToolCall::read_channel("D1", "1.0", None)];
        let stdout = [
            json!({"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":SERVER_NAME,"status":"connected"}]}),
            json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":Tool::ReadChannel.full_name(),"input":{"channel_id":"D9","oldest":"1.0","limit":100}}]}}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        assert!(matches!(
            parse_stream(&stdout, &calls),
            Err(ClaudeError::Rejected(_))
        ));
    }

    #[test]
    fn stream_reports_a_disconnected_connector() {
        let stdout = json!({"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":SERVER_NAME,"status":"needs-auth"}]}).to_string();
        assert_eq!(
            parse_stream(&stdout, &[]).unwrap_err(),
            ClaudeError::ConnectorDown("needs-auth".into())
        );
    }

    #[test]
    fn disallow_collapses_servers_but_keeps_allowed_slack_tools() {
        let tools: Vec<String> = [
            "Bash",
            "mcp__local-mcp__file_read",
            "mcp__local-mcp__file_write",
            "mcp__plugin_x_y__z",
            "mcp__claude_ai_Slack__slack_send_message",
            "mcp__claude_ai_Slack__slack_read_channel",
        ]
        .map(String::from)
        .to_vec();
        let learned = learn_disallow(&tools);
        assert!(learned.contains("Bash"));
        assert!(learned.contains("mcp__local-mcp"));
        assert!(learned.contains("mcp__plugin_x_y"));
        assert!(learned.contains("mcp__claude_ai_Slack__slack_send_message"));
        assert!(!learned.contains("mcp__claude_ai_Slack__slack_read_channel"));
    }

    #[test]
    fn cli_args_carry_the_allowlist_and_learned_disallow() {
        let request = RunRequest {
            calls: vec![],
            prompt: "p".into(),
            max_turns: 3,
            disallow: vec!["mcp__local-mcp".into(), "Bash".into()],
        };
        let args = cli_args(&request, &run_settings_json(&["a@b".to_string()]));
        let at = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        assert_eq!(at("--model").as_deref(), Some("haiku"));
        assert_eq!(
            at("--disallowedTools").as_deref(),
            Some("mcp__local-mcp Bash")
        );
        assert!(at("--allowedTools").unwrap().contains("slack_read_channel"));
        assert!(!args.iter().any(|a| a.contains("dangerously")));
        let settings: Value = serde_json::from_str(&at("--settings").unwrap()).unwrap();
        assert_eq!(settings["disableAllHooks"], true);
        assert_eq!(settings["enabledPlugins"]["a@b"], false);
    }

    #[test]
    fn search_parser_reads_dm_and_group_hits_and_cursor() {
        let text = search_text(
            &[
                (
                    "Group DM",
                    "C0GROUP001",
                    "1790000200.000100",
                    "Pat Peer (ID: U0PEER0001), Quinn Q (ID: U0QUINN001), Sam Self (ID: U0SELF0001)",
                ),
                (
                    "DM",
                    "D0DIRECT01",
                    "1790000100.000200",
                    "Pat Peer (ID: U0PEER0001)",
                ),
                ("Private channel", "C0OTHER", "1790000050.000000", ""),
            ],
            Some("Q1VS"),
        );
        let page = parse_search(&text).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("Q1VS"));
        assert_eq!(page.hits.len(), 2);
        assert_eq!(page.hits[0].kind, Kind::Group);
        assert_eq!(page.hits[0].participants.len(), 3);
        assert_eq!(page.hits[0].participants[1].name, "Quinn Q");
        assert_eq!(page.hits[1].channel_id, "D0DIRECT01");
        assert!(
            parse_search(&search_text(&[], None))
                .unwrap()
                .next_cursor
                .is_none()
        );
    }

    #[test]
    fn channel_parser_reads_messages_threads_and_strips_metadata() {
        let text = channel_text(
            "Group DM",
            "C0GROUP001",
            &[
                (
                    "pat",
                    "U0PEER0001",
                    "1790000300.000100",
                    "line one\nline two",
                    "Thread: 2 replies (latest: 2026-01-02 13:00:57 -03)\nReactions: +1 (1)",
                ),
                ("sam", SELF, "1790000200.000100", "from: me", ""),
            ],
            Some("bmV4dA=="),
        );
        let page = parse_channel(&text).unwrap();
        assert_eq!(page.kind, Some(Kind::Group));
        assert_eq!(page.next_cursor.as_deref(), Some("bmV4dA=="));
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].text, "line one\nline two");
        assert_eq!(page.messages[0].user_id, "U0PEER0001");
        let marker = page.messages[0].thread.as_ref().unwrap();
        assert_eq!(
            (marker.replies, marker.latest.as_str()),
            (2, "2026-01-02 13:00:57 -03")
        );
        assert_eq!(page.messages[1].text, "from: me");
        assert!(page.messages[1].thread.is_none());
        let last = channel_text("DM", "D1", &[], None);
        assert!(parse_channel(&last).unwrap().next_cursor.is_none());
    }

    #[test]
    fn thread_parser_returns_replies_only() {
        let text = thread_text(
            "1790000300.000100",
            &[
                ("U0PEER0001", "1790000400.000100", "a reply\nmore"),
                (SELF, "1790000500.000100", "mine"),
            ],
        );
        let replies = parse_thread(&text).unwrap();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].text, "a reply\nmore");
        assert_eq!(replies[1].user_id, SELF);
        assert_eq!(replies[1].ts, "1790000500.000100");
    }

    #[test]
    fn self_id_comes_from_the_profile() {
        let text = json!({"result": format!("User ID: {SELF}\nUsername: sam\n")}).to_string();
        assert_eq!(parse_self_id(&text).unwrap(), SELF);
        assert!(parse_self_id("initialization_failed: nope").is_err());
    }

    #[test]
    fn prompt_names_every_call_with_its_exact_arguments() {
        let calls = vec![
            ToolCall::read_channel("D1", "10.000000", None),
            ToolCall::read_thread("D1", "10.000100"),
        ];
        let prompt = build_prompt(&calls);
        assert!(prompt.starts_with("Make these 2 calls in parallel, in a single turn"));
        assert!(prompt.contains(&format!(
            "{} with {}",
            Tool::ReadThread.full_name(),
            calls[1].input
        )));
        assert!(prompt.ends_with("the single word: done"));
    }

    #[test]
    fn discovery_args_come_from_the_watermark() {
        let call = ToolCall::search(SELF, 1_790_000_000, Some("CUR"));
        assert_eq!(call.input["filters"], format!("from:<@{SELF}>"));
        assert_eq!(call.input["after"], "1790000000");
        assert_eq!(call.input["channel_types"], "im,mpim");
        assert_eq!(call.input["cursor"], "CUR");
        assert_eq!(call.input["limit"], 20);
    }

    fn standard_fake() -> Fake {
        Fake::new(|call| match call.tool {
            Tool::UserProfile => Some(json!({"result": format!("User ID: {SELF}\n")}).to_string()),
            Tool::Search => Some(search_text(
                &[
                    (
                        "DM",
                        "D0DIRECT01",
                        "1790000100.000200",
                        "Pat Peer (ID: U0PEER0001)",
                    ),
                    (
                        "Group DM",
                        "C0GROUP001",
                        "1790000200.000100",
                        "Pat Peer (ID: U0PEER0001), Quinn Q (ID: U0QUINN001), Sam Self (ID: U0SELF0001)",
                    ),
                ],
                None,
            )),
            Tool::ReadChannel => match call.input["channel_id"].as_str().unwrap() {
                "D0DIRECT01" => Some(channel_text(
                    "DM",
                    "D0DIRECT01",
                    &[
                        ("sam", SELF, "1790000100.000200", "on it", ""),
                        (
                            "pat",
                            "U0PEER0001",
                            "1790000050.000100",
                            "can you check the build?",
                            "",
                        ),
                    ],
                    None,
                )),
                _ => Some(channel_text(
                    "Group DM",
                    "C0GROUP001",
                    &[
                        ("quinn", "U0QUINN001", "1790000250.000100", "thanks", ""),
                        (
                            "pat",
                            "U0PEER0001",
                            "1790000200.000100",
                            "release is out",
                            "Thread: 1 replies (latest: 2026-01-02 11:00:00 -03)",
                        ),
                    ],
                    None,
                )),
            },
            Tool::ReadThread => Some(thread_text(
                "1790000200.000100",
                &[(SELF, "1790000260.000100", "nice")],
            )),
        })
    }

    #[test]
    fn sync_ingests_dms_groups_and_threads_idempotently() {
        let store = Store::open_in_memory().unwrap();
        let fake = standard_fake();
        let out = sync(&store, &fake, 1_790_001_000);
        assert_eq!(out.error, None);
        assert_eq!(out.channels, 2);
        assert_eq!(out.messages, 5);
        assert!((out.cost_usd - 0.003 * out.invocations as f64).abs() < 1e-9);
        // Self id probe, one search, one batch of two reads, one thread read.
        assert_eq!(out.invocations, 4);

        // The learned disallow list reaches the next run's request.
        assert!(disallow_list(&store).contains(&"mcp__local-mcp".to_string()));
        assert!(
            fake.requests
                .borrow()
                .last()
                .unwrap()
                .disallow
                .contains(&"Bash".to_string())
        );

        let episodes = store.list_slack_episodes(None, 50).unwrap();
        assert_eq!(episodes.len(), 2);
        let group = episodes
            .iter()
            .find(|e| e.episode.channel_id == "C0GROUP001")
            .unwrap();
        assert!(
            group
                .peer_name
                .as_deref()
                .unwrap()
                .starts_with("Group: Pat Peer, Quinn Q")
        );
        let detail = store.get_slack_episode(&group.episode.id).unwrap();
        let directions: Vec<_> = detail
            .messages
            .iter()
            .map(|m| (m.ts.as_str(), m.direction.as_str()))
            .collect();
        assert!(directions.contains(&("1790000250.000100", "in")));
        assert!(directions.contains(&("1790000260.000100", "out")));
        assert!(
            detail
                .messages
                .iter()
                .any(|m| m.text == "Quinn Q: thanks" || m.text.ends_with(": thanks"))
        );
        let dm = episodes
            .iter()
            .find(|e| e.episode.channel_id == "D0DIRECT01")
            .unwrap();
        assert_eq!(dm.peer_user_id, "U0PEER0001");

        // A second cycle changes nothing: same cursors, same thread signature.
        let again = sync(&store, &fake, 1_790_001_600);
        assert_eq!(again.error, None);
        assert_eq!(again.messages, 0);
        assert!(again.episodes.is_empty());
        assert_eq!(store.list_slack_episodes(None, 50).unwrap().len(), 2);
        assert_eq!(load_runs(&store).len(), 2);
    }

    #[test]
    fn sync_plans_fetch_arguments_from_cursors() {
        let store = Store::open_in_memory().unwrap();
        let fake = standard_fake();
        sync(&store, &fake, 1_790_001_000);
        let first_read = fake
            .requests
            .borrow()
            .iter()
            .flat_map(|r| r.calls.clone())
            .find(|c| c.tool == Tool::ReadChannel)
            .unwrap();
        // No cursor yet: the oldest is the first-run window (3 days before now).
        assert_eq!(
            first_read.input["oldest"],
            format!("{}.000000", 1_790_001_000 - 3 * 86_400)
        );
        fake.requests.borrow_mut().clear();
        sync(&store, &fake, 1_790_001_600);
        let reads: Vec<_> = fake
            .requests
            .borrow()
            .iter()
            .flat_map(|r| r.calls.clone())
            .filter(|c| c.tool == Tool::ReadChannel)
            .collect();
        let direct = reads
            .iter()
            .find(|c| c.input["channel_id"] == "D0DIRECT01")
            .unwrap();
        assert_eq!(direct.input["oldest"], "1790000100.000200");
        let search = fake
            .requests
            .borrow()
            .iter()
            .flat_map(|r| r.calls.clone())
            .find(|c| c.tool == Tool::Search)
            .unwrap();
        assert_eq!(
            search.input["after"],
            (1_790_001_000i64 - 86_400).to_string()
        );
    }

    #[test]
    fn skipped_calls_are_retried() {
        let store = Store::open_in_memory().unwrap();
        let fake = standard_fake();
        *fake.skip_first.borrow_mut() = true;
        let out = sync(&store, &fake, 1_790_001_000);
        assert_eq!(out.error, None);
        assert_eq!(out.invocations, 5);
    }

    #[test]
    fn a_disconnected_connector_surfaces_as_the_run_error() {
        let store = Store::open_in_memory().unwrap();
        let mut fake = standard_fake();
        fake.connector = "needs-auth";
        let out = sync(&store, &fake, 1_790_001_000);
        assert!(out.error.unwrap().contains("not connected"));
        assert_eq!(out.messages, 0);
        assert_eq!(load_runs(&store).len(), 1);
    }

    #[test]
    fn cost_today_sums_runs_of_the_same_utc_day() {
        let run = |at: &str, cost: f64| RunRecord {
            at: at.into(),
            calls: 0,
            invocations: 0,
            cost_usd: cost,
            duration_ms: 0,
            messages: 0,
            channels: 0,
            error: None,
        };
        let runs = vec![
            run("2026-01-01T23:00:00Z", 1.0),
            run("2026-01-02T01:00:00Z", 0.25),
            run("2026-01-02T09:00:00Z", 0.5),
        ];
        assert!((cost_today(&runs, "2026-01-02T12:00:00Z") - 0.75).abs() < 1e-9);
    }
}
