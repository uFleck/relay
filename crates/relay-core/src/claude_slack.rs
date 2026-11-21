//! Claude Slack proxy: Slack 1:1 and group DMs without a Slack app token.
//!
//! Relay shells out to headless Claude Code (`claude -p`, haiku) that has the user's claude.ai
//! Slack connector. Relay computes every tool call's exact arguments; the model only executes
//! them, and Relay reads the raw tool results from the stream-json output. This module is pure
//! (no process, no Tauri): the `Runner` trait is the seam, so tests inject a fake stream.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub const SERVER_NAME: &str = "claude.ai Slack";
const PREFIX: &str = "mcp__claude_ai_Slack__";
pub const SYSTEM_PROMPT: &str =
    "You execute Slack MCP tool calls exactly as instructed and nothing else.";

pub const BATCH: usize = 8;
const CHANNEL_LIMIT: i64 = 100;

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

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Search,
    ReadChannel,
    ReadThread,
    UserProfile,
    /// #33: the conversation directory scan (`slack_list_user_channels`).
    ListChannels,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::Search,
        Tool::ReadChannel,
        Tool::ReadThread,
        Tool::UserProfile,
        Tool::ListChannels,
    ];

    pub fn full_name(self) -> String {
        let short = match self {
            Tool::Search => "slack_search_public_and_private",
            Tool::ReadChannel => "slack_read_channel",
            Tool::ReadThread => "slack_read_thread",
            Tool::UserProfile => "slack_read_user_profile",
            Tool::ListChannels => "slack_list_user_channels",
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

    /// #33: a fixed-window read. Twenty messages per page is the window page limit, distinct
    /// from the v1 sync's `CHANNEL_LIMIT` of 100.
    pub fn read_window(channel: &str, oldest: &str, cursor: Option<&str>) -> Self {
        let mut input = json!({
            "channel_id": channel,
            "oldest": oldest,
            "limit": 20,
        });
        if let Some(cursor) = cursor {
            input["cursor"] = json!(cursor);
        }
        Self {
            tool: Tool::ReadChannel,
            input,
        }
    }

    /// #33: one full directory page. The four fixed arguments never change between pages; only
    /// the cursor advances, and a short page still carries a cursor that must be followed.
    pub fn list_channels(cursor: Option<&str>) -> Self {
        let mut input = json!({
            "types": "public_channel,private_channel,im,mpim",
            "format": "full",
            "limit": 200,
        });
        if let Some(cursor) = cursor {
            input["cursor"] = json!(cursor);
        }
        Self {
            tool: Tool::ListChannels,
            input,
        }
    }

    /// #33: one detailed message-search call. `keywords` replaces v1's DM-only `filters` search.
    pub fn search_messages(query: &str) -> Self {
        Self {
            tool: Tool::Search,
            input: json!({
                "keywords": [query],
                "natural_language_query": "",
                "response_format": "detailed",
                "include_context": false,
                "limit": 10,
            }),
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
    /// Per-run deadline. On-demand thread reads use the 30-second deadline; directory scans
    /// fall back to the transport's default. `None` means "use the default".
    pub timeout: Option<Duration>,
    /// Narrows `--allowedTools` for one run. `None` keeps the full Slack tool allowlist.
    pub tools: Option<Vec<String>>,
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
    args.push(
        request
            .tools
            .clone()
            .unwrap_or_else(allowed_tools)
            .join(" "),
    );
    if !request.disallow.is_empty() {
        args.push("--disallowedTools".into());
        args.push(request.disallow.join(" "));
    }
    args
}

/// `--settings` JSON: no hooks, every user-enabled plugin switched off for this run, and only remote
/// (https) MCP servers. The user's local stdio servers (each a process tree) cost ~10 CPU-seconds per
/// run and the sync never calls them; the claude.ai Slack connector is https.
pub fn run_settings_json(enabled_plugins: &[String]) -> String {
    let plugins: serde_json::Map<String, Value> = enabled_plugins
        .iter()
        .map(|name| (name.clone(), Value::Bool(false)))
        .collect();
    json!({
        "disableAllHooks": true,
        "enabledPlugins": plugins,
        "allowedMcpServers": [{ "serverUrl": "https://*" }],
    })
    .to_string()
}

/// Env for each run. Without the local servers the CLI starts before the claude.ai connectors finish
/// connecting, and Slack shows `pending` at init; blocking startup waits for them.
pub const RUN_ENV: [(&str, &str); 1] = [("MCP_CONNECTION_NONBLOCKING", "false")];

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
    pub(crate) fn failed(&self) -> bool {
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
    /// Strict-call consumers use these to reject a run that made more than the one requested
    /// call, repeated a call, or reached a connector that is not connected.
    pub tool_use_count: usize,
    pub duplicate_tool_use: bool,
    pub connector_status: Option<String>,
}

pub(crate) fn content_text(content: &Value) -> String {
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
                data.connector_status = connector.clone();
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
                    if uses.contains_key(id) {
                        data.duplicate_tool_use = true;
                    }
                    uses.insert(id.to_string(), index);
                    data.tool_use_count += 1;
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
pub(crate) fn body_and_marker(lines: &[&str]) -> (String, Option<ThreadMarker>) {
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

pub(crate) fn sender_from_header(header: &str) -> (String, String) {
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
    fn cli_args_carry_the_allowlist_and_learned_disallow() {
        let request = RunRequest {
            calls: vec![],
            prompt: "p".into(),
            max_turns: 3,
            disallow: vec!["mcp__local-mcp".into(), "Bash".into()],
            timeout: None,
            tools: None,
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
        assert_eq!(
            settings["allowedMcpServers"],
            json!([{ "serverUrl": "https://*" }])
        );
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
}
