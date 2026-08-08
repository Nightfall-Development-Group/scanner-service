//! Everything the UI draws, and the only place it reads from.
//!
//! The UI is a pure function of this struct. Background work never mutates it
//! directly — events arrive on a channel and are folded in once per frame, which
//! is what makes "widget disagrees with model" unrepresentable. v1's equivalent
//! state lived as ad-hoc attributes on the Qt window, initialised in three places
//! and mutated from worker threads.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use scanner_core::api::{Room, RoomImage};
use scanner_core::event::{Event, Status};
use scanner_core::geo::Location;

/// Console scrollback is capped by dropping the oldest line, which is O(1).
///
/// v1 rebuilt its entire console document on every message
/// (`setText(toPlainText()[-5000:] + msg)`), making logging quadratic over a
/// session and destroying the user's text selection each time.
const MAX_CONSOLE_LINES: usize = 500;
const MAX_DEBUG_LINES: usize = 2000;

/// A room the player entered. `room` is `None` when the corpus has no record,
/// which is a normal outcome rather than a failure.
#[derive(Debug, Clone)]
pub struct RoomEntry {
    pub name: String,
    pub room: Option<Room>,
}

impl RoomEntry {
    /// What to show as the heading: the corpus's display casing when we have it,
    /// otherwise the raw name from the log.
    pub fn title(&self) -> &str {
        match &self.room {
            Some(r) => &r.case_name,
            None => &self.name,
        }
    }

    pub fn is_documented(&self) -> bool {
        self.room.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub ip: String,
    pub location: Option<Location>,
}

impl ServerInfo {
    pub fn describe(&self) -> String {
        match &self.location {
            Some(l) => l.describe(),
            None => "Unknown location".to_string(),
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    pub status: Status,
    /// The room the player is in now.
    pub current: Option<RoomEntry>,
    /// Rooms this run, oldest first. Cleared when a new run starts.
    pub history: Vec<RoomEntry>,
    pub server: Option<ServerInfo>,
    pub console: VecDeque<String>,
    pub debug: VecDeque<String>,
    /// Most recent warning, shown inline until superseded.
    pub warning: Option<String>,
    /// File currently being followed, for the status bar.
    pub watching: Option<String>,
    /// Which of `current`'s images the carousel is showing.
    pub current_image_index: usize,
    /// When the carousel last changed image, manually or automatically. `None`
    /// right after entering a room, so auto-rotate waits a full interval from
    /// now rather than from whenever the previous room happened to rotate.
    last_image_rotate: Option<Instant>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            current: None,
            history: Vec::new(),
            server: None,
            console: VecDeque::new(),
            debug: VecDeque::new(),
            warning: None,
            watching: None,
            current_image_index: 0,
            last_image_rotate: None,
        }
    }
}

impl AppState {
    /// Apply one event. The only way this struct changes.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Status(s) => {
                // A run that recovers should clear a stale error banner.
                if !matches!(s, Status::Stopped { .. }) {
                    self.warning = None;
                }
                self.status = s;
            }

            Event::Log(line) => push_capped(&mut self.console, line, MAX_CONSOLE_LINES),
            Event::Debug(line) => push_capped(&mut self.debug, line, MAX_DEBUG_LINES),

            Event::RunStarted => {
                self.history.clear();
                self.current = None;
            }

            Event::RoomEntered { name } => {
                // Show the name immediately; the record fills in when it lands.
                self.current = Some(RoomEntry { name, room: None });
                self.current_image_index = 0;
                self.last_image_rotate = None;
            }

