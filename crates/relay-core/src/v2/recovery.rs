//! Persistent recovery policy for unreadable References. The store owns the rows; this module
//! owns the schedule and the reason mapping so retries behave the same in every surface.

use serde::{Deserialize, Serialize};

use crate::provider::ProviderFailure;

/// 30 seconds, 2 minutes, 10 minutes, then 30 minutes capped.
pub const RETRY_SCHEDULE_MS: [i64; 4] = [30_000, 120_000, 600_000, 1_800_000];
/// A snapshot older than this is refreshed in the background even while still readable.
pub const STALE_SNAPSHOT_MS: i64 = 24 * 60 * 60 * 1000;
/// The worker never holds more than this many live reads in flight at once.
pub const MAX_IN_FLIGHT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    Transient,
    Unknown,
    RateLimited,
    SignIn,
    Configuration,
}

impl RecoveryReason {
    pub fn from_failure(failure: &ProviderFailure) -> Self {
        match failure {
            ProviderFailure::SignIn => Self::SignIn,
            ProviderFailure::Configuration => Self::Configuration,
            ProviderFailure::RateLimited { .. } => Self::RateLimited,
            ProviderFailure::NotFound => Self::Unknown,
            ProviderFailure::Permission => Self::Unknown,
            ProviderFailure::Unavailable => Self::Transient,
        }
    }

    /// Sign-in and configuration failures pause until the user changes Settings or a
    /// refreshed external session; unknown unreadability retries on the normal schedule.
    pub fn pauses(&self) -> bool {
        matches!(self, Self::SignIn | Self::Configuration)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Unknown => "unknown",
            Self::RateLimited => "rate_limited",
            Self::SignIn => "sign_in",
            Self::Configuration => "configuration",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "transient" => Some(Self::Transient),
            "unknown" => Some(Self::Unknown),
            "rate_limited" => Some(Self::RateLimited),
            "sign_in" => Some(Self::SignIn),
            "configuration" => Some(Self::Configuration),
            _ => None,
        }
    }
}

/// A Reference the worker may attempt. All SQLite reads use this owned shape so the
/// connection is released before any network await.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryTarget {
    pub key: String,
    pub kind: String,
    pub target_id: String,
    pub canonical_url: String,
    pub provider: String,
    pub attempts: i64,
    pub next_due_at: String,
    pub next_due_ms: i64,
    pub reason: Option<RecoveryReason>,
    pub paused: bool,
    pub paused_config: Option<String>,
    pub updated_at: String,
}

/// The identity fields needed to create the one recovery row a shared key owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRecovery {
    pub key: String,
    pub kind: String,
    pub target_id: String,
    pub canonical_url: String,
    pub provider: String,
}

/// The decision a worker applies through `V2Store::record_recovery`. Keeping it pure makes the
/// backoff, pause and stale-completion rules testable without a database or a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryUpdate {
    pub clear: bool,
    pub attempts: i64,
    pub next_due_at: String,
    pub next_due_ms: i64,
    pub reason: Option<RecoveryReason>,
    pub paused: bool,
    pub paused_config: Option<String>,
}

/// `attempts_after` is the number of failures including this one. A provider rate-limit
/// deadline later than the backoff wins, so Relay never retries before the provider allows.
pub fn next_delay_ms(attempts_after: i64, rate_limit_until_ms: Option<i64>, now_ms: i64) -> i64 {
    let index = (attempts_after - 1).clamp(0, RETRY_SCHEDULE_MS.len() as i64 - 1) as usize;
    let mut due = now_ms.saturating_add(RETRY_SCHEDULE_MS[index]);
    if let Some(limit) = rate_limit_until_ms {
        due = due.max(limit);
    }
    due
}

pub fn decide_failure(
    target: Option<&RecoveryTarget>,
    failure: &ProviderFailure,
    now_ms: i64,
    config_version: &str,
) -> RecoveryUpdate {
    let reason = RecoveryReason::from_failure(failure);
    let attempts = target.map(|t| t.attempts.saturating_add(1)).unwrap_or(1);
    let rate_limit_until_ms = match failure {
        ProviderFailure::RateLimited { retry_after_ms } => *retry_after_ms,
        _ => None,
    };
    let next_due_ms = next_delay_ms(attempts, rate_limit_until_ms, now_ms);
    let next_due_at =
        crate::model::from_unix_millis(next_due_ms).unwrap_or_else(|| crate::model::now());
    RecoveryUpdate {
        clear: false,
        attempts,
        next_due_at,
        next_due_ms,
        paused: reason.pauses(),
        paused_config: reason.pauses().then(|| config_version.to_string()),
        reason: Some(reason),
    }
}

pub fn decide_success() -> RecoveryUpdate {
    RecoveryUpdate {
        clear: true,
        attempts: 0,
        next_due_at: String::new(),
        next_due_ms: 0,
        reason: None,
        paused: false,
        paused_config: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_uses_the_four_fixed_steps_and_caps() {
        let now = 1_000_000;
        assert_eq!(next_delay_ms(1, None, now), now + 30_000);
        assert_eq!(next_delay_ms(2, None, now), now + 120_000);
        assert_eq!(next_delay_ms(3, None, now), now + 600_000);
        assert_eq!(next_delay_ms(4, None, now), now + 1_800_000);
        assert_eq!(next_delay_ms(9, None, now), now + 1_800_000);
    }

    #[test]
    fn a_rate_limit_deadline_later_than_backoff_wins() {
        let now = 1_000_000;
        assert_eq!(next_delay_ms(1, Some(now + 300_000), now), now + 300_000);
        assert_eq!(next_delay_ms(1, Some(now + 10_000), now), now + 30_000);
    }

    #[test]
    fn sign_in_and_configuration_pause_others_retry() {
        let sign_in = decide_failure(None, &ProviderFailure::SignIn, 1_000_000, "gh:octo");
        assert!(sign_in.paused);
        assert_eq!(sign_in.reason, Some(RecoveryReason::SignIn));
        assert_eq!(sign_in.paused_config.as_deref(), Some("gh:octo"));
        assert_eq!(sign_in.attempts, 1);

        let transient = decide_failure(None, &ProviderFailure::Unavailable, 1_000_000, "v");
        assert!(!transient.paused);
        assert_eq!(transient.reason, Some(RecoveryReason::Transient));
        assert_eq!(transient.next_due_ms, 1_030_000);

        let unknown = decide_failure(None, &ProviderFailure::NotFound, 1_000_000, "v");
        assert_eq!(unknown.reason, Some(RecoveryReason::Unknown));
        assert!(!unknown.paused);
    }

    #[test]
    fn attempts_increase_from_the_existing_target() {
        let target = RecoveryTarget {
            key: "github:acme/relay#7".into(),
            kind: "github".into(),
            target_id: "acme/relay#7".into(),
            canonical_url: "https://github.com/acme/relay/pull/7".into(),
            provider: "github".into(),
            attempts: 2,
            next_due_at: "2026-11-21T00:00:00.000Z".into(),
            next_due_ms: 0,
            reason: Some(RecoveryReason::Transient),
            paused: false,
            paused_config: None,
            updated_at: "2026-11-21T00:00:00.000Z".into(),
        };
        let update = decide_failure(Some(&target), &ProviderFailure::Unavailable, 1_000_000, "v");
        assert_eq!(update.attempts, 3);
        assert_eq!(update.next_due_ms, 1_600_000);
        assert!(decide_success().clear);
    }
}
