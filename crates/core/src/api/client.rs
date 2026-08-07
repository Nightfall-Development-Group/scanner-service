//! HTTP client for db-api.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use super::cache::TtlCache;
use super::error::ApiError;
use super::limiter::TokenBucket;
use super::model::{LookupResponse, Room};

pub const DEFAULT_BASE_URL: &str = "https://db-api.nightfalldivision.com";

/// Room documentation changes on human timescales; a player re-enters rooms
/// within seconds. Ten minutes removes nearly all repeat traffic.
const ROOM_TTL: Duration = Duration::from_secs(600);
/// Name→slug resolution is effectively immutable, but keep it shorter so a newly
/// documented room becomes reachable without a restart.
const LOOKUP_TTL: Duration = Duration::from_secs(300);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ATTEMPTS: u32 = 3;
/// Cap on server-suggested backoff. Without this a hostile or buggy
/// `retry_after` could park the client for an hour.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub struct ApiClient {
    http: reqwest::Client,
    base: Url,
    key: String,
    limiter: Mutex<TokenBucket>,
    rooms: Mutex<TtlCache<String, Room>>,
    lookups: Mutex<TtlCache<String, LookupResponse>>,
}

impl ApiClient {
    pub fn new(api_key: impl Into<String>) -> Result<Self, ApiError> {
        Self::with_base(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base(api_key: impl Into<String>, base: &str) -> Result<Self, ApiError> {
        let key = api_key.into();
        if key.trim().is_empty() {
            return Err(ApiError::MissingKey);
        }
        crate::tls::ensure_provider();
        let base = Url::parse(base).map_err(|e| ApiError::Transport(e.to_string()))?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("nfd-scanner/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ApiError::Transport(e.to_string()))?;

        let now = Instant::now();
        let cap = NonZeroUsize::new(256).expect("nonzero");
        Ok(Self {
            http,
            base,
            key,
            limiter: Mutex::new(TokenBucket::default_at(now)),
            rooms: Mutex::new(TtlCache::new(cap, ROOM_TTL)),
            lookups: Mutex::new(TtlCache::new(cap, LOOKUP_TTL)),
        })
    }

    /// Resolve a room name from the log to its canonical record.
    ///
    /// Returns `Ok(None)` when the name is genuinely not in the corpus, or when
    /// the match was too weak to act on. That is a normal outcome for an
    /// undocumented room, not an error.
    pub async fn resolve_room(&self, name: &str) -> Result<Option<Room>, ApiError> {
        let lookup = self.lookup(name).await?;
        match lookup.resolved_slug() {
            Some(slug) => self.room(slug).await.map(Some),
            None => Ok(None),
        }
    }

    /// `GET /api/rooms/lookup?q=…`
    pub async fn lookup(&self, query: &str) -> Result<LookupResponse, ApiError> {
        let key = query.to_lowercase();
        if let Some(hit) = self.lookups.lock().await.get(&key, Instant::now()) {
            return Ok(hit);
        }

        let mut url = self.endpoint(&["api", "rooms", "lookup"])?;
        url.query_pairs_mut().append_pair("q", query);

        let response: LookupResponse = self.get_json(url).await?;
        self.lookups
            .lock()
            .await
            .put(key, response.clone(), Instant::now());
        Ok(response)
    }

    /// `GET /api/rooms/{slug}`
    ///
    /// `slug` must come from the API. Never pass a locally transformed room name
    /// — the server reserves the right to add disambiguating suffixes no client
    /// can predict.
    pub async fn room(&self, slug: &str) -> Result<Room, ApiError> {
        if let Some(hit) = self
            .rooms
            .lock()
            .await
            .get(&slug.to_string(), Instant::now())
        {
            return Ok(hit);
        }

        let url = self.endpoint(&["api", "rooms", slug])?;
        let room: Room = self.get_json(url).await?;
        self.rooms
            .lock()
            .await
            .put(slug.to_string(), room.clone(), Instant::now());
        Ok(room)
    }

    /// Drop cached data, e.g. when the user changes their API key.
    pub async fn clear_cache(&self) {
        self.rooms.lock().await.clear();
        self.lookups.lock().await.clear();
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, ApiError> {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| ApiError::Transport("base URL cannot have a path".into()))?;
            // Percent-encodes each segment, so a slug with unexpected characters
            // cannot escape into the path.
            path.extend(segments);
        }
        Ok(url)
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T, ApiError> {
        let body = self.get_with_retries(url).await?;
        serde_json::from_str(&body).map_err(|e| ApiError::Decode(e.to_string()))
    }

    async fn get_with_retries(&self, url: Url) -> Result<String, ApiError> {
        let mut last: Option<ApiError> = None;

        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let backoff = match &last {
                    Some(ApiError::RateLimited {
                        retry_after: Some(s),
                    }) => Duration::from_secs_f64(*s),
                    // Exponential: 250ms, 500ms.
                    _ => Duration::from_millis(250 << (attempt - 1)),
                }
                .min(MAX_BACKOFF);
                tokio::time::sleep(backoff).await;
            }

            self.await_slot().await;

            match self.send_once(url.clone()).await {
                Ok(body) => return Ok(body),
                Err(e) if e.is_transient() => {
                    if matches!(e, ApiError::RateLimited { .. }) {
                        self.limiter.lock().await.drain(Instant::now());
                    }
                    last = Some(e);
                }
                // 401/403/404/422 will fail identically forever. Retrying only
                // spends rate-limit budget and delays the error the user needs.
                Err(e) => return Err(e),
            }
        }

        Err(last.unwrap_or(ApiError::Unexpected { status: 0 }))
    }

    /// Block until the local rate limiter allows another request.
    async fn await_slot(&self) {
        loop {
            // Compute under the lock, sleep outside it, so one waiting request
            // does not stall every other caller.
            let wait = self.limiter.lock().await.try_take(Instant::now());
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }

    async fn send_once(&self, url: Url) -> Result<String, ApiError> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.key)
            .send()
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;

        let status = response.status();
        if status.is_success() {
            return response
                .text()
                .await
                .map_err(|e| ApiError::Transport(e.to_string()));
        }

        // Read the body before branching: the 429 detail carries `retry_after`.
        let body = response.text().await.unwrap_or_default();

        Err(match status {
            StatusCode::UNAUTHORIZED => ApiError::Unauthorized,
            StatusCode::FORBIDDEN => ApiError::Forbidden,
            StatusCode::NOT_FOUND => ApiError::NotFound,
            StatusCode::TOO_MANY_REQUESTS => ApiError::RateLimited {
                retry_after: parse_retry_after(&body),
            },
            s if s.is_server_error() => ApiError::Unavailable { status: s.as_u16() },
            s => ApiError::Unexpected { status: s.as_u16() },
        })
    }
}