            Event::RoomResolved { name, room } => {
                let entry = RoomEntry { name, room: *room };
                // Only overwrite `current` if the player has not already moved
                // on. Out-of-order resolution must not rewind the display.
                if self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.name == entry.name && c.room.is_none())
                {
                    self.current = Some(entry.clone());
                }
                self.history.push(entry);
            }

            Event::ServerLocated { ip, location } => {
                self.server = Some(ServerInfo {
                    ip,
                    location: *location,
                });
            }

            Event::WatchingFile(path) => {
                self.watching = Some(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                );
            }

            Event::Warning(w) => {
                push_capped(&mut self.debug, format!("warning: {w}"), MAX_DEBUG_LINES);
                self.warning = Some(w);
            }

            // Handled by ScannerApp directly, which owns the GPU texture
            // cache and so must run `ctx.load_texture` itself — these never
            // reach here in the running app, but the match must stay
            // exhaustive so a future event variant cannot be silently missed.
            Event::ImageReady { .. } | Event::ImageFailed { .. } => {}
        }
    }

    /// Rooms this run, newest first, for the history list.
    pub fn recent(&self) -> impl Iterator<Item = &RoomEntry> {
        self.history.iter().rev()
    }

    pub fn is_scanning(&self) -> bool {
        matches!(self.status, Status::Watching | Status::Searching)
    }

    /// How many images the room on screen has, ordered for display.
    fn ordered_images(&self) -> Vec<&RoomImage> {
        self.current
            .as_ref()
            .and_then(|c| c.room.as_ref())
            .map(Room::ordered_images)
            .unwrap_or_default()
    }

    pub fn image_count(&self) -> usize {
        self.ordered_images().len()
    }

    /// The image the carousel should be showing right now, if any.
    pub fn current_image(&self) -> Option<&RoomImage> {
        let images = self.ordered_images();
        if images.is_empty() {
            return None;
        }
        // Defensive clamp: normally kept in range by `next_image`/`prev_image`,
        // but a room re-resolving with fewer images than before must not index
        // out of bounds.
        let index = self.current_image_index.min(images.len() - 1);
        images.into_iter().nth(index)
    }

    /// Step the carousel forward, wrapping. A no-op with zero or one image.
    pub fn next_image(&mut self, now: Instant) {
        let n = self.image_count();
        if n == 0 {
            return;
        }
        self.current_image_index = (self.current_image_index + 1) % n;
        self.last_image_rotate = Some(now);
    }

    /// Step the carousel backward, wrapping.
    pub fn prev_image(&mut self, now: Instant) {
        let n = self.image_count();
        if n == 0 {
            return;
        }
        self.current_image_index = (self.current_image_index + n - 1) % n;
        self.last_image_rotate = Some(now);
    }

    /// Advance to the next image if `interval` has elapsed since the carousel
    /// last changed — manually or automatically — and there is more than one
    /// image to rotate through. Returns whether it rotated, so tests (and, in
    /// principle, callers) can observe it without inspecting private state.
    ///
    /// `now` is a parameter rather than read from the clock internally so this
    /// is testable without sleeping — the same pattern used for the API
    /// client's rate limiter.
    pub fn maybe_auto_rotate(&mut self, now: Instant, interval: Duration) -> bool {
        if self.image_count() <= 1 {
            return false;
        }
        match self.last_image_rotate {
            // No baseline yet (just entered this room): start the clock now
            // rather than rotating immediately, so a freshly entered room
            // shows its first image for a full interval like every other one.
            None => {
                self.last_image_rotate = Some(now);
                false
            }
            Some(last) if now.duration_since(last) >= interval => {
                self.next_image(now);
                true
            }
            Some(_) => false,
        }
    }
}

