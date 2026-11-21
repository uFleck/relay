//! Canonical Reference identity, display and local parsing for the v2 picker and chips.
//!
//! A `ReferenceKey` is identity only: display text, the resolved title, the workspace host
//! and availability all live outside it. Two keys that point at the same target are equal
//! and hash alike even when their presentation differs.

use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use super::model::PreparedReference;

/// GitHub PRs and issues share one number space, so `kind` travels with the key but is
/// deliberately excluded from identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GithubItemKind {
    Pull,
    Issue,
}

impl GithubItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Issue => "issues",
        }
    }
}

/// The identity of a Reference across Notes, Tasks, the picker, the Lens, snapshots and
/// the Import ledger. Presentation fields (`host`, `item`) do not participate in equality
/// or hashing; use `identity()` as the stable key for storage and deduplication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceKey {
    Clickup {
        id: String,
    },
    Github {
        owner_repo: String,
        number: u64,
        item: GithubItemKind,
    },
    Slack {
        channel_id: String,
        thread_ts: String,
        host: String,
        /// The message in the permalink path. Presentation only: a reply link keeps its own
        /// message here while `thread_ts` stays the shared root used for identity and caching.
        permalink_ts: Option<String>,
    },
    /// #33: a fixed window captured at selection. Identity is the local window id, so two picks
    /// of the same channel stay distinct References; the conversation link is presentation.
    SlackWindow {
        id: String,
        channel_id: String,
        host: String,
        name: Option<String>,
    },
    Note {
        id: String,
    },
    Task {
        id: String,
    },
}

impl ReferenceKey {
    /// The stable identity. This is what `line_refs`, `task_refs`, the Import ledger and
    /// the picker use to deduplicate; it never contains display text or availability.
    pub fn identity(&self) -> String {
        match self {
            Self::Clickup { id } => format!("clickup:{id}"),
            Self::Github {
                owner_repo, number, ..
            } => format!("github:{owner_repo}#{number}"),
            Self::Slack {
                channel_id,
                thread_ts,
                ..
            } => format!("slack:{channel_id}/{thread_ts}"),
            Self::SlackWindow { id, .. } => format!("slackwindow:{id}"),
            Self::Note { id } => format!("note:{id}"),
            Self::Task { id } => format!("task:{id}"),
        }
    }

