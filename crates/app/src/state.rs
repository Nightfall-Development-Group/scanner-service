//! Everything the UI draws, and the only place it reads from.
//!
//! The UI is a pure function of this struct. Background work never mutates it
//! directly — events arrive on a channel and are folded in once per frame, which
//! is what makes "widget disagrees with model" unrepresentable. v1's equivalent
//! state lived as ad-hoc attributes on the Qt window, initialised in three places
//! and mutated from worker threads.

use std::collections::VecDeque;

use scanner_core::api::Room;
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
        }
    }

    /// Rooms this run, newest first, for the history list.
    pub fn recent(&self) -> impl Iterator<Item = &RoomEntry> {
        self.history.iter().rev()
    }

    pub fn is_scanning(&self) -> bool {
        matches!(self.status, Status::Watching | Status::Searching)
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
}
