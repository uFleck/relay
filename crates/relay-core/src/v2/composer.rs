//! Local parsing for Composer submissions. Everything here runs without a provider: a
//! recognised URL becomes a link-only Reference, and a project comes from the v2 registry
//! or from an explicit `+slug`. The full original text of every line is kept for editing.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::model::{
    AgentMode, NoteWrite, PreparedLine, PreparedNoteWrite, PreparedReference, PreparedTaskIntent,
    V2Note, V2Project,
};
use super::store::{V2Error, V2Store};

/// One line in an Edit request. `line_id` is the id the caller read from the Note; `null`
/// means a new line. Text is re-parsed on every save, so IDs are never derived from text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerLineInput {
    pub line_id: Option<String>,
    pub text: String,
}

/// The two ways the Composer writes. `Create` is idempotent on its submission key and
/// `Edit` is guarded by the revision the caller read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ComposerSaveRequest {
    Create {
        submission_key: String,
        text: String,
    },
    Edit {
        note_id: String,
        expected_revision: i64,
        lines: Vec<ComposerLineInput>,
    },
}

/// The local parse of one line. `text` is the untouched line (with LF endings), including
/// its sigil, URLs and `+slug`, so the editor can round-trip it byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub text: String,
    pub agent_mode: Option<AgentMode>,
    pub references: Vec<PreparedReference>,
    pub project_token: Option<String>,
}

/// Parses one line. `??` is matched before `?` and `!`, and only at column one. A sigil
/// needs no following space; a leading space makes the line plain.
pub fn parse_line(text: &str) -> ParsedLine {
    ParsedLine {
        text: text.to_string(),
        agent_mode: directive(text),
        references: find_references(text),
        project_token: find_project_token(text),
    }
}

fn directive(text: &str) -> Option<AgentMode> {
    if text.starts_with("??") {
        Some(AgentMode::Grill)
    } else if text.starts_with('?') {
        Some(AgentMode::Wait)
    } else if text.starts_with('!') {
        Some(AgentMode::Act)
    } else {
        None
    }
}

/// The first standalone `+slug` on the line. A token is standalone when it starts at the
/// beginning or after whitespace and ends at whitespace or the end of the line. Unknown
/// slugs are returned too, so project inference cannot silently fall back to a URL.
fn find_project_token(text: &str) -> Option<String> {
    for (at, ch) in text.char_indices() {
        if ch != '+' || (at != 0 && !text[..at].chars().last().is_some_and(is_space)) {
            continue;
        }
        let rest = &text[at + 1..];
        let len = rest
            .char_indices()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .unwrap_or(0);
        if len == 0 {
            continue;
        }
        let token = &rest[..len];
        if rest[len..].chars().next().is_none_or(is_space) {
            return Some(token.to_string());
        }
    }
    None
}

fn is_space(ch: char) -> bool {
    ch.is_whitespace()
}

