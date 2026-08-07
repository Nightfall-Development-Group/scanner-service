//! Parser behaviour against a real captured Roblox session.
//!
//! `fixtures/session.log` is a scrubbed extract of an actual player log: IPs,
//! username, place id and job guid replaced, everything structural intact. It
//! covers a lobby join, a teleport into the game (which is what the doubled
//! `Client:Disconnect` actually marks), and 41 room entries.

use std::io::Cursor;

use scanner_core::logsrc::{find_resume_point, LogEvent, Parser};

const SESSION: &str = include_str!("fixtures/session.log");

fn events() -> Vec<LogEvent> {
    Parser::new().parse_lines(SESSION.lines())
}

#[test]
fn finds_every_room_in_the_session() {
    let rooms: Vec<_> = events()
        .into_iter()
        .filter_map(|e| match e {
            LogEvent::RoomEntered { name } => Some(name),
            _ => None,
        })
        .collect();

    assert_eq!(rooms.len(), 41, "fixture contains 41 Room Name lines");
    assert_eq!(rooms[0], "Start", "first room of the run");
    assert!(
        rooms.iter().any(|r| r == "LeftStraightW/Room1"),
        "names containing '/' must survive intact"
    );
    assert!(
        rooms.iter().all(|r| !r.is_empty()),
        "no empty names should be emitted"
    );
}

#[test]
fn collapses_the_doubled_disconnect_into_one() {
    // The raw log has two Client:Disconnect lines 0.65s apart marking a single
    // teleport. A scanner that treated those as two session boundaries would
    // reset its state twice.
    let disconnects = events()
        .iter()
        .filter(|e| matches!(e, LogEvent::Disconnected))
        .count();
    assert_eq!(disconnects, 1, "two log lines, one real event");
}

#[test]
fn takes_only_public_server_addresses() {
    let ips: Vec<_> = events()
        .into_iter()
        .filter_map(|e| match e {
            LogEvent::ServerAddress { ip } => Some(ip),
            _ => None,
        })
        .collect();

    assert!(!ips.is_empty(), "the session contains UDMUX lines");
    for ip in &ips {
        assert!(
            !ip.starts_with("10.") && !ip.starts_with("192.168."),
            "{ip} is a private RCC address and must never reach a geolocation service"
        );
    }
}

#[test]
fn ignores_the_creator_output_chatter() {
    // The log's CreatorOutput tag carries sprint state, jump power and other
    // game noise. Only `Room Name:` lines are rooms.
    let noise = "2026-08-07T09:27:03.017Z,23.017553,b90c,6,Info \
                 [FLog::CreatorOutput] Couldn't find EndlessFirewall100";
    assert_eq!(Parser::new().parse_line(noise), None);
}

#[test]
fn the_run_begins_at_start() {
    // A run is delimited by the room named `Start`, not by the disconnect. The
    // disconnect fires on the lobby teleport too, so it is not a run boundary.
    let evs = events();
    let run_start = evs
        .iter()
        .position(LogEvent::is_run_start)
        .expect("session contains a run start");

    assert!(
        evs[..run_start]
            .iter()
            .all(|e| !matches!(e, LogEvent::RoomEntered { .. })),
        "nothing before Start is a room; that stretch is lobby and teleport"
    );
    assert_eq!(
        evs.len() - run_start,
        41,
        "the run is 41 rooms including Start"
    );
}

#[test]
fn resume_skips_the_lobby_but_keeps_the_server_address() {
    // The property that makes this correct on a real log: the game server's
    // UDMUX line precedes Room Name: Start by ~10s, so resuming at Start must
    // still carry the address forward or geolocation silently breaks.
    let mut cursor = Cursor::new(SESSION.as_bytes());
    let point = find_resume_point(&mut cursor).expect("scan succeeds");

    let tail = &SESSION[point.offset as usize..];
    assert!(
        tail.starts_with("2026-") && tail.contains("Room Name: Start"),
        "resume lands on the Start line"
    );
    assert!(
        !tail.contains("! Joining game"),
        "the lobby join is behind us"
    );

    let ip = point
        .server_ip
        .expect("server address recovered from before the run");
    assert!(
        !ip.starts_with("10.") && !ip.starts_with("192.168."),
        "{ip} must be the public UDMUX address, not the private RCC one"
    );
}

#[test]
fn tailing_from_the_resume_point_yields_exactly_the_run() {
    let mut cursor = Cursor::new(SESSION.as_bytes());
    let point = find_resume_point(&mut cursor).expect("scan succeeds");

    let rooms: Vec<_> = Parser::new()
        .parse_lines(SESSION[point.offset as usize..].lines())
        .into_iter()
        .filter_map(|e| match e {
            LogEvent::RoomEntered { name } => Some(name),
            _ => None,
        })
        .collect();

    assert_eq!(rooms.len(), 41);
    assert_eq!(rooms.first().map(String::as_str), Some("Start"));
}
