//! Client for the NFD research database service.
//!
//! The service refuses anonymous access, so every request carries the user's
//! personal API key as `Authorization: Bearer`. Resolving a room from a log line
//! is always two steps — `/lookup` to turn a name into a slug, then `/{slug}` for
//! the record — because slugs are server-assigned and must never be derived
//! locally.

pub mod cache;
pub mod client;
pub mod error;
pub mod limiter;
pub mod model;

pub use client::{ApiClient, DEFAULT_BASE_URL};
pub use error::ApiError;
pub use model::{
    Contributor, LookupCandidate, LookupMatch, LookupResponse, Room, RoomAttributes, RoomImage,
};
