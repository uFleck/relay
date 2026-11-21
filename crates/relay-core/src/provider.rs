//! Provider-agnostic read outcomes shared by the GitHub/ClickUp clients and v2 recovery.
//!
//! The clients keep raw provider errors and credentials in Rust. Adapters and the store only
//! ever see these typed failures, which carry no token, header or response body.

use serde::{Deserialize, Serialize};

/// Why a read-only provider call did not produce a result. Configuration failures pause
/// recovery until Settings or a refreshed external session changes; the rest retry on the
/// shared backoff. A rate limit also carries the provider's own deadline when it gave one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailure {
    SignIn,
    Configuration,
    RateLimited { retry_after_ms: Option<i64> },
    Permission,
    NotFound,
    Unavailable,
}

impl ProviderFailure {
    /// Credential or configuration problems pause until conditions change, rather than
    /// burning retry attempts against a provider that keeps rejecting the same setup.
    pub fn is_config(&self) -> bool {
        matches!(self, Self::SignIn | Self::Configuration)
    }

    /// A short, safe reason for the Lens and Settings. Never includes a raw provider error.
    pub fn reason_text(&self) -> &'static str {
        match self {
            Self::SignIn => "Signed out or credentials expired",
            Self::Configuration => "Provider is not connected",
            Self::RateLimited { .. } => "Provider rate limit reached",
            Self::Permission => "No access to this target",
            Self::NotFound => "Target not found",
            Self::Unavailable => "Provider is unavailable",
        }
    }

    /// Where the user can act on this failure.
    pub fn settings_destination(&self, provider: &str) -> String {
        let name = match provider {
            "github" => "GitHub",
            "clickup" => "ClickUp",
            "slack" => "Slack",
            _ => "Providers",
        };
        format!("Settings → {name}")
    }
}

/// Maps a provider HTTP status to a typed failure. `reset_ms` is the provider's rate-limit
/// reset converted to Unix milliseconds, when the response supplied one. The message is
/// inspected only for the rate-limit phrase; it is never returned to a caller.
pub fn failure_from_status(status: u16, message: &str, reset_ms: Option<i64>) -> ProviderFailure {
    match status {
        401 => ProviderFailure::SignIn,
        429 => ProviderFailure::RateLimited {
            retry_after_ms: reset_ms,
        },
        403 => {
            let lower = message.to_ascii_lowercase();
            if reset_ms.is_some()
                || lower.contains("rate limit")
                || lower.contains("secondary rate")
                || lower.contains("abuse")
            {
                ProviderFailure::RateLimited {
                    retry_after_ms: reset_ms,
                }
            } else {
                ProviderFailure::Permission
            }
        }
        404 => ProviderFailure::NotFound,
        500..=599 => ProviderFailure::Unavailable,
        _ => ProviderFailure::Unavailable,
    }
}

/// GitHub sends `x-ratelimit-reset` as Unix seconds; ClickUp's retry hint is seconds from now.
pub fn parse_reset_seconds(raw: Option<&str>) -> Option<i64> {
    let raw = raw?;
    let seconds: i64 = raw.trim().parse().ok()?;
    (seconds > 0).then_some(seconds.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_maps_to_typed_failures() {
        assert_eq!(
            failure_from_status(401, "bad credentials", None),
            ProviderFailure::SignIn
        );
        assert_eq!(
            failure_from_status(403, "resource not accessible", None),
            ProviderFailure::Permission
        );
        assert_eq!(
            failure_from_status(
                403,
                "API rate limit exceeded for your IP",
                Some(1_795_000_000_000)
            ),
            ProviderFailure::RateLimited {
                retry_after_ms: Some(1_795_000_000_000)
            }
        );
        assert_eq!(
            failure_from_status(429, "", Some(5_000)),
            ProviderFailure::RateLimited {
                retry_after_ms: Some(5_000)
            }
        );
        assert_eq!(
            failure_from_status(404, "", None),
            ProviderFailure::NotFound
        );
        assert_eq!(
            failure_from_status(502, "", None),
            ProviderFailure::Unavailable
        );
    }

    #[test]
    fn only_config_failures_pause_and_destinations_are_safe() {
        assert!(ProviderFailure::SignIn.is_config());
        assert!(ProviderFailure::Configuration.is_config());
        assert!(
            !ProviderFailure::RateLimited {
                retry_after_ms: None
            }
            .is_config()
        );
        assert_eq!(
            ProviderFailure::SignIn.settings_destination("github"),
            "Settings → GitHub"
        );
        assert!(!ProviderFailure::SignIn.reason_text().contains("401"));
    }
}