    /// The `ref_targets.kind` value. GitHub keeps the legacy `github` kind so existing
    /// databases and filters stay valid; the PR/issue distinction lives in the URL.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Clickup { .. } => "clickup",
            Self::Github { .. } => "github",
            Self::Slack { .. } => "slack",
            Self::SlackWindow { .. } => "slackwindow",
            Self::Note { .. } => "note",
            Self::Task { .. } => "task",
        }
    }

    /// The `ref_targets.target_id` value for this identity.
    pub fn target_id(&self) -> String {
        match self {
            Self::Clickup { id } => format!("CU-{id}"),
            Self::Github {
                owner_repo, number, ..
            } => format!("{owner_repo}#{number}"),
            Self::Slack {
                channel_id,
                thread_ts,
                ..
            } => format!("{channel_id}/{thread_ts}"),
            Self::SlackWindow { id, .. } => id.clone(),
            Self::Note { id } | Self::Task { id } => id.clone(),
        }
    }

    /// The canonical, presentation-free link copied to the clipboard.
    pub fn canonical_url(&self) -> String {
        match self {
            Self::Clickup { id } => format!("https://app.clickup.com/t/{id}"),
            Self::Github {
                owner_repo,
                number,
                item,
            } => format!(
                "https://github.com/{owner_repo}/{}/{}",
                item.as_str(),
                number
            ),
            Self::Slack {
                channel_id,
                thread_ts,
                host,
                permalink_ts,
            } => format!(
                "https://{host}/archives/{channel_id}/p{}?thread_ts={thread_ts}",
                compact_ts(permalink_ts.as_deref().unwrap_or(thread_ts))
            ),
            Self::SlackWindow {
                channel_id, host, ..
            } => format!("https://{host}/archives/{channel_id}"),
            Self::Note { id } => format!("relay://note/{id}"),
            Self::Task { id } => format!("relay://task/{id}"),
        }
    }

    /// The inline chip text. An alias is a channel name or, for an ambiguous short repo
    /// name, the owner that qualifies it. Aliases are presentation only.
    pub fn display_id(&self, alias: Option<&str>) -> String {
        match self {
            Self::Clickup { id } => format!("@CU-{id}"),
            Self::Github {
                owner_repo, number, ..
            } => {
                // Short repo names can collide across owners, so a caller that knows the
                // repo is ambiguous passes the owner as the alias to qualify the display.
                let (_, repo) = owner_repo.rsplit_once('/').unwrap_or(("", owner_repo));
                match alias {
                    Some(a) if !a.is_empty() => format!("@{a}#{number}"),
                    _ => format!("@{repo}#{number}"),
                }
            }
            Self::Slack {
                channel_id,
                thread_ts,
                ..
            } => {
                let seconds = thread_ts.split('.').next().unwrap_or(thread_ts);
                match alias {
                    Some(name) if !name.is_empty() => format!("@#{name}/{seconds}"),
                    _ => format!("@{channel_id}/{seconds}"),
                }
            }
            Self::SlackWindow {
                id,
                channel_id,
                name,
                ..
            } => {
                // The short local id makes two picks of the same conversation distinct chips.
                let short: String = id
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                match name.as_deref().filter(|name| !name.is_empty()).or(alias) {
                    Some(label) => format!("@#{label}/{short}"),
                    None => format!("@{channel_id}/{short}"),
                }
            }
            Self::Note { id } => format!("@N-{id}"),
            Self::Task { id } => format!("@T-{id}"),
        }
    }

    /// Which picker group owns this key.
    pub fn group(&self) -> V2PickerGroup {
        match self {
            Self::Clickup { .. } => V2PickerGroup::Cards,
            Self::Github { item, .. } => match item {
                GithubItemKind::Pull => V2PickerGroup::Prs,
                GithubItemKind::Issue => V2PickerGroup::Issues,
            },
            Self::Slack { .. } => V2PickerGroup::Conversations,
            Self::SlackWindow { .. } => V2PickerGroup::Conversations,
            Self::Note { .. } | Self::Task { .. } => V2PickerGroup::NotesTasks,
        }
    }

    /// The `PreparedReference` #25 persists. A snapshot, when present, is read-only
    /// content for the Lens and never changes identity.
    pub fn to_prepared(&self, snapshot_json: Option<String>) -> PreparedReference {
        PreparedReference {
            kind: self.kind().to_string(),
            target_id: self.target_id(),
            canonical_url: self.canonical_url(),
            snapshot_json,
        }
    }

    /// Parse a single recognised URL. Query strings and fragments are presentation only
    /// and are stripped after identity is extracted; Slack's `thread_ts` is the exception,
    /// where a reply permalink names its parent thread.
    pub fn parse_url(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let (base, query) = match raw.split_once('?') {
            Some((base, query)) => (base, Some(query)),
            None => (raw, None),
        };
        let base = base.split('#').next().unwrap_or(base);

        if let Some(rest) = base.strip_prefix("https://github.com/") {
            let segments: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
            if segments.len() < 4 {
                return None;
            }
            let (owner, repo, item, number) = (segments[0], segments[1], segments[2], segments[3]);
            if owner.is_empty() || repo.is_empty() {
                return None;
            }
            let item = match item {
                "pull" => GithubItemKind::Pull,
                "issues" => GithubItemKind::Issue,
                _ => return None,
            };
            let number = number.parse::<u64>().ok()?;
            if number == 0 {
                return None;
            }
            return Some(Self::Github {
                owner_repo: format!(
                    "{}/{}",
                    owner.to_ascii_lowercase(),
                    repo.to_ascii_lowercase()
                ),
                number,
                item,
            });
        }

        if let Some(rest) = base.strip_prefix("https://app.clickup.com/t/") {
            let id = rest.trim_end_matches('/');
            if id.is_empty() || id.contains('/') {
                return None;
            }
            return Some(Self::Clickup { id: id.to_string() });
        }

        if let Some(rest) = base.strip_prefix("https://") {
            let (host, path) = rest.split_once('/')?;
            let host_lower = host.to_ascii_lowercase();
            if !(host_lower == "slack.com" || host_lower.ends_with(".slack.com")) {
                return None;
            }
            let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
            if segments.len() < 3 || segments[0] != "archives" || segments[1].is_empty() {
                return None;
            }
            let channel_id = segments[1];
            let permalink_ts = normalize_ts(segments[2].strip_prefix('p')?)?;
            let thread_ts = query
                .and_then(parse_thread_ts)
                .unwrap_or_else(|| permalink_ts.clone());
            return Some(Self::Slack {
                channel_id: channel_id.to_string(),
                thread_ts,
                host: host.to_string(),
                permalink_ts: Some(permalink_ts),
            });
        }

        None
    }

    /// Parse a typed `@` token (without the leading `@`). A bare `#142` cannot form a
    /// GitHub identity without a repo, so it returns `None`; the picker uses it as an ID
    /// scope instead.
    pub fn parse_typed(token: &str) -> Option<Self> {
        let token = token.trim().trim_end_matches(['.', ',', ';', ':', '!']);
        if let Some(id) = token.strip_prefix("CU-") {
            if !id.is_empty() {
                return Some(Self::Clickup { id: id.to_string() });
            }
        }
        if let Some(id) = token.strip_prefix("N-") {
            if !id.is_empty() {
                return Some(Self::Note { id: id.to_string() });
            }
        }
        if let Some(id) = token.strip_prefix("T-") {
            if !id.is_empty() {
                return Some(Self::Task { id: id.to_string() });
            }
        }
        if let Some((owner_repo, number)) = token.rsplit_once('#') {
            if owner_repo.is_empty() {
                return None;
            }
            let number = number.parse::<u64>().ok()?;
            if number == 0 {
                return None;
            }
            let owner_repo = owner_repo.to_ascii_lowercase();
            // Without a slash the short name is ambiguous; the picker resolves it. Use a
            // neutral repo so the key still deduplicates with the full form after
            // resolution by identity string comparison in the picker query.
            let owner_repo = if owner_repo.contains('/') {
                owner_repo
            } else {
                format!("?/{owner_repo}")
            };
            return Some(Self::Github {
                owner_repo,
                number,
                item: GithubItemKind::Pull,
            });
        }
        if let Some((channel, ts)) = token.split_once('/') {
            if !channel.is_empty() {
                let thread_ts = normalize_ts(ts)?;
                return Some(Self::Slack {
                    channel_id: channel.to_string(),
                    thread_ts,
                    host: "slack.com".to_string(),
                    permalink_ts: None,
                });
            }
        }
        None
    }

    /// Rebuild a key from its canonical identity. The GitHub PR/issue distinction is not in
    /// the identity, so a canonical URL is used when one is available; absent one, a direct
    /// read can still fall back to the issues endpoint, which classifies PRs itself.
    pub fn from_identity(identity: &str, canonical_url: Option<&str>) -> Option<Self> {
        let (kind, rest) = identity.split_once(':')?;
        if rest.is_empty() {
            return None;
        }
        let target_id = if kind == "clickup" {
            format!("CU-{rest}")
        } else {
            rest.to_string()
        };
        Self::from_storage(kind, &target_id, canonical_url)
    }

    /// Rebuild a key from a stored `kind` plus `target_id`, accepting a canonical URL for
    /// the GitHub PR/issue distinction when the snapshot kept one.
    pub fn from_storage(kind: &str, target_id: &str, canonical_url: Option<&str>) -> Option<Self> {
        match kind {
            "clickup" => target_id
                .strip_prefix("CU-")
                .map(|id| Self::Clickup { id: id.to_string() }),
            "github" => {
                let (owner_repo, number) = target_id.rsplit_once('#')?;
                let number = number.parse::<u64>().ok()?;
                let item = canonical_url
                    .and_then(|url| url.split('/').nth(5))
                    .map(|kind| match kind {
                        "issues" => GithubItemKind::Issue,
                        _ => GithubItemKind::Pull,
                    })
                    .unwrap_or(GithubItemKind::Pull);
                Some(Self::Github {
                    owner_repo: owner_repo.to_ascii_lowercase(),
                    number,
                    item,
                })
            }
            "slack" => {
                let (channel_id, thread_ts) = target_id.split_once('/')?;
                let rest = canonical_url.and_then(|url| url.strip_prefix("https://"));
                let host = rest
                    .and_then(|rest| rest.split_once('/'))
                    .map(|(host, _)| host.to_string())
                    .unwrap_or_else(|| "slack.com".to_string());
                // The canonical URL keeps the reply message in the path; restore it as
                // presentation only. The stored target id remains the shared root.
                let permalink_ts = rest
                    .and_then(|rest| rest.split_once('/'))
                    .and_then(|(_, path)| path.split('?').next())
                    .and_then(|path| path.rsplit('/').next())
                    .and_then(|last| last.strip_prefix('p'))
                    .and_then(normalize_ts);
                Some(Self::Slack {
                    channel_id: channel_id.to_string(),
                    thread_ts: normalize_ts(thread_ts)?,
                    host,
                    permalink_ts,
                })
            }
            "slackwindow" => {
                let rest = canonical_url.and_then(|url| url.strip_prefix("https://"));
                let (host, path) = rest
                    .and_then(|rest| rest.split_once('/'))
                    .unwrap_or(("slack.com", ""));
                let channel_id = path.trim_end_matches('/').split('/').nth(1).unwrap_or("");
                if channel_id.is_empty() {
                    return None;
                }
                Some(Self::SlackWindow {
                    id: target_id.to_string(),
                    channel_id: channel_id.to_string(),
                    host: host.to_string(),
                    name: None,
                })
            }
            "note" => Some(Self::Note {
                id: target_id.to_string(),
            }),
            "task" => Some(Self::Task {
                id: target_id.to_string(),
            }),
            _ => None,
        }
    }
}

