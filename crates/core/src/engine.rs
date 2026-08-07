//! The scanner loop: follow a log, resolve what appears in it, emit events.
//!
//! The design constraint is that nothing here can block the UI, because nothing
//! here *is* the UI. The engine owns its own state, talks to the outside world
//! through an [`Event`] channel, and is driven either by a filesystem watcher or
//! by a timer — both of which just call [`Scanner::pump`].
//!
//! `pump` is deliberately separable from the timing so tests can drive the whole
//! pipeline deterministically against a temp file, with no sleeping and no
//! filesystem events.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::api::{ApiClient, ApiError};
use crate::event::{Event, Status};
use crate::geo;
use crate::logsrc::finder::{self, LogSource};
use crate::logsrc::tailer::Tailer;
use crate::logsrc::{LogEvent, Parser, ResumePoint};

/// How often to read the log when no filesystem event has arrived.
///
/// A watcher alone is not enough: some writers append without updating metadata
/// in a way every platform reports promptly, and network or overlay filesystems
/// drop events entirely. The poll makes a missed event mean "slightly late"
/// rather than "silently dead".
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often to check whether the client rotated to a newer log file.
const ROTATION_CHECK: Duration = Duration::from_secs(5);

pub struct Scanner {
    client: Arc<ApiClient>,
    events: mpsc::UnboundedSender<Event>,
    parser: Parser,
    /// Suppresses a repeated room name from producing duplicate work when the
    /// client logs the same room twice in a row.
    last_room: Option<String>,
    /// Most recent server address, so we geolocate a given address once.
    located_ip: Option<String>,
}

impl Scanner {
    pub fn new(client: Arc<ApiClient>, events: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            client,
            events,
            parser: Parser::new(),
            last_room: None,
            located_ip: None,
        }
    }

    /// Apply the state a [`ResumePoint`] established before the run began.
    ///
    /// Necessary because the server address is logged ~10 s before the first
    /// room, so tailing from the run start alone would never see it. Also
    /// reports the current room when we joined partway through a run, so the UI
    /// has something to show immediately.
    pub async fn seed(&mut self, point: &ResumePoint) {
        if let Some(ip) = &point.server_ip {
            self.on_server_address(ip.clone()).await;
        }
        if let Some(room) = &point.current_room {
            self.on_room(room.clone()).await;
        }
    }

    /// Read whatever is new and emit the resulting events.
    ///
    /// Returns the number of log lines consumed, which the caller can use to
    /// decide whether to keep pumping immediately.
    pub async fn pump(&mut self, tailer: &mut Tailer) -> std::io::Result<usize> {
        let batch = tailer.read_new()?;

        if batch.restarted {
            self.emit(Event::debug(
                "log file was truncated; restarting from the top",
            ));
            self.reset_run();
        }

        let consumed = batch.lines.len();
        for log_event in self.parser.parse_lines(&batch.lines) {
            self.handle(log_event).await;
        }
        Ok(consumed)
    }

    async fn handle(&mut self, event: LogEvent) {
        match event {
            LogEvent::RoomEntered { name } => self.on_room(name).await,
            LogEvent::ServerAddress { ip } => self.on_server_address(ip).await,
            LogEvent::Disconnected => {
                // Not a run boundary: this also fires on the lobby teleport.
                self.emit(Event::debug("client disconnected"));
            }
            LogEvent::GameJoined { place_id } => {
                self.emit(Event::debug(format!("joining place {place_id}")));
            }
        }
    }

    async fn on_room(&mut self, name: String) {
        if self.last_room.as_deref() == Some(name.as_str()) {
            self.emit(Event::debug(format!("still in {name}; skipping")));
            return;
        }

        // A run starts at `Start`; clear the previous run before reporting it.
        let starts_run = LogEvent::RoomEntered { name: name.clone() }.is_run_start();
        if starts_run {
            self.reset_run();
            self.emit(Event::RunStarted);
            self.emit(Event::log("--- new run ---"));
        }

        self.last_room = Some(name.clone());
        self.emit(Event::RoomEntered { name: name.clone() });

        match self.client.resolve_room(&name).await {
            Ok(Some(room)) => {
                self.emit(Event::log(format!("Entered {}", room.case_name)));
                self.emit(Event::RoomResolved {
                    name,
                    room: Box::new(Some(room)),
                });
            }
            Ok(None) => {
                self.emit(Event::log(format!("Entered {name} (undocumented)")));
                self.emit(Event::RoomResolved {
                    name,
                    room: Box::new(None),
                });
            }
            Err(e) => {
                // A lookup failure must not stop the scan. The player keeps
                // moving; the next room may well succeed.
                self.emit(Event::Warning(format!("could not look up {name}: {e}")));
                if e.is_auth_problem() {
                    self.emit(Event::Status(Status::Stopped {
                        reason: e.to_string(),
                    }));
                }
                self.emit(Event::RoomResolved {
                    name,
                    room: Box::new(None),
                });
            }
        }
    }

    async fn on_server_address(&mut self, ip: String) {
        if self.located_ip.as_deref() == Some(ip.as_str()) {
            return;
        }
        self.located_ip = Some(ip.clone());

        let location = geo::locate(&ip).await;
        if let Some(l) = &location {
            self.emit(Event::log(format!("Server: {}", l.describe())));
        } else {
            self.emit(Event::debug(format!("no location for {ip}")));
        }
        self.emit(Event::ServerLocated {
            ip,
            location: Box::new(location),
        });
    }

    /// Forget per-run state. Called on a new run and on log truncation.
    fn reset_run(&mut self) {
        self.parser.reset();
        self.last_room = None;
    }

    /// Send, ignoring a closed channel — that only means the UI has gone away,
    /// which is a shutdown, not an error worth propagating.
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }
}

