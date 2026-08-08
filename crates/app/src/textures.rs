//! Bounded cache of decoded room images as GPU textures.
//!
//! v1's sync window cached downloaded image bytes per room name with no
//! eviction at all (`sync_window.py:609`), so a long session grew without
//! bound. `egui::TextureHandle` is refcounted, so dropping our copy from the
//! LRU here frees the GPU texture as soon as nothing else references it —
//! eviction is real cleanup, not just forgetting a byte buffer.

use std::num::NonZeroUsize;

use egui::{ColorImage, TextureHandle, TextureOptions};
use scanner_core::images::DecodedImage;

/// Comfortably more than one room's worth of images (the corpus averages
/// ~4.8), so paging back through recently visited rooms in a run stays instant
/// rather than re-downloading.
const CAPACITY: usize = 64;

enum Slot {
    Ready(TextureHandle),
    Failed,
}

pub struct TextureCache {
    slots: lru::LruCache<String, Slot>,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self {
            slots: lru::LruCache::new(NonZeroUsize::new(CAPACITY).expect("nonzero")),
        }
    }
}

impl TextureCache {
    /// Upload a decoded image to the GPU and remember it under `url`.
    pub fn insert(&mut self, ctx: &egui::Context, url: String, image: DecodedImage) {
        let color = ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.rgba,
        );
        let handle = ctx.load_texture(&url, color, TextureOptions::LINEAR);
        self.slots.put(url, Slot::Ready(handle));
    }

    /// Remember that `url` failed, so the UI can show a broken-image state
    /// instead of an indefinite loading spinner.
    pub fn mark_failed(&mut self, url: String) {
        self.slots.put(url, Slot::Failed);
    }

    /// The texture for `url`, once it has finished loading. `None` covers both
    /// "still loading" and "failed" — the caller does not need to tell them
    /// apart to decide whether to show a placeholder instead of an image.
    pub fn get(&mut self, url: &str) -> Option<&TextureHandle> {
        match self.slots.get(&url.to_string()) {
            Some(Slot::Ready(handle)) => Some(handle),
            _ => None,
        }
    }

    /// Whether `url` is known to have failed, for a distinct "broken" message
    /// rather than an indefinite spinner.
    pub fn failed(&mut self, url: &str) -> bool {
        matches!(self.slots.get(&url.to_string()), Some(Slot::Failed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_image() -> DecodedImage {
        DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        }
    }

    #[test]
    fn a_loaded_image_is_retrievable_and_not_marked_failed() {
        let ctx = egui::Context::default();
        let mut cache = TextureCache::default();

        cache.insert(&ctx, "u".into(), tiny_image());

        assert!(cache.get("u").is_some());
        assert!(!cache.failed("u"));
    }

    #[test]
    fn an_unknown_url_is_neither_ready_nor_failed() {
        // This is the "still loading" state: absence from the cache, not a
        // third enum variant, because nothing has been reported about it yet.
        let mut cache = TextureCache::default();
        assert!(cache.get("missing").is_none());
        assert!(!cache.failed("missing"));
    }

    #[test]
    fn a_failure_is_distinct_from_unknown() {
        let mut cache = TextureCache::default();
        cache.mark_failed("broken".into());

        assert!(cache.get("broken").is_none());
        assert!(cache.failed("broken"));
    }

    #[test]
    fn a_later_success_overwrites_an_earlier_failure() {
        // A retry, or the same url appearing in a different room, must not be
        // stuck showing "broken" forever once it actually loads.
        let ctx = egui::Context::default();
        let mut cache = TextureCache::default();

        cache.mark_failed("u".into());
        cache.insert(&ctx, "u".into(), tiny_image());

        assert!(cache.get("u").is_some());
        assert!(!cache.failed("u"));
    }

    #[test]
    fn bounded_capacity_evicts_the_least_recently_used() {
        // The property v1 lacked entirely: a long session must not grow this
        // cache without bound.
        let ctx = egui::Context::default();
        let mut cache = TextureCache::default();

        for i in 0..CAPACITY + 5 {
            cache.insert(&ctx, format!("u{i}"), tiny_image());
        }

        assert!(cache.get("u0").is_none(), "oldest entry was evicted");
        assert!(cache.get(&format!("u{}", CAPACITY + 4)).is_some());
    }
}
