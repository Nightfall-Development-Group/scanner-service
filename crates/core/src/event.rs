//! The one channel between background work and the UI.
//!
//! Everything the scanner learns crosses this boundary as a value. No background
//! task holds a widget handle, and no UI code touches scanner state directly —
//! which is precisely the coupling that produced v1's worst defects, where a
//! worker thread called `setText()` and network I/O ran inside a paint handler.

use std::path::PathBuf;

use crate::api::Room;
use crate::geo::Location;
use crate::images::DecodedImage;

/// What the scanner is currently doing. The UI renders this directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not scanning.
    Idle,
    /// Looking for a log file.
    Searching,
    /// Following a log file.
    Watching,
    /// Stopped because of a problem the user must resolve.
    Stopped { reason: String },
}

/// Something that happened. Ordering within the channel is meaningful:
/// `RoomEntered` always precedes the matching `RoomResolved`.
#[derive(Debug, Clone)]
pub enum Event {
    Status(Status),

    /// A line for the user-facing console.
    Log(String),
    /// A line for the debug console. Higher volume, lower level.
    Debug(String),

    /// The player entered a room. Emitted immediately on parsing, before the
    /// network round trip, so the UI can show the name without waiting.
    RoomEntered {
        name: String,
    },

    /// Resolution finished. `room` is `None` when the name is not in the corpus,
    /// which is a normal outcome for an undocumented room rather than an error.
    RoomResolved {
        name: String,
        room: Box<Option<Room>>,
    },

    /// A new run began. The UI should clear the previous run's history.
    RunStarted,

    /// The server address was found, and geolocated if it was routable.
    ServerLocated {
        ip: String,
        location: Box<Option<Location>>,
    },

    /// We started following a different file, either on startup or because the
    /// client rotated to a new one.
    WatchingFile(PathBuf),

    /// A failure worth showing. Recoverable — the scanner keeps running.
    Warning(String),

    /// A room image finished downloading and decoding. The app must confirm
    /// the URL still belongs to the room on screen before creating a texture
    /// from it — the player may already have moved on.
    ImageReady {
        url: String,
        image: Box<DecodedImage>,
    },

    /// An image failed to download or decode. Deliberately not a `Warning`:
    /// one broken image in a gallery is not worth interrupting the user over.
    ImageFailed {
        url: String,
    },
}

impl Event {
    /// Convenience for the common case of a formatted console line.
    pub fn log(message: impl Into<String>) -> Self {
        Self::Log(message.into())
    }

    pub fn debug(message: impl Into<String>) -> Self {
        Self::Debug(message.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_compares_by_value() {
        assert_eq!(Status::Idle, Status::Idle);
        assert_ne!(Status::Idle, Status::Watching);
        assert_eq!(
            Status::Stopped {
                reason: "no key".into()
            },
            Status::Stopped {
                reason: "no key".into()
            }
        );
    }

    #[test]
    fn constructors_take_anything_stringy() {
        assert!(matches!(Event::log("literal"), Event::Log(_)));
        assert!(matches!(Event::debug(format!("{}", 1)), Event::Debug(_)));
    }
}
