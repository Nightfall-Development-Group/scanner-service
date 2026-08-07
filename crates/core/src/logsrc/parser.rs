//! Turning Roblox client log lines into scanner events.
//!
//! Roblox writes lines in a loose CSV-ish shape whose field count varies:
//!
//! ```text
//! 2026-08-07T09:28:54.580Z,134.580063,84a4,6,Info [FLog::CreatorOutput] Room Name: Start
//! 2026-08-07T09:26:58.131Z,18.131926,ab24,7 [FLog::Network] UDMUX Address = 1.2.3.4, Port = 63300 | ...
//! ```
//!
//! Note the second line has no level *name* where the first has `Info`. Rather
//! than count commas, we split at the first bracketed tag: everything before it
//! is metadata, everything after is the message.
//!
//! # Why matching is deliberately loose
//!
//! v1 looked for the literal `"[flog::network] client:disconnect"`. The real tag
//! is `[DFLog::NetworkClient]`, so that test never once fired — which silently
//! disabled both its session reset and its "resume from the last disconnect"
//! logic. We therefore key on the distinctive *message*, and treat the tag as a
//! hint rather than a requirement. A tag rename should degrade nothing.

/// A log line split into its metadata and message halves.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine<'a> {
    /// Leading ISO-8601 timestamp, e.g. `2026-08-07T09:28:54.580Z`.
    pub timestamp: &'a str,
    /// Seconds since client start. Monotonic within a session, which is what
    /// makes it usable for de-duplication without a clock or a date parser.
    pub uptime: Option<f64>,
    /// The bracketed tag including brackets, e.g. `[FLog::CreatorOutput]`.
    pub tag: &'a str,
    /// Everything after the tag, trimmed.
    pub message: &'a str,
}

/// Something the scanner cares about.
#[derive(Debug, Clone, PartialEq)]
pub enum LogEvent {
    /// The player entered a room. `name` is verbatim from the log and is *not*
    /// a slug — resolve it through `/api/rooms/lookup`, never by transforming
    /// it locally.
    RoomEntered { name: String },
    /// The game server's public address, for geolocation.
    ServerAddress { ip: String },
    /// The client dropped its connection. Also fires on a teleport between
    /// places, so this marks a *session boundary*, not necessarily a quit.
    Disconnected,
    /// The client began joining a place.
    GameJoined { place_id: u64 },
}

const ROOM_PREFIX: &str = "Room Name:";
const UDMUX_PREFIX: &str = "UDMUX Address =";
const DISCONNECT_MSG: &str = "Client:Disconnect";
const JOIN_PREFIX: &str = "! Joining game";

/// Roblox emits `Client:Disconnect` twice for a single drop, ~0.65 s apart in
/// the reference log. Collapse anything inside this window into one event.
const DISCONNECT_DEBOUNCE_SECS: f64 = 5.0;

/// Split a raw line into metadata and message. Returns `None` for lines with no
/// bracketed tag, which are not interesting to us.
pub fn split_line(line: &str) -> Option<LogLine<'_>> {
    let open = line.find('[')?;
    let close = line[open..].find(']')? + open;

    let meta = &line[..open];
    let mut fields = meta.split(',');
    let timestamp = fields.next().unwrap_or("").trim();
    let uptime = fields.next().and_then(|f| f.trim().parse::<f64>().ok());

    Some(LogLine {
        timestamp,
        uptime,
        tag: &line[open..=close],
        message: line[close + 1..].trim(),
    })
}

/// Case-insensitive `starts_with` that also hands back the remainder.
fn strip_prefix_ci<'a>(haystack: &'a str, prefix: &str) -> Option<&'a str> {
    let head = haystack.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| haystack[prefix.len()..].trim_start())
}

/// Stateful across lines, because de-duplicating the doubled disconnect needs
/// to remember the previous one.
///
/// v1 kept this state in module-level globals, so it was shared process-wide and
/// could never be reset between sessions. Here it belongs to the parser.
#[derive(Debug, Default)]
pub struct Parser {
    last_disconnect_uptime: Option<f64>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget accumulated state. Call when switching log files.
    pub fn reset(&mut self) {
        self.last_disconnect_uptime = None;
    }

