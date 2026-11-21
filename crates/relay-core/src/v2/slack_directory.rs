//! #33: metadata-only Slack conversation directory and explicit message search.
//!
//! All connector policy lives here: fixed tool arguments, cursor validation, the two-pages-per-
//! invocation budget, the seven-day freshness clock and the merge rules that keep discoveries
//! from permalinks and searches without ever advancing the full-scan clock. The Tauri adapter
//! only supplies a `Runner`, a store and the wall clock.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude_slack::{
    self, ClaudeError, RunRequest, Runner, SERVER_NAME, Tool, ToolCall, ToolOutput, build_prompt,
    parse_stream,
};

use super::store::V2Store;

/// One directory invocation gets 60 seconds; a window read gets the thread's 30 seconds.
pub const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(60);
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);
/// A single headless run follows at most two pages before Relay resumes from the last cursor.
pub const PAGES_PER_RUN: usize = 2;
/// Eight runs cover 1600 conversations; a scan still unfinished after that keeps the old directory.
pub const MAX_RUNS: usize = 8;
/// A fresh directory older than this is rescanned at startup.
pub const STALE_DAYS: i64 = 7;
const FIXED_TYPES: &str = "public_channel,private_channel,im,mpim";

/// The explicit kind from the connector's `Type` line. A `C` id cannot distinguish a group DM
/// from a channel, so `Unknown` is preserved until a full scan supplies the real kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConversationKind {
    #[default]
    Unknown,
    Public,
    Private,
    Dm,
    Group,
}

impl ConversationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "",
            Self::Public => "public",
            Self::Private => "private",
            Self::Dm => "dm",
            Self::Group => "group",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "public" => Self::Public,
            "private" => Self::Private,
            "dm" => Self::Dm,
            "group" => Self::Group,
            _ => Self::Unknown,
        }
    }

    /// Maps the connector's `Type` value. Never infer a group DM from a `C` prefix.
    fn from_connector(raw: &str) -> Self {
        match raw.trim() {
            "Public Channel" => Self::Public,
            "Private Channel" => Self::Private,
            "Direct Message" => Self::Dm,
            "Group DM" => Self::Group,
            _ => Self::Unknown,
        }
    }
}

/// One directory row. `name` may be empty for a permalink-only discovery; the id is then the
/// fallback label in the picker. `discovered` marks rows that did not come from a full scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryConversation {
    pub id: String,
    pub kind: ConversationKind,
    pub name: String,
    pub members: Vec<String>,
    pub archived: bool,
    pub discovered: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DirectoryPage {
    pub conversations: Vec<DirectoryConversation>,
    pub next_cursor: Option<String>,
}

/// The scan clock and resume point. A missing timestamp means a first scan is due.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DirectoryState {
    pub last_success_at: Option<String>,
    pub last_cursor: Option<String>,
}

impl DirectoryState {
    /// Missing, or older than `STALE_DAYS` UTC days, is due. Unparseable timestamps are due so
    /// a corrupt clock cannot silently pin the directory forever.
    pub fn due(&self, now: &str) -> bool {
        let Some(at) = &self.last_success_at else {
            return true;
        };
        let parsed = crate::model::parse_time(at);
        let current = crate::model::parse_time(now);
        match (parsed, current) {
            (Some(at), Some(now)) => now - at >= time::Duration::days(STALE_DAYS),
            _ => true,
        }
    }
}

/// A frozen window. `oldest`/`latest` are the immutable bounds; `latest` is the UTC selection
/// time and `oldest` is exactly 24 hours earlier. Once `ready`, the first successful result is
/// final and later reads never replace it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackWindow {
    pub id: String,
    pub channel_id: String,
    pub selected_at: String,
    pub oldest: String,
    pub latest: String,
    pub messages: Vec<super::slack_thread::CachedMessage>,
    pub link: String,
    pub ready: bool,
}

/// One hit of a detailed `slack_search_public_and_private` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailedHit {
    pub channel_id: String,
    pub channel_name: String,
    pub ts: String,
    pub permalink: String,
    pub thread_ts: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DetailedPage {
    pub hits: Vec<DetailedHit>,
    pub next_cursor: Option<String>,
}

fn result_field(value: &Value) -> Result<&str, ClaudeError> {
    for key in ["result", "results", "messages", "content"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return Ok(text);
        }
    }
    Err(ClaudeError::Parse(
        "connector result has no text field".into(),
    ))
}

fn json_text(text: &str) -> Result<Value, ClaudeError> {
    serde_json::from_str(text.trim()).map_err(|_| {
        let head: String = text.chars().take(120).collect();
        ClaudeError::Tool(head)
    })
}