fn push_capped(buffer: &mut VecDeque<String>, line: String, cap: usize) {
    buffer.push_back(line);
    while buffer.len() > cap {
        buffer.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(name: &str) -> Event {
        Event::RoomResolved {
            name: name.into(),
            room: Box::new(None),
        }
    }

    #[test]
    fn a_run_start_clears_the_previous_run() {
        let mut s = AppState::default();
        s.apply(Event::RoomEntered { name: "A".into() });
        s.apply(resolved("A"));
        assert_eq!(s.history.len(), 1);

        s.apply(Event::RunStarted);
        assert!(s.history.is_empty());
        assert!(s.current.is_none());
    }

    #[test]
    fn entering_shows_the_name_before_the_record_arrives() {
        let mut s = AppState::default();
        s.apply(Event::RoomEntered {
            name: "Straight2".into(),
        });

        let current = s.current.as_ref().expect("shown immediately");
        assert_eq!(current.title(), "Straight2");
        assert!(!current.is_documented());
    }

    #[test]
    fn a_late_resolution_does_not_rewind_the_display() {
        // The player moved on before the first lookup returned. Applying the
        // stale result to `current` would show the wrong room.
        let mut s = AppState::default();
        s.apply(Event::RoomEntered { name: "A".into() });
        s.apply(Event::RoomEntered { name: "B".into() });
        s.apply(resolved("A"));

        assert_eq!(s.current.as_ref().unwrap().name, "B");
        assert_eq!(s.history.len(), 1, "A is still recorded in history");
    }

    #[test]
    fn console_scrollback_is_bounded() {
        // v1 grew this without bound and rebuilt the whole document per line.
        let mut s = AppState::default();
        for i in 0..MAX_CONSOLE_LINES + 100 {
            s.apply(Event::Log(format!("line {i}")));
        }
        assert_eq!(s.console.len(), MAX_CONSOLE_LINES);
        assert!(
            s.console.front().unwrap().contains("line 100"),
            "oldest lines are dropped, newest kept"
        );
    }

    #[test]
    fn a_warning_is_surfaced_and_also_logged_to_debug() {
        let mut s = AppState::default();
        s.apply(Event::Warning("lookup failed".into()));

        assert_eq!(s.warning.as_deref(), Some("lookup failed"));
        assert!(s.debug.back().unwrap().contains("lookup failed"));
    }

    #[test]
    fn recovering_clears_a_stale_warning() {
        let mut s = AppState::default();
        s.apply(Event::Warning("transient".into()));
        s.apply(Event::Status(Status::Watching));
        assert!(s.warning.is_none());
    }

    #[test]
    fn a_stop_keeps_the_warning_visible() {
        // Stopped means the user must act, so the reason must not be cleared.
        let mut s = AppState::default();
        s.apply(Event::Warning("bad key".into()));
        s.apply(Event::Status(Status::Stopped {
            reason: "bad key".into(),
        }));
        assert!(s.warning.is_some());
    }

    #[test]
    fn history_is_newest_first_for_display() {
        let mut s = AppState::default();
        for n in ["A", "B", "C"] {
            s.apply(resolved(n));
        }
        let names: Vec<_> = s.recent().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["C", "B", "A"]);
    }

    #[test]
    fn watching_shows_just_the_filename() {
        let mut s = AppState::default();
        s.apply(Event::WatchingFile("/long/path/to/player.log".into()));
        assert_eq!(s.watching.as_deref(), Some("player.log"));
    }

    fn image(position: i32) -> RoomImage {
        RoomImage {
            id: position as i64,
            image_url: format!("https://cdn.example/{position}.webp"),
            object_key: format!("room/{position}.webp"),
            position,
            caption: None,
            is_primary: position == 0,
            uploaded_by: None,
            uploaded_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn room_with_images(n: i32) -> Room {
        Room {
            room_name: "r".into(),
            case_name: "R".into(),
            slug: "r".into(),
            description: String::new(),
            roomtype: "Unclassified".into(),
            documented_by: None,
            last_edited_by: None,
            documented_at: "2026-01-01T00:00:00Z".into(),
            last_edited_at: "2026-01-01T00:00:00Z".into(),
            version: 1,
            edit_reason: None,
            soft_deleted: false,
            scan_state: None,
            images: (0..n).map(image).collect(),
            tags: Vec::new(),
            contributor: None,
            attributes: None,
        }
    }

    fn with_images(n: i32) -> AppState {
        let mut s = AppState::default();
        s.apply(Event::RoomEntered { name: "r".into() });
        s.apply(Event::RoomResolved {
            name: "r".into(),
            room: Box::new(Some(room_with_images(n))),
        });
        s
    }

    #[test]
    fn no_current_room_has_no_images() {
        let s = AppState::default();
        assert_eq!(s.image_count(), 0);
        assert!(s.current_image().is_none());
    }

    #[test]
    fn next_and_prev_wrap_around() {
        let mut s = with_images(3);
        let now = Instant::now();

        assert_eq!(s.current_image().unwrap().position, 0);
        s.next_image(now);
        assert_eq!(s.current_image().unwrap().position, 1);
        s.next_image(now);
        s.next_image(now);
        assert_eq!(s.current_image().unwrap().position, 0, "wraps forward");

        s.prev_image(now);
        assert_eq!(s.current_image().unwrap().position, 2, "wraps backward");
    }

    #[test]
    fn a_single_image_does_not_move() {
        let mut s = with_images(1);
        s.next_image(Instant::now());
        assert_eq!(s.current_image_index, 0);
    }

    #[test]
    fn zero_images_do_not_panic_on_navigation() {
        let mut s = with_images(0);
        s.next_image(Instant::now());
        s.prev_image(Instant::now());
        assert!(s.current_image().is_none());
    }

    #[test]
    fn entering_a_new_room_resets_the_carousel() {
        let mut s = with_images(3);
        s.next_image(Instant::now());
        assert_eq!(s.current_image_index, 1);

        s.apply(Event::RoomEntered {
            name: "other".into(),
        });
        assert_eq!(s.current_image_index, 0, "index resets for the new room");
    }

    #[test]
    fn auto_rotate_waits_for_the_interval() {
        let mut s = with_images(3);
        let t0 = Instant::now();
        let interval = Duration::from_secs(5);

        assert!(
            !s.maybe_auto_rotate(t0, interval),
            "nothing has elapsed yet"
        );
        assert_eq!(s.current_image_index, 0);

        let rotated = s.maybe_auto_rotate(t0 + interval, interval);
        assert!(rotated);
        assert_eq!(s.current_image_index, 1);
    }

    #[test]
    fn auto_rotate_does_nothing_with_one_or_no_images() {
        let mut s = with_images(1);
        let t0 = Instant::now();
        assert!(!s.maybe_auto_rotate(t0 + Duration::from_secs(999), Duration::from_secs(1)));
    }

    #[test]
    fn manual_navigation_resets_the_auto_rotate_clock() {
        // Otherwise a manual click right before the auto-rotate is due would
        // immediately be followed by an automatic one, jumping twice at once.
        let mut s = with_images(3);
        let t0 = Instant::now();
        let interval = Duration::from_secs(5);

        s.next_image(t0);
        assert!(
            !s.maybe_auto_rotate(t0 + Duration::from_millis(1), interval),
            "the manual step just reset the clock"
        );
    }

    #[test]
    fn an_index_beyond_the_image_count_clamps_instead_of_panicking() {
        // `current_image_index` is public so the UI can read it for a counter
        // like "3 / 6"; being public also means it can be set out of range by
        // mistake, and `current_image` must not panic if that happens.
        let mut s = with_images(3);
        s.current_image_index = 999;
        assert_eq!(
            s.current_image().unwrap().position,
            2,
            "clamped to the last image"
        );
    }
}