/// Pull `retry_after` out of a 429 body: `{"detail":{"error":…,"retry_after":12.5}}`.
/// Tolerates the field appearing at the top level too.
fn parse_retry_after(body: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.pointer("/detail/retry_after")
        .or_else(|| v.pointer("/retry_after"))
        .and_then(serde_json::Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_key_is_rejected_up_front() {
        // The API has no anonymous access, so building a client without a key is
        // a configuration error we can catch before any request.
        assert!(matches!(ApiClient::new(""), Err(ApiError::MissingKey)));
        assert!(matches!(ApiClient::new("   "), Err(ApiError::MissingKey)));
    }

    #[test]
    fn builds_encoded_paths() {
        let c = ApiClient::new("k").unwrap();
        let url = c.endpoint(&["api", "rooms", "altiroom"]).unwrap();
        assert_eq!(
            url.as_str(),
            "https://db-api.nightfalldivision.com/api/rooms/altiroom"
        );
    }

    #[test]
    fn a_hostile_slug_cannot_escape_the_path() {
        // Slugs come from the API today, but a path-traversal payload must not
        // be able to redirect the request to another endpoint.
        let c = ApiClient::new("k").unwrap();
        let url = c.endpoint(&["api", "rooms", "../db/keys/list"]).unwrap();
        assert!(
            !url.path().contains("/db/keys/list"),
            "slug escaped into the path: {}",
            url.path()
        );
    }

    #[test]
    fn query_values_are_encoded() {
        let c = ApiClient::new("k").unwrap();
        let mut url = c.endpoint(&["api", "rooms", "lookup"]).unwrap();
        url.query_pairs_mut().append_pair("q", "alti room&x=1");
        let q = url.query().unwrap();
        assert!(q.contains("alti+room") || q.contains("alti%20room"), "{q}");
        assert!(
            !q.contains("&x=1"),
            "ampersand must not split the query: {q}"
        );
    }

    #[test]
    fn reads_retry_after_from_a_429_body() {
        let body = r#"{"detail":{"error":"Rate limit exceeded.","retry_after":12.5}}"#;
        assert_eq!(parse_retry_after(body), Some(12.5));
    }

    #[test]
    fn tolerates_a_top_level_retry_after() {
        assert_eq!(parse_retry_after(r#"{"retry_after":3}"#), Some(3.0));
    }

    #[test]
    fn missing_retry_after_is_not_fatal() {
        assert_eq!(parse_retry_after("{}"), None);
        assert_eq!(parse_retry_after("not json"), None);
    }
}