/// Cursors appear as `Use cursor: "abc"` or ``use cursor: `abc` `` depending on the tool.
fn any_cursor(info: &str) -> Option<String> {
    let at = info.to_ascii_lowercase().find("cursor")?;
    let rest = &info[at..];
    for (open, close) in [('"', '"'), ('`', '`')] {
        if let (Some(start), Some(tail)) = (rest.find(open), rest.rfind(close)) {
            if start < tail {
                let value = rest[start + open.len_utf8()..tail].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn id_in_parens(text: &str) -> Option<String> {
    let start = text.find('(')? + 1;
    let end = text[start..].find(')')? + start;
    let inner = text[start..end].trim();
    let id = inner.strip_prefix("ID:").unwrap_or(inner).trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Parses the markdown of one `slack_list_user_channels` page. The `Type` line is authoritative:
/// a `C` id alone cannot tell a group DM from a channel.
pub fn parse_directory(text: &str, now: &str) -> Result<DirectoryPage, ClaudeError> {
    let value = json_text(text)?;
    let body = result_field(&value)?;
    let mut page = DirectoryPage {
        next_cursor: value
            .get("pagination_info")
            .and_then(Value::as_str)
            .and_then(any_cursor),
        ..DirectoryPage::default()
    };
    let mut archived_section = false;
    let mut block: Option<(
        String,
        Option<String>,
        Option<ConversationKind>,
        Vec<String>,
        bool,
    )> = None;
    let flush = |block: &mut Option<(
        String,
        Option<String>,
        Option<ConversationKind>,
        Vec<String>,
        bool,
    )>,
                 page: &mut DirectoryPage| {
        let Some((header, id, kind, members, archived)) = block.take() else {
            return;
        };
        let (Some(id), Some(kind)) = (id, kind) else {
            return;
        };
        if kind == ConversationKind::Unknown {
            return;
        }
        let name = directory_name(&header, kind, &members);
        page.conversations.push(DirectoryConversation {
            id,
            kind,
            name,
            members,
            archived,
            discovered: false,
            updated_at: now.to_string(),
        });
    };
    for line in body.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("### ") {
            flush(&mut block, &mut page);
            block = Some((
                rest.trim().to_string(),
                None,
                None,
                Vec::new(),
                archived_section,
            ));
            continue;
        }
        if line.starts_with("## ") {
            flush(&mut block, &mut page);
            archived_section = line.starts_with("## Archived");
            continue;
        }
        if line.contains("cursor:") || line.to_ascii_lowercase().contains("cursor:") {
            if let Some(cursor) = any_cursor(line) {
                page.next_cursor = Some(cursor);
            }
        }
        let Some(current) = block.as_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("ID:") {
            current.1 = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Type:") {
            current.2 = Some(ConversationKind::from_connector(rest));
        } else if let Some(rest) = line.strip_prefix("User ID:") {
            let id = rest.trim();
            if !id.is_empty() && !current.3.iter().any(|member| member == id) {
                current.3.push(id.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("Members:") {
            let handles = rest.split_once(':').map_or(rest, |(_, tail)| tail);
            for member in handles.split(',') {
                let member = member.trim().trim_start_matches('@').trim();
                if !member.is_empty() && !current.3.contains(&member.to_string()) {
                    current.3.push(member.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("Archived:") {
            let yes = matches!(rest.trim().to_ascii_lowercase().as_str(), "yes" | "true");
            current.4 |= yes;
        }
    }
    flush(&mut block, &mut page);
    page.conversations
        .retain(|conversation| !conversation.id.is_empty());
    Ok(page)
}

fn directory_name(header: &str, _kind: ConversationKind, members: &[String]) -> String {
    let header = header.trim();
    if header.is_empty() || header.eq_ignore_ascii_case("Group DM") {
        let others = if members.len() > 1 {
            members[..members.len().saturating_sub(1)].join(", ")
        } else {
            members.join(", ")
        };
        return if others.is_empty() {
            String::new()
        } else {
            format!("Group: {others}")
        };
    }
    header
        .strip_prefix("DM with ")
        .map(str::trim)
        .unwrap_or(header)
        .to_string()
}

/// Parses a detailed message-search page. Unlike v1's DM-only `parse_search`, this keeps the
/// permalink (the only place a reply's `thread_ts` lives) and a first-line snippet.
pub fn parse_search_detailed(text: &str, now: &str) -> Result<DetailedPage, ClaudeError> {
    let value = json_text(text)?;
    let body = result_field(&value)?;
    let mut page = DetailedPage {
        next_cursor: value
            .get("pagination_info")
            .and_then(Value::as_str)
            .and_then(any_cursor),
        ..DetailedPage::default()
    };
    for block in body.split("### Result ").skip(1) {
        let mut channel: Option<(String, String)> = None;
        let mut ts = None;
        let mut permalink = None;
        let mut snippet = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("Channel:") {
                let name = rest
                    .split(" (ID:")
                    .next()
                    .unwrap_or(rest)
                    .trim()
                    .to_string();
                channel = id_in_parens(rest).map(|id| (id, name));
            } else if let Some(rest) = line.strip_prefix("Message_ts:") {
                let value = rest.trim();
                if !value.is_empty() {
                    ts = Some(value.to_string());
                }
            } else if let Some(rest) = line.strip_prefix("Permalink:") {
                permalink = bracket_url(rest).map(str::to_string);
            } else if let Some(rest) = line.strip_prefix("Text:") {
                let first = rest.trim();
                snippet = Some(if first.is_empty() {
                    block.lines().nth(1).unwrap_or("").trim().to_string()
                } else {
                    first.to_string()
                });
            }
        }
        let (Some((channel_id, channel_name)), Some(ts), Some(permalink)) =
            (channel, ts, permalink)
        else {
            continue;
        };
        let (_, _, thread_ts) = match permalink_parts(&permalink) {
            Some(parts) => parts,
            None => continue,
        };
        let _ = now;
        page.hits.push(DetailedHit {
            channel_id,
            channel_name,
            ts,
            permalink,
            thread_ts,
            snippet: snippet.unwrap_or_default(),
        });
    }
    Ok(page)
}

fn bracket_url(line: &str) -> Option<&str> {
    let start = line.find("](")? + 2;
    let end = start + line[start..].find(')')?;
    Some(&line[start..end])
}

/// `(channel id, message ts, thread ts)` from a Slack permalink. The `thread_ts` query is the
/// parent for a reply; absent one the message is its own thread root.
fn permalink_parts(url: &str) -> Option<(String, String, Option<String>)> {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let path = base.rsplit('/').next()?;
    let message = path.strip_prefix('p')?;
    if message.is_empty() || !message.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let channel = base.split("/archives/").nth(1)?.split('/').next()?;
    if channel.is_empty() {
        return None;
    }
    let thread = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("thread_ts="))
        .and_then(slack_ts);
    Some((channel.to_string(), slack_ts(message)?, thread))
}

/// Insert the decimal point before the last six digits of a compact Slack timestamp.
fn slack_ts(raw: &str) -> Option<String> {
    if raw.contains('.') {
        let (seconds, micros) = raw.split_once('.')?;
        return (seconds.bytes().all(|b| b.is_ascii_digit())
            && micros.len() == 6
            && micros.bytes().all(|b| b.is_ascii_digit()))
        .then(|| raw.to_string());
    }
    if !raw.bytes().all(|b| b.is_ascii_digit()) || raw.len() < 16 {
        return None;
    }
    let split = raw.len() - 6;
    Some(format!("{}.{}", &raw[..split], &raw[split..]))
}

/// A validated run: every call used the fixed arguments, followed the previous page's cursor and
/// produced a result. A repeated cursor, a skipped result or a malformed page rejects the run.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryRun {
    pub pages: Vec<DirectoryPage>,
    pub cost_usd: f64,
    pub tool_use_count: usize,
}

/// Validates the raw stream of one directory invocation, without trusting the model's prose.
pub fn parse_directory_run(
    stdout: &str,
    initial_cursor: Option<&str>,
    now: &str,
) -> Result<DirectoryRun, ClaudeError> {
    let mut connector = None;
    let mut saw_init = false;
    let mut cost_usd = 0.0;
    let mut calls: Vec<(String, Value)> = Vec::new();
    let mut results: Vec<(String, ToolOutput)> = Vec::new();
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("system") if event.get("subtype").and_then(Value::as_str) == Some("init") => {
                saw_init = true;
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
                for block in event
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if Tool::from_full_name(name) != Some(Tool::ListChannels) {
                        return Err(ClaudeError::Rejected(name.to_string()));
                    }
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    calls.push((id, input));
                }
            }
            Some("user") => {
                for block in event
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let text =
                        claude_slack::content_text(block.get("content").unwrap_or(&Value::Null));
                    results.push((
                        id,
                        ToolOutput {
                            text,
                            is_error: block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        },
                    ));
                }
            }
            Some("result") => {
                cost_usd = event
                    .get("total_cost_usd")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
            }
            _ => {}
        }
    }
    if !saw_init {
        return Err(ClaudeError::Cli("no init event in the output".into()));
    }
    if connector.as_deref() != Some("connected") {
        return Err(ClaudeError::ConnectorDown(
            connector.unwrap_or_else(|| "not listed".into()),
        ));
    }
    if calls.is_empty() || calls.len() != results.len() {
        return Err(ClaudeError::Parse("an incomplete directory run".into()));
    }
    let mut pages = Vec::with_capacity(calls.len());
    let mut expected = initial_cursor.map(str::to_string);
    let mut seen: Vec<String> = Vec::new();
    for (index, (id, input)) in calls.iter().enumerate() {
        if input.get("types").and_then(Value::as_str) != Some(FIXED_TYPES)
            || input.get("format").and_then(Value::as_str) != Some("full")
            || input.get("limit").and_then(Value::as_i64) != Some(200)
        {
            return Err(ClaudeError::Rejected(
                "slack_list_user_channels arguments".into(),
            ));
        }
        let cursor = input
            .get("cursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        if cursor.as_deref() != expected.as_deref() {
            return Err(ClaudeError::Rejected(
                "a repeated or unexpected cursor".into(),
            ));
        }
        if let Some(cursor) = &cursor {
            if seen.iter().any(|other| other == cursor) {
                return Err(ClaudeError::Rejected("a repeated cursor".into()));
            }
            seen.push(cursor.clone());
        }
        let (result_id, output) = &results[index];
        if result_id != id || output.failed() {
            return Err(ClaudeError::Tool("slack_list_user_channels failed".into()));
        }
        let page = parse_directory(&output.text, now)?;
        expected = page.next_cursor.clone();
        pages.push(page);
    }
    Ok(DirectoryRun {
        pages,
        cost_usd,
        tool_use_count: calls.len(),
    })
}

/// The one-run request: the model is told to follow the returned cursor for up to two pages and
/// nothing else. The strict args are validated after the run by `parse_directory_run`.
pub fn directory_request(cursor: Option<&str>) -> RunRequest {
    let call = ToolCall::list_channels(cursor);
    let prompt = format!(
        "Call {} exactly once with these arguments: {}. If the result shows another page, call \
         the same tool once more with the cursor it names. Make at most {} calls in total. Add \
         no other parameters. Do not summarize. Reply with the single word: done",
        call.tool.full_name(),
        call.input,
        PAGES_PER_RUN
    );
    RunRequest {
        calls: vec![call],
        prompt,
        max_turns: 4,
        disallow: Vec::new(),
        timeout: Some(DIRECTORY_TIMEOUT),
        tools: Some(vec![Tool::ListChannels.full_name()]),
    }
}

fn search_request_with(query: &str, suppress_context: bool) -> RunRequest {
    let mut call = ToolCall::search_messages(query);
    if !suppress_context {
        call.input
            .as_object_mut()
            .map(|input| input.remove("include_context"));
    }
    RunRequest {
        calls: vec![call.clone()],
        prompt: build_prompt(std::slice::from_ref(&call)),
        max_turns: 2,
        disallow: Vec::new(),
        timeout: Some(SEARCH_TIMEOUT),
        tools: Some(vec![Tool::Search.full_name()]),
    }
}

/// Outcome of a full directory scan. The previous directory is never cleared on failure.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DirectoryScanResult {
    pub conversations: usize,
    pub invocations: usize,
    pub calls: usize,
    pub cost_usd: f64,
    pub scanned_at: Option<String>,
    pub error: Option<String>,
    /// Another scan held the single-flight guard; no CLI call was started.
    pub coalesced: bool,
}

fn safe_error(error: &ClaudeError) -> String {
    match error {
        ClaudeError::NotFound => {
            "Claude Code not found. Install it, then sign in once with `claude`.".into()
        }
        ClaudeError::ConnectorDown(status) if status == "pending" => {
            "Slack is still connecting. Try again shortly".into()
        }
        ClaudeError::ConnectorDown(_) => {
            "Claude Slack connector is not connected or signed out".into()
        }
        ClaudeError::Timeout => "Slack directory scan timed out".into(),
        ClaudeError::Cli(_)
        | ClaudeError::Tool(_)
        | ClaudeError::Parse(_)
        | ClaudeError::Rejected(_) => "Slack returned an unreadable directory".into(),
        ClaudeError::Store(_) => "the local directory is unavailable".into(),
    }
}

/// Runs a full scan. On success the merged directory and the successful-scan timestamp are
/// committed together; on any failure the last directory and freshness time stay untouched and
/// only the resume cursor moves. The guard is separate from the thread gate.
pub fn scan_directory(store: &V2Store, runner: &dyn Runner, now: &str) -> DirectoryScanResult {
    let Some(_guard) = store.try_begin_directory() else {
        return DirectoryScanResult {
            coalesced: true,
            ..DirectoryScanResult::default()
        };
    };
    let Ok(state) = store.directory_state() else {
        return DirectoryScanResult {
            error: Some("the local directory is unavailable".into()),
            ..DirectoryScanResult::default()
        };
    };
    let Ok(existing) = store.directory() else {
        return DirectoryScanResult {
            error: Some("the local directory is unavailable".into()),
            ..DirectoryScanResult::default()
        };
    };
    let mut cursor = state.last_cursor.clone();
    let mut collected: Vec<DirectoryConversation> = Vec::new();
    let mut result = DirectoryScanResult::default();
    for _ in 0..MAX_RUNS {
        let request = directory_request(cursor.as_deref());
        let stdout = match runner.run(&request) {
            Ok(stdout) => stdout,
            Err(error) => {
                result.error = Some(safe_error(&error));
                break;
            }
        };
        result.invocations += 1;
        let run = match parse_directory_run(&stdout, cursor.as_deref(), now) {
            Ok(run) => run,
            Err(error) => {
                result.error = Some(safe_error(&error));
                break;
            }
        };
        result.calls += run.tool_use_count;
        result.cost_usd += run.cost_usd;
        for page in &run.pages {
            collected.extend(page.conversations.iter().cloned());
        }
        let next = run.pages.last().and_then(|page| page.next_cursor.clone());
        match &next {
            None => {
                let merged = merge_conversations(&existing, &collected, now);
                match store.save_directory(&merged, Some(now), None) {
                    Ok(()) => {
                        result.conversations = merged.len();
                        result.scanned_at = Some(now.to_string());
                    }
                    Err(_) => {
                        result.error = Some("the local directory is unavailable".into());
                    }
                }
                return result;
            }
            Some(next) => {
                // Resume from the last validated cursor. The previous directory and clock stay.
                let _ = store.save_directory(&existing, None, Some(next));
                cursor = Some(next.clone());
            }
        }
    }
    if result.error.is_none() {
        result.error = Some("the directory scan was too large to finish; it will resume".into());
    }
    result
}

/// Keeps discoveries that a scan did not see, and never lets a thin discovery overwrite a known
/// kind, name or member list. A full scan row is authoritative for its own id.
pub fn merge_conversations(
    existing: &[DirectoryConversation],
    incoming: &[DirectoryConversation],
    now: &str,
) -> Vec<DirectoryConversation> {
    let mut out: Vec<DirectoryConversation> = existing.to_vec();
    for candidate in incoming {
        if let Some(current) = out.iter_mut().find(|row| row.id == candidate.id) {
            if candidate.discovered {
                // A discovery only adds what is missing. A scanned row keeps its explicit kind.
                if current.kind == ConversationKind::Unknown {
                    current.kind = candidate.kind;
                }
                if current.name.is_empty() {
                    current.name = candidate.name.clone();
                }
                for member in &candidate.members {
                    if !current.members.contains(member) {
                        current.members.push(member.clone());
                    }
                }
                current.updated_at = now.to_string();
            } else {
                let mut merged = candidate.clone();
                if merged.kind == ConversationKind::Unknown {
                    merged.kind = current.kind;
                }
                if merged.name.is_empty() {
                    merged.name = current.name.clone();
                }
                if merged.members.is_empty() {
                    merged.members = current.members.clone();
                }
                merged.discovered = false;
                merged.updated_at = now.to_string();
                *current = merged;
            }
            continue;
        }
        let mut row = candidate.clone();
        row.updated_at = now.to_string();
        out.push(row);
    }
    out
}

/// The fallback discovery from a permalink: an id, perhaps a channel name, nothing else. Only a
/// `D` id establishes a kind; a `C` id stays unknown until a scan classifies it.
pub fn discovery(id: &str, name: Option<&str>) -> Option<DirectoryConversation> {
    if id.is_empty() {
        return None;
    }
    Some(DirectoryConversation {
        id: id.to_string(),
        kind: if id.starts_with('D') {
            ConversationKind::Dm
        } else {
            ConversationKind::Unknown
        },
        name: name.unwrap_or("").to_string(),
        members: Vec::new(),
        archived: false,
        discovered: true,
        updated_at: crate::model::now(),
    })
}

/// Adds permalink/search discoveries to the directory without advancing the scan clock.
pub fn note_discovery(store: &V2Store, id: &str, name: Option<&str>, now: &str) {
    let Ok(existing) = store.directory() else {
        return;
    };
    let Some(found) = discovery(id, name) else {
        return;
    };
    let merged = merge_conversations(&existing, std::slice::from_ref(&found), now);
    let _ = store.save_directory(&merged, None, None);
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchOutcome {
    pub hits: Vec<DetailedHit>,
    pub error: Option<String>,
}

/// One explicit search. A `pending` connector, timeout or skipped call gets no automatic retry:
/// the picker owns the retry when the user presses Enter again on a new query.
pub fn search_messages(
    store: &V2Store,
    runner: &dyn Runner,
    query: &str,
    now: &str,
) -> SearchOutcome {
    search_messages_with(store, runner, query, now, true)
}

fn search_messages_with(
    store: &V2Store,
    runner: &dyn Runner,
    query: &str,
    now: &str,
    suppress_context: bool,
) -> SearchOutcome {
    let request = search_request_with(query, suppress_context);
    let stdout = match runner.run(&request) {
        Ok(stdout) => stdout,
        Err(error) => {
            return SearchOutcome {
                hits: Vec::new(),
                error: Some(safe_error(&error)),
            };
        }
    };
    let data = match parse_stream(&stdout, &request.calls) {
        Ok(data) => data,
        Err(error) => {
            return SearchOutcome {
                hits: Vec::new(),
                error: Some(safe_error(&error)),
            };
        }
    };
    if data.connector_status.as_deref() != Some("connected") {
        let detail = if data.connector_status.as_deref() == Some("pending") {
            "Slack is still connecting. Try again shortly"
        } else {
            "Claude Slack connector is not connected or signed out"
        };
        return SearchOutcome {
            hits: Vec::new(),
            error: Some(detail.into()),
        };
    }
    if data.tool_use_count != 1 || data.duplicate_tool_use {
        return SearchOutcome {
            hits: Vec::new(),
            error: Some("Slack returned an unreadable search".into()),
        };
    }
    let Some(output) = data.results.first().and_then(|slot| slot.as_ref()) else {
        return SearchOutcome {
            hits: Vec::new(),
            error: Some("Slack is still connecting. Try again shortly".into()),
        };
    };
    if output.failed() {
        let text = output.text.to_ascii_lowercase();
        // A connector that rejects context suppression still gives detailed hits; the parser
        // reads only hit fields. This compatibility fallback is not a transient retry.
        if text.contains("include_context")
            || text.contains("unexpected argument")
            || text.contains("unexpected parameter")
        {
            return search_messages_with(store, runner, query, now, false);
        }
        let reason = if text.contains("invalid_auth") || text.contains("not_authed") {
            "Claude Slack connector is not connected or signed out"
        } else {
            "Slack returned an unreadable search"
        };
        return SearchOutcome {
            hits: Vec::new(),
            error: Some(reason.into()),
        };
    }
    let page = match parse_search_detailed(&output.text, now) {
        Ok(page) => page,
        Err(error) => {
            return SearchOutcome {
                hits: Vec::new(),
                error: Some(safe_error(&error)),
            };
        }
    };
    for hit in &page.hits {
        note_discovery(store, &hit.channel_id, Some(&hit.channel_name), now);
    }
    SearchOutcome {
        hits: page.hits,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::V2PickerGroup;
    use std::cell::RefCell;

    fn directory_text(blocks: &[(&str, &str, &str, &str, bool)], cursor: Option<&str>) -> String {
        let mut body = String::from("## My Channels\n\n");
        for (header, id, kind, extra, archived) in blocks {
            body.push_str(&format!(
                "### {header}\nID: {id}\nType: {kind}\n{extra}\nArchived: {}\n\n",
                if *archived { "Yes" } else { "No" }
            ));
        }
        if let Some(cursor) = cursor {
            body.push_str(&format!(
                "Pagination: more results. Use cursor: \"{cursor}\"\n"
            ));
        }
        serde_json::json!({ "result": body }).to_string()
    }

    fn stream(calls: &[(&str, Option<&str>)], results: &[String], connector: &str) -> String {
        let mut lines = vec![
            serde_json::json!({
                "type": "system", "subtype": "init", "tools": [Tool::ListChannels.full_name()],
                "mcp_servers": [{"name": SERVER_NAME, "status": connector}]
            })
            .to_string(),
        ];
        for ((id, cursor), text) in calls.iter().zip(results) {
            let mut input = serde_json::json!({
                "types": FIXED_TYPES, "format": "full", "limit": 200
            });
            if let Some(cursor) = cursor {
                input["cursor"] = serde_json::json!(cursor);
            }
            lines.push(
                serde_json::json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":id,"name":Tool::ListChannels.full_name(),"input":input}]}})
                .to_string(),
            );
            lines.push(
                serde_json::json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":id,"content":[{"type":"text","text":text}]}]}})
                .to_string(),
            );
        }
        lines.push(serde_json::json!({"type":"result","total_cost_usd":0.01,"num_turns":3,"is_error":false}).to_string());
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
            assert_eq!(request.calls[0].tool, Tool::ListChannels);
            assert_eq!(request.timeout, Some(DIRECTORY_TIMEOUT));
            *self.calls.borrow_mut() += 1;
            self.responses.borrow_mut().remove(0)
        }
    }

    #[test]
    fn parser_accepts_a_short_page_with_a_cursor_and_all_four_kinds() {
        let text = directory_text(
            &[
                (
                    "#general",
                    "C001",
                    "Public Channel",
                    "Topic: updates",
                    false,
                ),
                ("#secret", "C002", "Private Channel", "", true),
                (
                    "DM with Pat",
                    "D003",
                    "Direct Message",
                    "User ID: U1",
                    false,
                ),
                (
                    "Group DM",
                    "C004",
                    "Group DM",
                    "Members: Group messaging with: @pat, @sam, @me",
                    false,
                ),
            ],
            Some("next-page"),
        );
        let page = parse_directory(&text, "2026-11-21T00:00:00.000Z").unwrap();
        assert_eq!(page.conversations.len(), 4);
        assert_eq!(page.conversations[0].kind, ConversationKind::Public);
        assert_eq!(page.conversations[1].kind, ConversationKind::Private);
        assert!(page.conversations[1].archived);
        assert_eq!(page.conversations[2].kind, ConversationKind::Dm);
        assert_eq!(page.conversations[3].kind, ConversationKind::Group);
        assert!(page.conversations[3].name.starts_with("Group:"));
        assert_eq!(page.next_cursor.as_deref(), Some("next-page"));
    }

    #[test]
    fn a_c_prefixed_group_dm_is_not_inferred_from_its_id() {
        let found = discovery("C004", None).unwrap();
        assert_eq!(found.kind, ConversationKind::Unknown);
        assert_eq!(discovery("D003", None).unwrap().kind, ConversationKind::Dm);
    }

    #[test]
    fn stream_rejects_a_repeated_cursor_and_changed_arguments() {
        let text = directory_text(&[("#a", "C1", "Public Channel", "", false)], Some("cur-1"));
        let repeated = stream(
            &[("a", None), ("b", Some("cur-1")), ("c", Some("cur-1"))],
            &[text.clone(), text.clone(), text.clone()],
            "connected",
        );
        assert!(parse_directory_run(&repeated, None, "now").is_err());

        let mut wrong = stream(&[("a", None)], &[text], "connected");
        wrong = wrong.replace("\"limit\":200", "\"limit\":50");
        assert!(parse_directory_run(&wrong, None, "now").is_err());
    }

    #[test]
    fn scan_commits_the_directory_and_clock_together_and_failure_keeps_both() {
        let store = V2Store::open_in_memory().unwrap();
        let page = directory_text(&[("#a", "C1", "Public Channel", "", false)], None);
        let fake = Fake::new(vec![Ok(stream(&[("a", None)], &[page], "connected"))]);
        let result = scan_directory(&store, &fake, "2026-11-21T00:00:00.000Z");
        assert_eq!(result.error, None);
        assert_eq!(result.conversations, 1);
        let state = store.directory_state().unwrap();
        assert_eq!(
            state.last_success_at.as_deref(),
            Some("2026-11-21T00:00:00.000Z")
        );

        let failing = Fake::new(vec![Ok(stream(&[], &[], "disconnected"))]);
        let failed = scan_directory(&store, &failing, "2026-11-22T00:00:00.000Z");
        assert!(failed.error.is_some());
        assert_eq!(store.directory().unwrap().len(), 1);
        assert_eq!(
            store.directory_state().unwrap().last_success_at.as_deref(),
            Some("2026-11-21T00:00:00.000Z")
        );
    }

    #[test]
    fn scan_follows_the_cursor_and_resumes_from_the_last_validated_one() {
        let store = V2Store::open_in_memory().unwrap();
        let first = directory_text(&[("#a", "C1", "Public Channel", "", false)], Some("cur-1"));
        let second = directory_text(&[("#b", "C2", "Public Channel", "", false)], None);
        let fake = Fake::new(vec![
            Ok(stream(&[("a", None)], &[first], "connected")),
            Ok(stream(&[("b", Some("cur-1"))], &[second], "connected")),
        ]);
        let result = scan_directory(&store, &fake, "2026-11-21T00:00:00.000Z");
        assert_eq!(result.error, None);
        assert_eq!(result.invocations, 2);
        assert_eq!(store.directory().unwrap().len(), 2);
    }

    #[test]
    fn discoveries_do_not_advance_the_scan_clock_and_preserve_known_metadata() {
        let store = V2Store::open_in_memory().unwrap();
        let page = directory_text(&[("#a", "C1", "Public Channel", "", false)], None);
        let fake = Fake::new(vec![Ok(stream(&[("a", None)], &[page], "connected"))]);
        let _ = scan_directory(&store, &fake, "2026-11-21T00:00:00.000Z");
        note_discovery(&store, "C1", None, "2026-11-22T00:00:00.000Z");
        note_discovery(&store, "C9", None, "2026-11-22T00:00:00.000Z");
        let rows = store.directory().unwrap();
        assert_eq!(rows.len(), 2);
        let known = rows.iter().find(|row| row.id == "C1").unwrap();
        assert_eq!(known.kind, ConversationKind::Public);
        assert_eq!(known.name, "#a");
        assert_eq!(
            store.directory_state().unwrap().last_success_at.as_deref(),
            Some("2026-11-21T00:00:00.000Z")
        );
    }

    #[test]
    fn due_is_true_when_missing_or_older_than_seven_days() {
        let state = DirectoryState::default();
        assert!(state.due("2026-11-21T00:00:00.000Z"));
        let fresh = DirectoryState {
            last_success_at: Some("2026-11-20T00:00:00.000Z".into()),
            last_cursor: None,
        };
        assert!(!fresh.due("2026-11-21T00:00:00.000Z"));
        let stale = DirectoryState {
            last_success_at: Some("2026-11-01T00:00:00.000Z".into()),
            last_cursor: None,
        };
        assert!(stale.due("2026-11-21T00:00:00.000Z"));
    }

    #[test]
    fn detailed_search_keeps_the_reply_thread_ts_and_a_snippet() {
        let body = "## Messages (2 results)\n\n### Result 1 of 2\nChannel: #general (ID: C1)\nParticipants: Pat (ID: U2)\nFrom: Pat <p@example.test> (ID: U2)\nTime: 2026-11-21 10:00:00 -03\nMessage_ts: 1731000000.000001\nPermalink: [link](https://acme.slack.com/archives/C1/p1731000000000001)\nText: first line\nmore text\n\n### Result 2 of 2\nChannel: #general (ID: C1)\nMessage_ts: 1731000000.000099\nPermalink: [link](https://acme.slack.com/archives/C1/p1731000000000099?thread_ts=1731000000.000001&cid=C1)\nText: a reply\n";
        let text = serde_json::json!({"results": body, "pagination_info": "no more"}).to_string();
        let page = parse_search_detailed(&text, "now").unwrap();
        assert_eq!(page.hits.len(), 2);
        assert_eq!(page.hits[0].snippet, "first line");
        assert_eq!(page.hits[1].thread_ts.as_deref(), Some("1731000000.000001"));
        assert_eq!(page.hits[1].ts, "1731000000.000099");
    }

    struct SearchFake {
        responses: RefCell<Vec<Result<String, ClaudeError>>>,
        calls: RefCell<usize>,
    }

    impl SearchFake {
        fn new(responses: Vec<Result<String, ClaudeError>>) -> Self {
            Self {
                responses: RefCell::new(responses),
                calls: RefCell::new(0),
            }
        }
    }

    impl Runner for SearchFake {
        fn run(&self, request: &RunRequest) -> Result<String, ClaudeError> {
            assert_eq!(request.calls[0].tool, Tool::Search);
            assert_eq!(request.timeout, Some(SEARCH_TIMEOUT));
            *self.calls.borrow_mut() += 1;
            self.responses.borrow_mut().remove(0)
        }
    }

    fn search_stream(call: &ToolCall, text: &str, connector: &str, failed: bool) -> String {
        let mut lines = vec![
            serde_json::json!({
                "type": "system", "subtype": "init", "tools": [Tool::Search.full_name()],
                "mcp_servers": [{"name": SERVER_NAME, "status": connector}]
            })
            .to_string(),
        ];
        lines.push(
            serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"s1","name":call.tool.full_name(),"input":call.input}]}})
            .to_string(),
        );
        lines.push(serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"s1","is_error":failed,"content":[{"type":"text","text":text}]}]}}).to_string());
        lines.push(serde_json::json!({"type":"result","total_cost_usd":0.004,"num_turns":2,"is_error":false}).to_string());
        lines.join("\n")
    }

    fn detailed_text() -> String {
        let body = "## Messages (1 result)\n\n### Result 1 of 1\nChannel: #general (ID: C9)\nMessage_ts: 1731000000.000001\nPermalink: [link](https://acme.slack.com/archives/C9/p1731000000000001)\nText: only a hit\n";
        serde_json::json!({"results": body, "pagination_info": "End of results"}).to_string()
    }

    #[test]
    fn search_notes_discovery_without_advancing_the_scan_clock() {
        let store = V2Store::open_in_memory().unwrap();
        let call = ToolCall::search_messages("deploy");
        let fake = SearchFake::new(vec![Ok(search_stream(
            &call,
            &detailed_text(),
            "connected",
            false,
        ))]);
        let outcome = search_messages(&store, &fake, "deploy", "2026-11-22T00:00:00.000Z");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(*fake.calls.borrow(), 1);
        let rows = store.directory().unwrap();
        assert!(rows.iter().any(|row| row.id == "C9" && row.discovered));
        assert_eq!(
            store.directory_state().unwrap().last_success_at,
            None,
            "a search discovery never advances the full-scan clock"
        );
    }

    #[test]
    fn a_failed_or_pending_search_does_not_retry() {
        let store = V2Store::open_in_memory().unwrap();
        let call = ToolCall::search_messages("deploy");
        let fake = SearchFake::new(vec![Ok(search_stream(&call, "", "pending", false))]);
        let outcome = search_messages(&store, &fake, "deploy", "now");
        assert!(outcome.error.is_some());
        assert!(outcome.hits.is_empty());
        assert_eq!(*fake.calls.borrow(), 1, "no automatic search retry");
    }

    #[test]
    fn context_suppression_rejection_retries_once_without_the_argument() {
        let store = V2Store::open_in_memory().unwrap();
        let first = search_request_with("deploy", true);
        let second = search_request_with("deploy", false);
        let fake = SearchFake::new(vec![
            Ok(search_stream(
                &first.calls[0],
                "unexpected argument include_context",
                "connected",
                true,
            )),
            Ok(search_stream(
                &second.calls[0],
                &detailed_text(),
                "connected",
                false,
            )),
        ]);
        let outcome = search_messages(&store, &fake, "deploy", "now");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(*fake.calls.borrow(), 2);
    }

    #[test]
    fn directory_rows_are_pickable_as_conversations() {
        let store = V2Store::open_in_memory().unwrap();
        let page = directory_text(&[("#general", "C1", "Public Channel", "", false)], None);
        let fake = Fake::new(vec![Ok(stream(&[("a", None)], &[page], "connected"))]);
        let _ = scan_directory(&store, &fake, "2026-11-21T00:00:00.000Z");
        let rows = store.reference_picker("general").unwrap();
        let conversation = rows.iter().find(|row| row.key == "slackconv:C1").unwrap();
        assert_eq!(conversation.kind, "slackchannel");
        assert_eq!(conversation.display_id, "@#general");
        assert_eq!(conversation.group, V2PickerGroup::Conversations);
    }

    #[test]
    fn search_request_uses_the_explicit_detailed_arguments() {
        let request = search_request_with("deploy", true);
        let input = &request.calls[0].input;
        assert_eq!(input["keywords"][0], "deploy");
        assert_eq!(input["natural_language_query"], "");
        assert_eq!(input["response_format"], "detailed");
        assert_eq!(input["include_context"], false);
        assert_eq!(input["limit"], 10);
        assert_eq!(request.timeout, Some(SEARCH_TIMEOUT));
    }
}
