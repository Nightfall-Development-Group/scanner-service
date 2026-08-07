//! Deciding where in a log file to begin reading.
//!
//! A run begins at the room named `Start`. Everything before it is the lobby and
//! the teleport into the game, which the scanner does not care about.
//!
//! Seeking straight to that line would be wrong, though: the game server's
//! `UDMUX` address is logged ~10 s *before* the first room, so a naive seek
//! throws away the address we need for geolocation. So this is a two-part
//! answer — scan the whole file once to establish state, and report the offset
//! to start tailing from separately.
//!
//! v1 instead sought to the last `Client:Disconnect`, which never matched (see
//! `parser`), so it always re-read from byte zero and re-reported every room.

use std::io::{self, BufRead, Seek, SeekFrom};

use super::parser::{LogEvent, Parser};

/// The room whose appearance marks the beginning of a run.
pub const RUN_START_ROOM: &str = "Start";

impl LogEvent {
    /// Whether this event marks the beginning of a run. The engine uses this to
    /// clear per-run state rather than keying off a disconnect, which also fires
    /// on a lobby teleport.
    pub fn is_run_start(&self) -> bool {
        matches!(self, LogEvent::RoomEntered { name } if name.eq_ignore_ascii_case(RUN_START_ROOM))
    }
}

/// Where to resume, plus whatever state was established before that point.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResumePoint {
    /// Byte offset to begin tailing from. Positioned at the *start* of the run's
    /// `Room Name: Start` line, so `Start` is reported like any other room.
    pub offset: u64,
    /// Most recent server address at or before `offset`. Logged before the run
    /// begins, so it would be lost if we only read forward from `offset`.
    pub server_ip: Option<String>,
    /// The room the player is currently in, when we are resuming mid-run with no
    /// run start ahead of us. Lets the UI show something immediately instead of
    /// staying blank until the player next moves.
    pub current_room: Option<String>,
}

/// Scan `reader` from the beginning and decide where a scan should start.
///
/// If the file contains a run start, tailing resumes there and the run's rooms
/// are reported from the top. If it does not — the player is mid-run, or this is
/// a lobby-only log — we resume at the end of the file and report only new
/// activity, carrying the current room forward so the display is not empty.
pub fn find_resume_point<R: BufRead + Seek>(reader: &mut R) -> io::Result<ResumePoint> {
    reader.seek(SeekFrom::Start(0))?;

    let mut parser = Parser::new();
    let mut offset: u64 = 0;
    let mut line = String::new();

    // State as of the most recent run start.
    let mut run_start: Option<u64> = None;
    let mut ip_at_run_start: Option<String> = None;

    // Running state, used when there is no run start to fall back on.
    let mut latest_ip: Option<String> = None;
    let mut latest_room: Option<String> = None;

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }

        match parser.parse_line(&line) {
            Some(LogEvent::ServerAddress { ip }) => latest_ip = Some(ip),
            Some(LogEvent::RoomEntered { name }) => {
                if name.eq_ignore_ascii_case(RUN_START_ROOM) {
                    // A later run supersedes an earlier one.
                    run_start = Some(offset);
                    ip_at_run_start = latest_ip.clone();
                }
                latest_room = Some(name);
            }
            _ => {}
        }

        offset += read as u64;
    }

    Ok(match run_start {
        Some(start) => ResumePoint {
            offset: start,
            server_ip: ip_at_run_start,
            current_room: None,
        },
        None => ResumePoint {
            offset,
            server_ip: latest_ip,
            current_room: latest_room,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn room(name: &str) -> String {
        format!(
            "2026-08-07T09:00:00.000Z,1.0,aaaa,6,Info [FLog::CreatorOutput] Room Name: {name}\n"
        )
    }

    fn udmux(ip: &str) -> String {
        format!(
            "2026-08-07T09:00:00.000Z,1.0,aaaa,7 [FLog::Network] UDMUX Address = {ip}, Port = 1\n"
        )
    }

    fn point_of(text: &str) -> ResumePoint {
        find_resume_point(&mut Cursor::new(text.as_bytes())).unwrap()
    }

    #[test]
    fn resumes_at_the_run_start() {
        let text = format!("{}{}{}", udmux("1.2.3.4"), room("Lobby"), room("Start"));
        let p = point_of(&text);

        let expected = (udmux("1.2.3.4").len() + room("Lobby").len()) as u64;
        assert_eq!(p.offset, expected, "offset is the start of the Start line");
        assert_eq!(p.current_room, None);
    }

    #[test]
    fn carries_forward_a_server_address_logged_before_the_run() {
        // The case that motivates this module: UDMUX precedes Room Name: Start,
        // so reading forward from the offset alone would lose it.
        let text = format!("{}{}", udmux("128.116.55.33"), room("Start"));
        assert_eq!(point_of(&text).server_ip.as_deref(), Some("128.116.55.33"));
    }

    #[test]
    fn a_later_run_supersedes_an_earlier_one() {
        let text = format!(
            "{}{}{}{}",
            room("Start"),
            room("Straight2"),
            udmux("9.9.9.9"),
            room("Start")
        );
        let p = point_of(&text);

        let expected =
            (room("Start").len() + room("Straight2").len() + udmux("9.9.9.9").len()) as u64;
        assert_eq!(p.offset, expected, "resume at the most recent run");
        assert_eq!(p.server_ip.as_deref(), Some("9.9.9.9"));
    }

    #[test]
    fn mid_run_resume_goes_to_the_end_and_keeps_the_current_room() {
        // No Start anywhere: the scanner was launched partway through a run.
        // Re-reading from zero is what made v1 duplicate everything.
        let text = format!(
            "{}{}{}",
            udmux("1.2.3.4"),
            room("Straight2"),
            room("LeftTurn1")
        );
        let p = point_of(&text);

        assert_eq!(p.offset, text.len() as u64, "tail only new activity");
        assert_eq!(p.current_room.as_deref(), Some("LeftTurn1"));
        assert_eq!(p.server_ip.as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn an_empty_file_resumes_at_zero() {
        assert_eq!(point_of(""), ResumePoint::default());
    }

    #[test]
    fn offset_lands_exactly_on_a_line_boundary() {
        // Guard against off-by-one: reading from the offset must yield the Start
        // line whole, not a fragment of it.
        let text = format!("{}{}{}", room("Lobby"), room("Start"), room("Straight2"));
        let p = point_of(&text);

        let rest = &text[p.offset as usize..];
        assert!(rest.starts_with("2026-"), "offset is mid-line: {rest:.40}");
        assert!(rest.contains("Room Name: Start"));
    }
}
