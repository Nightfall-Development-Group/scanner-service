//! Failure modes of the db-api client.
//!
//! The split that matters is retryable versus not. `401`/`403`/`404`/`422` mean
//! the request itself is wrong and will fail identically forever, so retrying
//! them just burns rate-limit budget. `429`, `503` and 5xx are transient.

/// Seconds the server asked us to wait, when it said.
pub type RetryAfter = Option<f64>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// No API key configured. The service has no anonymous access, so this is a
    /// setup problem rather than a request failure.
    #[error("no API key configured; enter one in settings")]
    MissingKey,

    /// 401 — the key is missing or malformed in the header.
    #[error("API key was rejected as malformed")]
    Unauthorized,

    /// 403 — the key is not registered, or lacks the permission level.
    #[error("API key is not registered or lacks permission")]
    Forbidden,

    /// 404 — no room with that slug. Distinct from a lookup returning
    /// `match: "none"`, which is a successful request with no result.
    #[error("no such room")]
    NotFound,

    /// 429 after exhausting retries.
    #[error("rate limited{}", match .retry_after {
        Some(s) => format!("; retry in {s:.0}s"),
        None => String::new(),
    })]
    RateLimited { retry_after: RetryAfter },

    /// 5xx, or 503 when Redis is down, after exhausting retries.
    #[error("service unavailable (HTTP {status})")]
    Unavailable { status: u16 },

    /// An unexpected status we have no specific handling for.
    #[error("unexpected response (HTTP {status})")]
    Unexpected { status: u16 },

    /// DNS, TLS, connection refused, timeout.
    #[error("could not reach the database service: {0}")]
    Transport(String),

    /// The response arrived but did not match the schema. Worth surfacing rather
    /// than swallowing: it means the API changed under us.
    #[error("unexpected response format: {0}")]
    Decode(String),
}

impl ApiError {
    /// Whether trying again could plausibly succeed. Used by the retry loop and
    /// worth exposing so the UI can distinguish "try later" from "fix your key".
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Unavailable { .. } | Self::Transport(_)
        )
    }

    /// Whether this indicates a configuration problem the user must fix.
    pub fn is_auth_problem(&self) -> bool {
        matches!(
            self,
            Self::MissingKey | Self::Unauthorized | Self::Forbidden
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transient_failures() {
        assert!(ApiError::RateLimited { retry_after: None }.is_transient());
        assert!(ApiError::Unavailable { status: 503 }.is_transient());
        assert!(ApiError::Transport("refused".into()).is_transient());

        assert!(!ApiError::NotFound.is_transient());
        assert!(!ApiError::Forbidden.is_transient());
        assert!(!ApiError::MissingKey.is_transient());
    }

    #[test]
    fn classifies_auth_problems() {
        assert!(ApiError::MissingKey.is_auth_problem());
        assert!(ApiError::Unauthorized.is_auth_problem());
        assert!(ApiError::Forbidden.is_auth_problem());
        assert!(!ApiError::NotFound.is_auth_problem());
    }

    #[test]
    fn rate_limit_message_mentions_the_delay() {
        let e = ApiError::RateLimited {
            retry_after: Some(12.0),
        };
        assert!(e.to_string().contains("12s"), "{e}");
    }
}