// Identity is the whole point of the type: implement it once, from `identity()`.
impl PartialEq for ReferenceKey {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}
impl Eq for ReferenceKey {}
impl Hash for ReferenceKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
    }
}

fn compact_ts(ts: &str) -> String {
    ts.replace('.', "")
}

/// Accept `1731000000.123456` or a compact `1731000000123456`, inserting the decimal point
/// before the last six digits. Validation is string-only: no floating-point conversion (which
/// would lose precision on a microsecond timestamp) and no parse into a lossy numeric type.
fn normalize_ts(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if let Some((seconds, micros)) = raw.split_once('.') {
        // Slack epoch seconds are ten digits in this era; microsecond precision is six digits.
        if seconds.len() >= 10
            && seconds.bytes().all(|b| b.is_ascii_digit())
            && micros.len() == 6
            && micros.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(raw.to_string());
        }
        return None;
    }
    if !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if raw.len() >= 16 {
        let split = raw.len() - 6;
        return Some(format!("{}.{}", &raw[..split], &raw[split..]));
    }
    None
}

fn parse_thread_ts(query: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("thread_ts=") {
            if let Some(ts) = normalize_ts(value) {
                if ts.contains('.') {
                    return Some(ts);
                }
            }
        }
    }
    None
}

/// A URL recognised inside free text, with offsets in Rust character indices. The matched
/// `value` excludes surrounding punctuation and is the exact substring a caller replaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlMatch {
    pub start: usize,
    pub end: usize,
    pub value: String,
    pub key: String,
    pub kind: String,
    pub canonical_link: String,
    pub display_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceParseResult {
    pub urls: Vec<UrlMatch>,
}

