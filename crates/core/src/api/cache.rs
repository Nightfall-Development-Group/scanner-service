//! A small LRU cache with per-entry expiry.
//!
//! Conditional requests would be the natural fit here, but db-api implements
//! `304` only on `/api/rooms/export`, which needs `BULK_OPERATIONS` — a VIEW key
//! cannot use it, and `GET /api/rooms/{slug}` returns a full body every time
//! (verified: `If-None-Match` still answers 200). So we cache locally instead.
//!
//! That is a good trade for this workload. Room documentation changes on human
//! timescales, while a player re-enters the same rooms constantly, so a short
//! TTL removes most requests without ever showing meaningfully stale data.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use lru::LruCache;

pub struct TtlCache<K: std::hash::Hash + Eq, V> {
    entries: LruCache<K, (Instant, V)>,
    ttl: Duration,
}

impl<K: std::hash::Hash + Eq, V: Clone> TtlCache<K, V> {
    pub fn new(capacity: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            entries: LruCache::new(capacity),
            ttl,
        }
    }

    /// Fetch if present and unexpired. An expired entry is evicted on contact
    /// rather than lingering until the LRU pushes it out.
    pub fn get(&mut self, key: &K, now: Instant) -> Option<V> {
        match self.entries.peek(key) {
            Some((stored, value)) if now.saturating_duration_since(*stored) < self.ttl => {
                let value = value.clone();
                self.entries.promote(key);
                Some(value)
            }
            Some(_) => {
                self.entries.pop(key);
                None
            }
            None => None,
        }
    }

    pub fn put(&mut self, key: K, value: V, now: Instant) {
        self.entries.put(key, (now, value));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(cap: usize, ttl_secs: u64) -> TtlCache<String, i32> {
        TtlCache::new(
            NonZeroUsize::new(cap).unwrap(),
            Duration::from_secs(ttl_secs),
        )
    }

    #[test]
    fn returns_a_fresh_entry() {
        let now = Instant::now();
        let mut c = cache(4, 60);
        c.put("a".into(), 1, now);
        assert_eq!(c.get(&"a".to_string(), now), Some(1));
    }

    #[test]
    fn expires_an_old_entry() {
        let now = Instant::now();
        let mut c = cache(4, 60);
        c.put("a".into(), 1, now);

        let later = now + Duration::from_secs(61);
        assert_eq!(c.get(&"a".to_string(), later), None);
        assert!(c.is_empty(), "expired entries are dropped, not retained");
    }

    #[test]
    fn evicts_least_recently_used_past_capacity() {
        let now = Instant::now();
        let mut c = cache(2, 60);
        c.put("a".into(), 1, now);
        c.put("b".into(), 2, now);
        c.get(&"a".to_string(), now); // touch "a" so "b" is now the coldest
        c.put("c".into(), 3, now);

        assert_eq!(c.get(&"a".to_string(), now), Some(1));
        assert_eq!(c.get(&"b".to_string(), now), None, "b was coldest");
        assert_eq!(c.get(&"c".to_string(), now), Some(3));
    }

    #[test]
    fn a_miss_is_not_an_error() {
        let now = Instant::now();
        let mut c = cache(2, 60);
        assert_eq!(c.get(&"nope".to_string(), now), None);
    }

    #[test]
    fn bounded_growth_under_a_long_session() {
        // v1's sync window cached image bytes by room name with no eviction at
        // all. This asserts the property that was missing there.
        let now = Instant::now();
        let mut c = cache(8, 600);
        for i in 0..1000 {
            c.put(format!("room{i}"), i, now);
        }
        assert_eq!(c.len(), 8);
    }
}