/// Locate a log, seed state from it, and follow it until `shutdown` flips.
///
/// Returns when shutdown is requested or the log cannot be opened at all.
pub async fn run(
    client: Arc<ApiClient>,
    events: mpsc::UnboundedSender<Event>,
    override_path: Option<PathBuf>,
    mut shutdown: watch::Receiver<bool>,
) {
    let _ = events.send(Event::Status(Status::Searching));

    let source = match finder::find_log(override_path.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(Event::Status(Status::Stopped {
                reason: e.to_string(),
            }));
            return;
        }
    };

    let mut scanner = Scanner::new(client, events.clone());
    let mut tailer = match start_following(&source, &mut scanner).await {
        Ok(t) => t,
        Err(e) => {
            let _ = events.send(Event::Status(Status::Stopped {
                reason: format!("could not read {}: {e}", source.path().display()),
            }));
            return;
        }
    };

    let _ = events.send(Event::Status(Status::Watching));

    let mut poll = tokio::time::interval(POLL_INTERVAL);
    let mut rotation = tokio::time::interval(ROTATION_CHECK);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
            _ = poll.tick() => {
                if let Err(e) = scanner.pump(&mut tailer).await {
                    let _ = events.send(Event::Warning(format!("read failed: {e}")));
                }
            }
            _ = rotation.tick() => {
                if let Some(newer) = newer_log(&source, tailer.path()) {
                    let _ = events.send(Event::log(format!(
                        "switched to {}", newer.file_name().unwrap_or_default().to_string_lossy()
                    )));
                    match Tailer::at_run_start(&newer) {
                        Ok((t, _)) => {
                            tailer = t;
                            scanner.reset_run();
                            let _ = events.send(Event::WatchingFile(newer));
                        }
                        Err(e) => {
                            let _ = events.send(Event::Warning(format!("could not open {}: {e}", newer.display())));
                        }
                    }
                }
            }
        }
    }

    let _ = events.send(Event::Status(Status::Idle));
}

/// Open the log at its most recent run start and report what that established.
async fn start_following(source: &LogSource, scanner: &mut Scanner) -> std::io::Result<Tailer> {
    let (tailer, point) = Tailer::at_run_start(source.path())?;

    scanner.emit(Event::WatchingFile(source.path().to_owned()));
    scanner.emit(Event::log(format!(
        "Watching {}",
        source
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    )));

    scanner.seed(&point).await;
    Ok(tailer)
}

/// A different, newer log file in the watched directory, if one appeared.
fn newer_log(source: &LogSource, current: &Path) -> Option<PathBuf> {
    let dir = source.dir()?;
    let newest = finder::newest_log_in(dir)?;
    (newest != current).then_some(newest)
}

