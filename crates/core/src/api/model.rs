//! Wire types for db-api.
//!
//! These mirror `RoomResponse` and friends in the database service. Two contract
//! rules are load-bearing and easy to get wrong in a typed language:
//!
//! 1. **Discord snowflakes arrive as JSON strings, not numbers.** `documented_by`,
//!    `last_edited_by` and `uploaded_by` are all `Option<String>`. Typing them as
//!    `u64` would work against most payloads and then fail, because the service
//!    deliberately stringifies them to survive JavaScript's 53-bit integers.
//! 2. **`documented_by` means two different things** depending on where it
//!    appears. On a room it is a snowflake identifying a person; on
//!    [`RoomAttributes`] it is a free-text *source label* naming a program.

use serde::Deserialize;

/// A room record with its images, tags and observed attributes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Room {
    /// Lowercased primary key, e.g. `altiroom?`. Note it can differ from `slug`
    /// by more than case — punctuation is stripped for the slug.
    pub room_name: String,
    /// Display casing, e.g. `AltiRoom?`. This is what the UI should show.
    pub case_name: String,
    /// URL-safe identifier, e.g. `altiroom`. Server-assigned; never derive it.
    pub slug: String,
    pub description: String,
    pub roomtype: String,
    /// Discord snowflake of the documenting user, as a string.
    pub documented_by: Option<String>,
    pub last_edited_by: Option<String>,
    pub documented_at: String,
    pub last_edited_at: String,
    /// Optimistic-concurrency counter, mirrored in the `ETag` header. Only
    /// meaningful for writers, which this client is not.
    pub version: i64,
    pub edit_reason: Option<String>,
    pub soft_deleted: bool,
    /// Harvester bookkeeping: `scanned`, `failed`, `unspawnable`, or absent.
    #[serde(default)]
    pub scan_state: Option<String>,
    #[serde(default)]
    pub images: Vec<RoomImage>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Resolved Discord profile of `documented_by`, when the service could
    /// resolve it.
    #[serde(default)]
    pub contributor: Option<Contributor>,
    /// `None` when nothing has been observed about the room at all.
    #[serde(default)]
    pub attributes: Option<RoomAttributes>,
}

impl Room {
    /// Images ordered for display. The API returns them by `position`, but sort
    /// defensively rather than trusting order across an API change.
    pub fn ordered_images(&self) -> Vec<&RoomImage> {
        let mut v: Vec<&RoomImage> = self.images.iter().collect();
        v.sort_by_key(|i| i.position);
        v
    }

    /// The hero image: the one flagged primary, else the first by position.
    /// Mirrors `primaryImage()` in the website's `room-utils.ts`.
    pub fn primary_image(&self) -> Option<&RoomImage> {
        let ordered = self.ordered_images();
        ordered
            .iter()
            .find(|i| i.is_primary)
            .or(ordered.first())
            .copied()
    }

