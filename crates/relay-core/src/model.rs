//! Shared value helpers for the v2 store and the read-only provider clients.
//!
//! Ids are ULIDs as strings and timestamps are fixed-width RFC 3339 UTC strings. The
//! retired v1 intake model (proposals, episodes, analysis jobs, JEV scores and claims)
//! is gone with the automatic Slack pipeline; v2 keeps its own models under `src/v2/`.

use serde::{Deserialize, Serialize};

/// A new local identifier. ULIDs sort by creation time.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The one timestamp format in the database: fixed-width RFC 3339 UTC with
/// milliseconds. Fixed width matters because values are compared as text in SQLite.
fn format_timestamp(value: time::OffsetDateTime) -> String {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .unwrap_or_default()
}

/// UTC now as fixed-width RFC 3339, the format stored in SQLite.
pub fn now() -> String {
    format_timestamp(time::OffsetDateTime::now_utc())
}

/// Normalizes any RFC 3339 value (including an offset) to the stored format.
pub fn format_time(value: time::OffsetDateTime) -> String {
    format_timestamp(value)
}

/// Converts Unix milliseconds (ClickUp due dates) to the stored RFC 3339 form.
pub fn from_unix_millis(ms: i64) -> Option<String> {
    let seconds = ms.div_euclid(1000);
    let nanos = ms.rem_euclid(1000) * 1_000_000;
    let parsed = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?
        + time::Duration::nanoseconds(nanos);
    Some(format_timestamp(parsed))
}

/// Parses an RFC 3339 timestamp, the only accepted user-supplied time format.
pub fn parse_time(value: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

/// The status categories used by the read-only ClickUp-card classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusCategory {
    Backlog,
    Todo,
    Active,
    Review,
    Merged,
    Deploying,
    Done,
    Blocked,
}

impl StatusCategory {
    pub const ALL: [Self; 8] = [
        Self::Backlog,
        Self::Todo,
        Self::Active,
        Self::Review,
        Self::Merged,
        Self::Deploying,
        Self::Done,
        Self::Blocked,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Review => "review",
            Self::Merged => "merged",
            Self::Deploying => "deploying",
            Self::Done => "done",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }
}

impl std::fmt::Display for StatusCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