    /// Parse one line. Returns `None` when the line carries nothing we track.
    pub fn parse_line(&mut self, line: &str) -> Option<LogEvent> {
        let parsed = split_line(line)?;
        let msg = parsed.message;

        if let Some(name) = strip_prefix_ci(msg, ROOM_PREFIX) {
            let name = name.trim();
            // Guard against a malformed `Room Name:` with nothing after it.
            if name.is_empty() {
                return None;
            }
            // Take the whole remainder, not the last whitespace token as v1 did.
            // Names legitimately contain `/` (`LeftStraightW/Room1`), and the
            // corpus has names with spaces that v1 would have truncated.
            return Some(LogEvent::RoomEntered {
                name: name.to_string(),
            });
        }

        if let Some(rest) = strip_prefix_ci(msg, UDMUX_PREFIX) {
            // `1.2.3.4, Port = 63300 | RCC Server Address = 10.x.x.x, ...`
            // Only the first address is public; the RCC one is a private LAN
            // address and must never be sent to a geolocation service.
            let ip = rest.split(',').next()?.trim();
            if !ip.is_empty() {
                return Some(LogEvent::ServerAddress { ip: ip.to_string() });
            }
        }

        if msg.eq_ignore_ascii_case(DISCONNECT_MSG) {
            return self.debounce_disconnect(parsed.uptime);
        }

        if let Some(rest) = strip_prefix_ci(msg, JOIN_PREFIX) {
            if let Some(place_id) = parse_place_id(rest) {
                return Some(LogEvent::GameJoined { place_id });
            }
        }

        None
    }

    /// Parse a batch, dropping lines that carry nothing.
    pub fn parse_lines<I, S>(&mut self, lines: I) -> Vec<LogEvent>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        lines
            .into_iter()
            .filter_map(|l| self.parse_line(l.as_ref()))
            .collect()
    }

    fn debounce_disconnect(&mut self, uptime: Option<f64>) -> Option<LogEvent> {
        match (uptime, self.last_disconnect_uptime) {
            (Some(now), Some(prev)) if (now - prev).abs() < DISCONNECT_DEBOUNCE_SECS => {
                // Second half of the doubled pair; keep the newer timestamp so a
                // burst of three collapses rather than alternating through.
                self.last_disconnect_uptime = Some(now);
                None
            }
            (Some(now), _) => {
                self.last_disconnect_uptime = Some(now);
                Some(LogEvent::Disconnected)
            }
            // No uptime to compare against: report rather than risk swallowing a
            // real session boundary.
            (None, _) => Some(LogEvent::Disconnected),
        }
    }
}