    /// Whether the room has any documentation worth showing. A room can exist in
    /// the corpus with an empty description and no images.
    pub fn is_documented(&self) -> bool {
        !self.description.trim().is_empty() || !self.images.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RoomImage {
    pub id: i64,
    /// Absolute, fetchable CDN URL. Never construct this from `object_key`; the
    /// CDN host belongs to the image service, not to db-api.
    pub image_url: String,
    pub object_key: String,
    pub position: i32,
    pub caption: Option<String>,
    pub is_primary: bool,
    /// Discord snowflake, as a string.
    pub uploaded_by: Option<String>,
    pub uploaded_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Contributor {
    pub display_name: String,
    pub avatar: Option<String>,
}

/// Observed gameplay properties.
///
/// Every field is three-state and the distinction matters: a field absent from
/// the payload means nobody has checked, whereas `Some(false)` / `Some(0)` /
/// `Some(vec![])` means someone checked and found none. Both arrive here as the
/// same `Option`, so the UI must render "unknown" for `None` rather than
/// defaulting to a negative.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RoomAttributes {
    pub documented_at: Option<String>,
    /// A *source label* like `"harvester-v1"`, not a snowflake.
    pub documented_by: Option<String>,
    pub roomfamily: Option<String>,
    pub guaranteed_keycard: Option<bool>,
    pub sometimes_keycard: Option<bool>,
    pub purple_keycard: Option<bool>,
    pub entrances: Option<i32>,
    pub exits: Option<Vec<String>>,
    pub is_roomtype_entrance: Option<bool>,
    pub is_roomtype_exit: Option<bool>,
    /// Relative likelihood against the rest of the spawn pool. Unbounded above.
    pub spawnweight: Option<f64>,
    pub spawn_before: Option<i32>,
    pub spawn_after: Option<i32>,
    pub spawn_max_amount: Option<i32>,
    pub max_squiddles: Option<i32>,
    pub has_water: Option<bool>,
    pub has_fire: Option<bool>,
    pub has_pit: Option<bool>,
    pub has_lava: Option<bool>,
    pub has_electricity: Option<bool>,
    pub has_siderooms: Option<bool>,
    pub has_vents: Option<bool>,
    pub has_fans: Option<bool>,
    pub has_steam: Option<bool>,
    pub has_turrets: Option<bool>,
    pub has_code_breacher_door: Option<bool>,
}

/// How closely `/lookup` matched the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LookupMatch {
    /// Exact slug hit.
    Slug,
    /// Exact `room_name` hit.
    RoomName,
    /// Matched after stripping non-alphanumerics from both sides.
    Squashed,
    /// Query is a prefix of one or more rooms.
    Prefix,
    /// Query appears somewhere within one or more rooms.
    Substring,
    /// Nothing resolved. The room is not in the database — this is not an error.
    None,
}

impl LookupMatch {
    /// Whether this tier is good enough to act on without asking the user.
    ///
    /// The website applies exactly this rule before redirecting. `Prefix` and
    /// `Substring` are suggestions, not answers: acting on them would display
    /// the wrong room with full confidence.
    pub fn is_authoritative(self) -> bool {
        matches!(self, Self::Slug | Self::RoomName | Self::Squashed)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LookupCandidate {
    pub slug: String,
    pub case_name: String,
    pub room_name: String,
    pub roomtype: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LookupResponse {
    pub query: String,
    /// `match` is a Rust keyword.
    #[serde(rename = "match")]
    pub match_kind: LookupMatch,
    /// Populated only for the authoritative tiers.
    pub exact: Option<LookupCandidate>,
    #[serde(default)]
    pub candidates: Vec<LookupCandidate>,
    /// May exceed `candidates.len()`, which is capped by the request's `limit`.
    pub total: i64,
}

impl LookupResponse {
    /// The slug to fetch, if this lookup answered the question outright.
    pub fn resolved_slug(&self) -> Option<&str> {
        if self.match_kind.is_authoritative() {
            self.exact.as_ref().map(|c| c.slug.as_str())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETAIL: &str = include_str!("../../tests/fixtures/room_detail.json");
    const SQUASHED: &str = include_str!("../../tests/fixtures/lookup_squashed.json");
    const NONE: &str = include_str!("../../tests/fixtures/lookup_none.json");

    #[test]
    fn deserializes_a_real_room() {
        let room: Room = serde_json::from_str(DETAIL).expect("real payload parses");
        assert_eq!(room.slug, "000");
        assert_eq!(room.images.len(), 6);
        assert!(room.is_documented());
    }

    #[test]
    fn snowflakes_stay_strings() {
        // Typing these as u64 parses most payloads and then breaks. The service
        // stringifies them on purpose; the fixture is a real response.
        let room: Room = serde_json::from_str(DETAIL).unwrap();
        let id = room.documented_by.expect("fixture has a documenting user");
        assert!(
            id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 17,
            "expected a snowflake as a string, got {id:?}"
        );
    }

    #[test]
    fn primary_image_prefers_the_flagged_one() {
        let room: Room = serde_json::from_str(DETAIL).unwrap();
        let primary = room.primary_image().expect("fixture has images");
        if room.images.iter().any(|i| i.is_primary) {
            assert!(primary.is_primary);
        } else {
            // Falls back to lowest position, matching the website.
            assert_eq!(primary.position, room.ordered_images()[0].position);
        }
    }

    #[test]
    fn ordered_images_sort_by_position() {
        let room: Room = serde_json::from_str(DETAIL).unwrap();
        let positions: Vec<_> = room.ordered_images().iter().map(|i| i.position).collect();
        let mut sorted = positions.clone();
        sorted.sort();
        assert_eq!(positions, sorted);
    }

    #[test]
    fn parses_a_squashed_match_and_resolves_it() {
        // "alti room" -> "altiroom". Note room_name is "altiroom?" but the slug
        // drops the '?', which is why deriving slugs locally cannot work.
        let r: LookupResponse = serde_json::from_str(SQUASHED).unwrap();
        assert_eq!(r.match_kind, LookupMatch::Squashed);
        assert_eq!(r.resolved_slug(), Some("altiroom"));
        assert_eq!(r.exact.as_ref().unwrap().room_name, "altiroom?");
    }

    #[test]
    fn a_miss_is_not_an_error() {
        let r: LookupResponse = serde_json::from_str(NONE).unwrap();
        assert_eq!(r.match_kind, LookupMatch::None);
        assert_eq!(r.resolved_slug(), None);
        assert_eq!(r.total, 0);
    }

    #[test]
    fn weak_tiers_are_suggestions_not_answers() {
        assert!(LookupMatch::Slug.is_authoritative());
        assert!(LookupMatch::RoomName.is_authoritative());
        assert!(LookupMatch::Squashed.is_authoritative());
        assert!(!LookupMatch::Prefix.is_authoritative());
        assert!(!LookupMatch::Substring.is_authoritative());
        assert!(!LookupMatch::None.is_authoritative());
    }

    #[test]
    fn absent_attributes_stay_none() {
        // `null` attributes must not become a default-filled struct, or the UI
        // would render "no water, no fire" for a room nobody has examined.
        let room: Room = serde_json::from_str(DETAIL).unwrap();
        assert!(room.attributes.is_none());
    }

    #[test]
    fn attributes_distinguish_unknown_from_observed_none() {
        let json = r#"{"entrances":0,"has_water":false,"exits":[]}"#;
        let a: RoomAttributes = serde_json::from_str(json).unwrap();
        assert_eq!(a.entrances, Some(0), "observed zero, not unknown");
        assert_eq!(a.has_water, Some(false), "observed absent, not unknown");
        assert_eq!(a.exits, Some(vec![]), "observed none, not unknown");
        assert_eq!(a.has_turrets, None, "genuinely unchecked");
    }
}