/// Build a client from a key, mapping the empty-key case to a clear status.
pub fn client_from_key(api_key: &str) -> Result<Arc<ApiClient>, ApiError> {
    ApiClient::new(api_key).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn room_line(name: &str) -> String {
        format!("2026-01-01T00:00:00Z,1.0,aaaa,6,Info [FLog::CreatorOutput] Room Name: {name}\n")
    }

    /// A scanner pointed at an unreachable base URL, so `resolve_room` always
    /// fails fast. Lets us assert the event *pipeline* without a network.
    fn offline_scanner() -> (Scanner, mpsc::UnboundedReceiver<Event>) {
        let client = Arc::new(
            ApiClient::with_base("test-key", "http://127.0.0.1:1").expect("client builds"),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        (Scanner::new(client, tx), rx)
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    #[tokio::test]
    async fn emits_room_entered_before_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Straight2"));

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();

        let events = drain(&mut rx);
        let entered = events
            .iter()
            .position(|e| matches!(e, Event::RoomEntered { .. }))
            .expect("RoomEntered emitted");
        let resolved = events
            .iter()
            .position(|e| matches!(e, Event::RoomResolved { .. }))
            .expect("RoomResolved emitted");

        assert!(
            entered < resolved,
            "the name must reach the UI before the network round trip"
        );
    }

    #[tokio::test]
    async fn a_lookup_failure_does_not_stop_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Straight2"));
        write(&path, &room_line("LeftTurn1"));

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();

        let events = drain(&mut rx);
        let resolved = events
            .iter()
            .filter(|e| matches!(e, Event::RoomResolved { .. }))
            .count();
        assert_eq!(resolved, 2, "both rooms reported despite lookup failures");
        assert!(events.iter().any(|e| matches!(e, Event::Warning(_))));
    }

    #[tokio::test]
    async fn suppresses_a_consecutive_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Straight2"));
        write(&path, &room_line("Straight2"));
        write(&path, &room_line("LeftTurn1"));

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();

        let entered: Vec<_> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                Event::RoomEntered { name } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(entered, vec!["Straight2", "LeftTurn1"]);
    }

    #[tokio::test]
    async fn a_run_start_announces_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Start"));

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();

        let events = drain(&mut rx);
        let run = events
            .iter()
            .position(|e| matches!(e, Event::RunStarted))
            .expect("RunStarted emitted");
        let entered = events
            .iter()
            .position(|e| matches!(e, Event::RoomEntered { .. }))
            .expect("RoomEntered emitted");

        assert!(
            run < entered,
            "clear the old run before reporting the new one"
        );
    }

    #[tokio::test]
    async fn a_repeat_after_a_new_run_is_not_suppressed() {
        // Entering Straight2, finishing the run, then entering Straight2 again
        // must report twice — the suppression is per-run, not global.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Straight2"));
        write(&path, &room_line("Start"));
        write(&path, &room_line("Straight2"));

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();

        let entered: Vec<_> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                Event::RoomEntered { name } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(entered, vec!["Straight2", "Start", "Straight2"]);
    }

    #[tokio::test]
    async fn nothing_new_emits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Straight2"));

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();
        drain(&mut rx);

        assert_eq!(scanner.pump(&mut tailer).await.unwrap(), 0);
        assert!(drain(&mut rx).is_empty(), "an idle poll is silent");
    }

    #[tokio::test]
    async fn a_closed_channel_does_not_panic() {
        // The UI closing must look like shutdown, not a crash in a worker.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(&path, &room_line("Straight2"));

        let (mut scanner, rx) = offline_scanner();
        drop(rx);

        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.expect("pump survives");
    }

    #[tokio::test]
    async fn private_server_addresses_are_never_geolocated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("player.log");
        write(
            &path,
            "2026-01-01T00:00:00Z,1.0,a,7 [FLog::Network] UDMUX Address = 10.8.4.154, Port = 1\n",
        );

        let (mut scanner, mut rx) = offline_scanner();
        let mut tailer = Tailer::at_offset(&path, 0);
        scanner.pump(&mut tailer).await.unwrap();

        let located = drain(&mut rx)
            .into_iter()
            .find_map(|e| match e {
                Event::ServerLocated { location, .. } => Some(*location),
                _ => None,
            })
            .expect("ServerLocated emitted");
        assert!(located.is_none(), "a private address must not be resolved");
    }
}
