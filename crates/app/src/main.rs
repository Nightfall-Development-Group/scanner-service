//! Franktorio Research Scanner.
//!
//! A frameless, always-on-top overlay that follows the Roblox client log and
//! shows what researchers have documented about the room you are standing in.

// No console window on Windows in release builds. Kept in debug so `println!`
// and panics remain visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod state;
mod theme;
mod ui;

use app::ScannerApp;

/// Per-surface alpha needs a compositor to blend against. X11 sessions running a
/// bare window manager with no compositing render a transparent window as solid
/// black instead, which makes the app look broken and unrecoverable — the
/// settings panel is invisible too, so the user cannot turn opacity back up.
///
/// `SCANNER_OPAQUE=1` requests an ordinary opaque window as a way out.
fn transparency_requested() -> bool {
    !matches!(
        std::env::var("SCANNER_OPAQUE").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn main() -> eframe::Result<()> {
    let transparent = transparency_requested();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Franktorio Research Scanner")
            .with_inner_size([900.0, 640.0])
            .with_min_inner_size([460.0, 320.0])
            // Frameless: the title bar is ours, so the window can sit over a
            // fullscreen-windowed game without OS chrome.
            .with_decorations(false)
            .with_transparent(transparent)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_app_id("com.nightfalldivision.scanner"),
        ..Default::default()
    };

    eframe::run_native(
        "Franktorio Research Scanner",
        options,
        Box::new(move |cc| Ok(Box::new(ScannerApp::new(cc, transparent)))),
    )
}