/// Pull the place id out of `'<guid>' place 12411473842 at 10.8.4.154`.
fn parse_place_id(rest: &str) -> Option<u64> {
    rest.split_once("place")?
        .1
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = "2026-08-07T09:28:54.580Z,134.580063,84a4,6,Info \
                        [FLog::CreatorOutput] Room Name: Start";
    const UDMUX: &str = "2026-08-07T09:26:58.131Z,18.131926,ab24,7 [FLog::Network] \
                         UDMUX Address = 128.116.95.33, Port = 63300 | \
                         RCC Server Address = 10.8.4.154, Port = 63300";
    const DISCONNECT: &str = "2026-08-07T09:28:46.608Z,126.608849,b90c,6,Info \
                              [DFLog::NetworkClient] Client:Disconnect";

    #[test]
    fn splits_a_line_with_a_level_name() {
        let l = split_line(ROOM).unwrap();
        assert_eq!(l.timestamp, "2026-08-07T09:28:54.580Z");
        assert_eq!(l.uptime, Some(134.580063));
        assert_eq!(l.tag, "[FLog::CreatorOutput]");
        assert_eq!(l.message, "Room Name: Start");
    }

    #[test]
    fn splits_a_line_without_a_level_name() {
        // This shape has one fewer metadata field; a comma-counting parser
        // would misread it.
        let l = split_line(UDMUX).unwrap();
        assert_eq!(l.uptime, Some(18.131926));
        assert_eq!(l.tag, "[FLog::Network]");
        assert!(l.message.starts_with("UDMUX Address ="));
    }

    #[test]
    fn ignores_lines_with_no_tag() {
        assert!(split_line("just some text").is_none());
    }

    #[test]
    fn extracts_a_room_name() {
        let mut p = Parser::new();
        assert_eq!(
            p.parse_line(ROOM),
            Some(LogEvent::RoomEntered {
                name: "Start".into()
            })
        );
    }

    #[test]
    fn keeps_slashes_in_room_names() {
        let mut p = Parser::new();
        let line = "2026-08-07T09:30:09.582Z,209.582550,ba3c,6,Info \
                    [FLog::CreatorOutput] Room Name: LeftStraightW/Room1";
        assert_eq!(
            p.parse_line(line),
            Some(LogEvent::RoomEntered {
                name: "LeftStraightW/Room1".into()
            })
        );
    }

    #[test]
    fn keeps_spaces_in_room_names() {
        // v1 took the last whitespace token, which truncated this to "Name".
        let mut p = Parser::new();
        let line = "2026-08-07T09:30:09.582Z,209.5,ba3c,6,Info \
                    [FLog::CreatorOutput] Room Name: Some Long Name";
        assert_eq!(
            p.parse_line(line),
            Some(LogEvent::RoomEntered {
                name: "Some Long Name".into()
            })
        );
    }

    #[test]
    fn ignores_non_room_creator_output() {
        let mut p = Parser::new();
        let line = "2026-08-07T09:27:01.649Z,21.6,8760,6,Info \
                    [FLog::CreatorOutput] TestPlayer Already in group";
        assert_eq!(p.parse_line(line), None);
    }

    #[test]
    fn takes_the_public_address_not_the_rcc_one() {
        let mut p = Parser::new();
        assert_eq!(
            p.parse_line(UDMUX),
            Some(LogEvent::ServerAddress {
                ip: "128.116.95.33".into()
            })
        );
    }

    #[test]
    fn detects_disconnect_despite_the_dflog_tag() {
        // The regression that mattered: v1 required "[flog::network]" here.
        let mut p = Parser::new();
        assert_eq!(p.parse_line(DISCONNECT), Some(LogEvent::Disconnected));
    }

    #[test]
    fn collapses_the_doubled_disconnect() {
        let mut p = Parser::new();
        let first = DISCONNECT;
        let second = "2026-08-07T09:28:47.258Z,127.258163,ba3c,6,Info \
                      [DFLog::NetworkClient] Client:Disconnect";
        assert_eq!(p.parse_line(first), Some(LogEvent::Disconnected));
        assert_eq!(p.parse_line(second), None, "0.65s apart is one event");
    }

    #[test]
    fn reports_a_genuinely_later_disconnect() {
        let mut p = Parser::new();
        assert_eq!(p.parse_line(DISCONNECT), Some(LogEvent::Disconnected));
        let much_later = "2026-08-07T09:40:00.000Z,800.0,ba3c,6,Info \
                          [DFLog::NetworkClient] Client:Disconnect";
        assert_eq!(p.parse_line(much_later), Some(LogEvent::Disconnected));
    }

    #[test]
    fn extracts_the_place_id() {
        let mut p = Parser::new();
        let line = "2026-08-07T09:26:58.125Z,18.125952,ab24,6 [FLog::Output] \
                    ! Joining game '00000000-0000-0000-0000-000000000000' \
                    place 12411473842 at 10.8.4.154";
        assert_eq!(
            p.parse_line(line),
            Some(LogEvent::GameJoined {
                place_id: 12411473842
            })
        );
    }

    #[test]
    fn reset_clears_disconnect_state() {
        let mut p = Parser::new();
        assert_eq!(p.parse_line(DISCONNECT), Some(LogEvent::Disconnected));
        p.reset();
        // Same line again: after a reset it is a fresh session boundary.
        assert_eq!(p.parse_line(DISCONNECT), Some(LogEvent::Disconnected));
    }
}