/// Recognises GitHub pull/issue URLs, ClickUp card URLs and Slack permalinks in text order.
/// Other URLs stay text. Duplicate targets on one line keep their first position, because
/// `line_refs` is keyed by line plus Reference.
fn find_references(text: &str) -> Vec<PreparedReference> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut from = 0;
    while let Some(relative) = lower[from..].find("https://") {
        let start = from + relative;
        let mut end = text.len();
        for (offset, ch) in text[start..].char_indices() {
            if ch.is_ascii_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`') {
                end = start + offset;
                break;
            }
        }
        let raw = &text[start..end];
        let url = trim_url_tail(raw);
        let url = url.split(['?', '#']).next().unwrap_or(url);
        let parsed = github_reference(url)
            .or_else(|| clickup_reference(url))
            .or_else(|| slack_reference(url));
        if let Some((kind, target_id, canonical_url)) = parsed
            && seen.insert((kind.to_string(), target_id.clone()))
        {
            found.push(PreparedReference {
                kind: kind.to_string(),
                target_id,
                canonical_url,
                snapshot_json: None,
            });
        }
        from = start + 8;
    }
    found
}

fn trim_url_tail(mut url: &str) -> &str {
    loop {
        let Some((_, ch)) = url.char_indices().last() else {
            break;
        };
        if !matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"'
        ) {
            break;
        }
        url = &url[..url.len() - ch.len_utf8()];
    }
    url
}

/// `(kind, target id, canonical url)`. The owner/repo on a GitHub target is what project
/// inference matches against the registry.
fn github_reference(url: &str) -> Option<(&'static str, String, String)> {
    let path = url.strip_prefix("https://github.com/")?;
    let segments: Vec<&str> = path.split('/').collect();
    let [owner, repo, kind, number] = segments.as_slice() else {
        return None;
    };
    if owner.is_empty()
        || repo.is_empty()
        || !matches!(*kind, "pull" | "issues")
        || number.is_empty()
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let target_id = format!("{owner}/{repo}#{number}");
    let canonical_url = format!("https://github.com/{owner}/{repo}/{kind}/{number}");
    Some(("github", target_id, canonical_url))
}

fn clickup_reference(url: &str) -> Option<(&'static str, String, String)> {
    let path = url.strip_prefix("https://app.clickup.com/t/")?;
    let id = path.trim_end_matches('/');
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some((
        "clickup",
        format!("CU-{id}"),
        format!("https://app.clickup.com/t/{id}"),
    ))
}

fn slack_reference(url: &str) -> Option<(&'static str, String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if !(host == "slack.com" || host.ends_with(".slack.com")) {
        return None;
    }
    let segments: Vec<&str> = path.split('/').collect();
    let [archives, channel, permalink] = segments.as_slice() else {
        return None;
    };
    if *archives != "archives" || channel.is_empty() {
        return None;
    }
    let raw_ts = permalink.strip_prefix('p')?;
    if raw_ts.is_empty() || !raw_ts.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ts = if raw_ts.len() > 10 && !raw_ts.contains('.') {
        let split = raw_ts.len() - 6;
        format!("{}.{}", &raw_ts[..split], &raw_ts[split..])
    } else {
        raw_ts.to_string()
    };
    Some((
        "slack",
        format!("{channel}/{ts}"),
        format!("https://{host}/archives/{channel}/p{raw_ts}"),
    ))
}

/// Explicit `+slug` wins, even when it is unknown; otherwise the first GitHub Reference on
/// the line supplies its `owner/repo`. Nothing else infers a project.
fn infer_project(parsed: &ParsedLine, projects: &[V2Project]) -> Option<String> {
    if let Some(token) = &parsed.project_token {
        let slug = token.to_ascii_lowercase();
        return projects
            .iter()
            .find(|p| p.slug == slug)
            .map(|p| p.id.clone());
    }
    let target = parsed
        .references
        .iter()
        .find(|reference| reference.kind == "github")?;
    let owner_repo = target.target_id.rsplit_once('#')?.0.to_ascii_lowercase();
    projects
        .iter()
        .find(|p| p.owner_repo.as_deref() == Some(owner_repo.as_str()))
        .map(|p| p.id.clone())
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Parses a submission and writes it through the store's guarded `save_note`. The store
/// owns the revision check, idempotent replay and transaction; this function stays pure
/// apart from the registry read it needs for project inference.
pub fn save_submission(store: &V2Store, request: ComposerSaveRequest) -> Result<V2Note, V2Error> {
    let projects = store.list_projects()?;
    let mut lines = Vec::new();
    let write = match request {
        ComposerSaveRequest::Create {
            submission_key,
            text,
        } => {
            let text = normalize_newlines(&text);
            if text.trim().is_empty() {
                return Err(V2Error::InvalidInput(
                    "a new draft needs at least one non-blank line".into(),
                ));
            }
            for line in text.split('\n') {
                lines.push((None, parse_line(line)));
            }
            NoteWrite::Create { submission_key }
        }
        ComposerSaveRequest::Edit {
            note_id,
            expected_revision,
            lines: inputs,
        } => {
            for input in inputs {
                lines.push((input.line_id, parse_line(&normalize_newlines(&input.text))));
            }
            NoteWrite::Edit {
                note_id,
                expected_revision,
            }
        }
    };

    let prepared = lines
        .into_iter()
        .map(|(line_id, parsed)| {
            let project_id = infer_project(&parsed, &projects);
            PreparedLine {
                line_id,
                text: parsed.text,
                references: parsed.references,
                task_intent: parsed.agent_mode.map(|agent_mode| PreparedTaskIntent {
                    agent_mode,
                    project_id,
                }),
            }
        })
        .collect();

    store.save_note(PreparedNoteWrite {
        write,
        source: "composer".into(),
        lines: prepared,
    })
}

#[cfg(test)]
mod tests;
