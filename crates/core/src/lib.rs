//! Scanner core: everything the app does that is not drawing pixels.
//!
//! This crate deliberately has no GUI dependency so it runs under `cargo test`
//! with no display attached. v1 had no test surface at all because every piece
//! of logic reached into the Qt main window.

pub mod api;
pub mod config;
pub mod engine;
pub mod event;
pub mod geo;
pub mod images;
pub mod logsrc;
pub mod tls;

pub use api::{ApiClient, ApiError, Room};
pub use config::Config;
pub use engine::Scanner;
pub use event::{Event, Status};
pub use images::{DecodedImage, ImageError};
pub use logsrc::{LogEvent, Parser, Tailer};