/// Scan free text for recognised URLs in order. Surrounding punctuation is left in place;
/// unsupported URLs are ignored, so the caller can replace only the returned spans.
pub fn parse_urls(text: &str) -> ReferenceParseResult {
    let mut urls = Vec::new();
    let mut from = 0;
    while let Some(relative) = text[from..].find("https://") {
        let start = from + relative;
        let rest = &text[start..];
        let mut end = rest.len();
        for (offset, ch) in rest.char_indices() {
            if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`') {
                end = offset;
                break;
            }
        }
        let mut raw = &rest[..end];
        loop {
            let Some(ch) = raw.chars().last() else { break };
            if matches!(
                ch,
                '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"'
            ) {
                raw = &raw[..raw.len() - ch.len_utf8()];
            } else {
                break;
            }
        }
        if let Some(key) = ReferenceKey::parse_url(raw) {
            let char_start = text[..start].chars().count();
            let char_end = char_start + raw.chars().count();
            urls.push(UrlMatch {
                start: char_start,
                end: char_end,
                value: raw.to_string(),
                key: key.identity(),
                kind: key.kind().to_string(),
                canonical_link: key.canonical_url(),
                display_id: key.display_id(None),
            });
        }
        from = start + 8;
    }
    ReferenceParseResult { urls }
}

/// Whether a token identifies a known key immediately, without a provider lookup.
pub fn known_key(token: &str) -> Option<ReferenceKey> {
    let trimmed = token.strip_prefix('@').unwrap_or(token);
    if let Some(url) = ReferenceKey::parse_url(trimmed) {
        return Some(url);
    }
    ReferenceKey::parse_typed(trimmed)
}

/// The read-only Lens snapshot. Producers (#30-#33) write a superset of these fields into
/// `snapshot_json`; unknown fields are ignored and a plain string becomes `content`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReferenceSnapshot {
    pub title: Option<String>,
    pub state: Option<String>,
    pub metadata: Vec<String>,
    pub cached_at: Option<String>,
    pub reason: Option<String>,
    pub content: Option<String>,
    pub channel_name: Option<String>,
}

impl ReferenceSnapshot {
    pub fn from_json(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_else(|_| Self {
            content: Some(raw.to_string()),
            ..Self::default()
        })
    }

    /// A provider read writes the read-only Lens fields plus the local fetch time. The
    /// provider's own update time goes into metadata; `cached_at` is always Relay's clock.
    pub fn from_provider(
        title: impl Into<String>,
        state: impl Into<String>,
        metadata: Vec<String>,
        cached_at: impl Into<String>,
    ) -> Self {
        Self {
            title: Some(title.into()),
            state: Some(state.into()),
            metadata: metadata.into_iter().take(3).collect(),
            cached_at: Some(cached_at.into()),
            reason: None,
            content: None,
            channel_name: None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn cached_at_ms(&self) -> Option<i64> {
        let raw = self.cached_at.as_deref()?;
        let parsed =
            time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
                .ok()?;
        Some((parsed.unix_timestamp_nanos() / 1_000_000) as i64)
    }
}

/// A Reference needs background recovery when it has no snapshot, carries an unreadable
/// reason, or its copy is older than `stale_after_ms`. Notes and Tasks never lose their link;
/// this only schedules a refresh of the read-only content.
pub fn snapshot_needs_recovery(
    snapshot: Option<&ReferenceSnapshot>,
    now_ms: i64,
    stale_after_ms: i64,
) -> bool {
    match snapshot {
        None => true,
        Some(snapshot) if snapshot.reason.is_some() => true,
        Some(snapshot) => snapshot
            .cached_at_ms()
            .is_none_or(|at| now_ms.saturating_sub(at) >= stale_after_ms),
    }
}

/// Availability is separate from identity: a known target stays linked while unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceAvailability {
    Ready,
    Unavailable,
}

/// The five picker groups, in the order the spec fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum V2PickerGroup {
    Cards,
    Prs,
    Issues,
    Conversations,
    NotesTasks,
}

impl V2PickerGroup {
    pub fn rank(self) -> u8 {
        match self {
            Self::Cards => 0,
            Self::Prs => 1,
            Self::Issues => 2,
            Self::Conversations => 3,
            Self::NotesTasks => 4,
        }
    }
}

/// One shared row contract for the picker and its Lens. A readable row carries a title,
/// state and up to three metadata lines; an unavailable row with a snapshot shows its
/// content and cache age, and without one shows the id, reason and Settings destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2PickerRow {
    pub key: String,
    pub kind: String,
    pub display_id: String,
    pub canonical_link: String,
    pub availability: ReferenceAvailability,
    pub group: V2PickerGroup,
    pub title: String,
    pub state: Option<String>,
    pub metadata: Vec<String>,
    pub snapshot: Option<ReferenceSnapshot>,
    pub unavailable_reason: Option<String>,
    pub settings_destination: Option<String>,
    pub updated_at: String,
    /// Exact title/identity match ranks ahead of recency inside a group.
    pub exact_match: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_ignores_presentation_but_not_target() {
        let a = ReferenceKey::Github {
            owner_repo: "Sample/App".to_ascii_lowercase(),
            number: 142,
            item: GithubItemKind::Pull,
        };
        let issue = ReferenceKey::Github {
            owner_repo: "sample/app".into(),
            number: 142,
            item: GithubItemKind::Issue,
        };
        assert_eq!(a, issue);
        assert_eq!(a.identity(), "github:sample/app#142");
        let slack_a = ReferenceKey::Slack {
            channel_id: "C1".into(),
            thread_ts: "1731000000.000001".into(),
            host: "acme.slack.com".into(),
            permalink_ts: Some("1731000000.000001".into()),
        };
        let slack_b = ReferenceKey::Slack {
            channel_id: "C1".into(),
            thread_ts: "1731000000.000001".into(),
            host: "other.slack.com".into(),
            // A reply permalink has the same root but a different message in the path.
            permalink_ts: Some("1731000000.000099".into()),
        };
        assert_eq!(slack_a, slack_b);
        assert_ne!(
            slack_a.canonical_url(),
            slack_b.canonical_url(),
            "the permalink is presentation, not identity"
        );
        assert_ne!(a, slack_a);
    }

    #[test]
    fn github_urls_normalize_and_keep_the_item_kind() {
        let pr = ReferenceKey::parse_url("https://github.com/Sample/App/pull/142").unwrap();
        assert_eq!(
            pr,
            ReferenceKey::Github {
                owner_repo: "sample/app".into(),
                number: 142,
                item: GithubItemKind::Pull,
            }
        );
        assert_eq!(pr.canonical_url(), "https://github.com/sample/app/pull/142");
        assert_eq!(pr.display_id(None), "@app#142");
        let issue = ReferenceKey::parse_url("https://github.com/Sample/App/issues/7").unwrap();
        assert_eq!(
            issue.canonical_url(),
            "https://github.com/sample/app/issues/7"
        );
        assert_eq!(issue.group(), V2PickerGroup::Issues);
        assert!(ReferenceKey::parse_url("https://github.com/Sample/App/pull/0").is_none());
    }

    #[test]
    fn clickup_preserves_case_and_display_prefix() {
        let key = ReferenceKey::parse_url("https://app.clickup.com/t/8x1K2").unwrap();
        assert_eq!(key.identity(), "clickup:8x1K2");
        assert_eq!(key.display_id(None), "@CU-8x1K2");
        assert_eq!(key.canonical_url(), "https://app.clickup.com/t/8x1K2");
    }

    #[test]
    fn slack_reply_permalink_prefers_thread_ts_and_preserves_host() {
        let key = ReferenceKey::parse_url(
            "https://acme.slack.com/archives/C123/p1731000000000099?thread_ts=1731000000.000001&cid=C123",
        )
        .unwrap();
        assert_eq!(key.identity(), "slack:C123/1731000000.000001");
        // The reply's own message timestamp stays in the copied permalink; the root still
        // travels as `thread_ts` and drives identity, so both links share one cache entry.
        assert_eq!(
            key.canonical_url(),
            "https://acme.slack.com/archives/C123/p1731000000000099?thread_ts=1731000000.000001"
        );
        assert_eq!(key.display_id(Some("general")), "@#general/1731000000");
        assert!(ReferenceKey::parse_url("https://example.com/archives/C1/p123").is_none());
    }

    #[test]
    fn slack_parent_and_reply_permalinks_share_the_root_and_malformed_stays_text() {
        let parent =
            ReferenceKey::parse_url("https://acme.slack.com/archives/C01/p1731000000000001")
                .unwrap();
        let reply = ReferenceKey::parse_url(
            "https://acme.slack.com/archives/C01/p1731000000000099?thread_ts=1731000000.000001&cid=C01",
        )
        .unwrap();
        assert_eq!(parent, reply);
        assert_eq!(parent.identity(), "slack:C01/1731000000.000001");
        // The reply permalink is preserved for copy and navigation while identity stays the same.
        assert!(
            reply
                .canonical_url()
                .contains("thread_ts=1731000000.000001")
        );
        assert!(reply.canonical_url().contains("/p1731000000000099?"));
        assert!(ReferenceKey::parse_url("https://acme.slack.com/archives/C01/pnot-a-ts").is_none());
        assert!(
            ReferenceKey::parse_url("https://acme.slack.com/archives/C01/p1731000000.12345")
                .is_none()
        );
        assert_eq!(
            parse_urls("see https://acme.slack.com/archives/C01/pnot-a-ts now")
                .urls
                .len(),
            0
        );
    }

    #[test]
    fn parse_urls_trims_punctuation_and_leaves_unsupported_text() {
        let parsed = parse_urls(
            "see (https://github.com/Sample/App/pull/142), then https://app.clickup.com/t/AB. \
             keep https://example.com/x",
        );
        assert_eq!(parsed.urls.len(), 2);
        assert_eq!(
            parsed.urls[0].value,
            "https://github.com/Sample/App/pull/142"
        );
        assert_eq!(parsed.urls[1].value, "https://app.clickup.com/t/AB");
        assert!(parsed.urls.iter().all(|m| !m.value.contains(',')));
    }

    #[test]
    fn typed_tokens_resolve_without_a_provider() {
        assert_eq!(known_key("@CU-8x1").unwrap().identity(), "clickup:8x1");
        assert_eq!(known_key("@N-12").unwrap().kind(), "note");
        assert_eq!(known_key("@#142"), None);
        assert_eq!(
            known_key("@sample/app#142").unwrap().identity(),
            "github:sample/app#142"
        );
    }

    #[test]
    fn snapshots_tolerate_missing_fields_and_plain_text() {
        let snapshot = ReferenceSnapshot::from_json(r#"{"title":"Fix chips","metadata":["repo"]}"#);
        assert_eq!(snapshot.title.as_deref(), Some("Fix chips"));
        assert_eq!(snapshot.metadata, vec!["repo"]);
        assert_eq!(snapshot.state, None);
        let plain = ReferenceSnapshot::from_json("cached body");
        assert_eq!(plain.content.as_deref(), Some("cached body"));
    }
}
