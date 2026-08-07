//! Replay a Roblox log through the full scanner pipeline and print the events.
//!
//! This is the headless equivalent of running the app: it exercises tailing,
//! parsing, run detection, room resolution and geolocation, with no display and
//! no GUI code. Useful for verifying a change end to end, and for diagnosing a
//! user's log without asking them to run a build.
//!
//! ```sh
//! NFD_API_KEY=$(cat ~/.nfd-scanner-dev-key) \
//!   cargo run -p scanner-core --example replay -- path/to/player.log
//! ```
//!
//! Without `NFD_API_KEY` it still parses and reports rooms, but every lookup
//! fails — which is itself a useful demonstration that resolution failures do
//! not stop the scan.

use std::path::PathBuf;
use std::sync::Arc;

use scanner_core::api::ApiClient;
use scanner_core::engine::Scanner;
use scanner_core::event::{Event, Status};
use scanner_core::logsrc::Tailer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .ok_or("usage: replay <log file>")?
        .into();

    let key = std::env::var("NFD_API_KEY").unwrap_or_default();
    let client = Arc::new(if key.trim().is_empty() {
        eprintln!("note: NFD_API_KEY unset; lookups will fail\n");
        // A key that will be rejected, so the failure path is exercised rather
        // than the client refusing to build.
        ApiClient::new("unset")?
    } else {
        ApiClient::new(key)?
    });

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut scanner = Scanner::new(client, tx);

    // Start where the app would: the most recent run, not the top of the file.
    let (mut tailer, point) = Tailer::at_run_start(&path)?;
    println!(
        "resuming at byte {} of {}",
        point.offset,
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    println!();

    // Apply what the pre-scan established (server address, current room). This
    // is the same call the app makes, so geolocation is exercised here too.
    scanner.seed(&point).await;

    // Drain in the background so `pump` is never blocked by the printer, exactly
    // as the UI thread would consume the channel each frame.
    let printer = tokio::spawn(async move {
        let mut rooms = 0usize;
        let mut undocumented = 0usize;
        while let Some(event) = rx.recv().await {
            match event {
                Event::RunStarted => println!("=== run started ==="),
                Event::RoomEntered { name } => println!("  -> {name}"),
                Event::RoomResolved { name, room } => match *room {
                    Some(r) => {
                        rooms += 1;
                        println!(
                            "     {} [{}] {} image(s){}",
                            r.case_name,
                            r.roomtype,
                            r.images.len(),
                            if r.description.is_empty() {
                                String::new()
                            } else {
                                format!(" — {:.60}…", r.description)
                            }
                        );
                    }
                    None => {
                        undocumented += 1;
                        println!("     {name}: undocumented");
                    }
                },
                Event::ServerLocated { ip, location } => match *location {
                    Some(l) => println!("server {ip} — {}", l.describe()),
                    None => println!("server {ip} — not geolocatable"),
                },
                Event::Warning(w) => println!("  !! {w}"),
                Event::Status(Status::Stopped { reason }) => println!("  !! stopped: {reason}"),
                // Console and debug lines are the UI's business, not this tool's.
                _ => {}
            }
        }
        (rooms, undocumented)
    });

    let mut total = 0usize;
    loop {
        let consumed = scanner.pump(&mut tailer).await?;
        total += consumed;
        if consumed == 0 {
            break;
        }
    }

    drop(scanner);
    let (resolved, undocumented) = printer.await?;

    println!("\n{total} lines, {resolved} resolved, {undocumented} undocumented");
    Ok(())
}
