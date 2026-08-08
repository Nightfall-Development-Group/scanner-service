//! Franktorio Research Scanner.
//!
//! A frameless, always-on-top overlay that follows the Roblox client log and
//! shows what researchers have documented about the room you are standing in.

// No console window on Windows in release builds. Kept in debug so `println!`
// and panics remain visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod redirection_surface;
mod state;
mod textures;
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

/// Send eframe/wgpu logging to a file beside the config.
///
/// The release build has no console on Windows, so without this the surface and
/// alpha-mode messages that explain rendering problems are simply lost. The one
/// worth looking for is egui_wgpu's warning that the surface does not support a
/// `CompositeAlphaMode` with transparency.
fn start_logging() -> Option<std::path::PathBuf> {
    let path = scanner_core::config::Config::default_path()
        .ok()?
        .with_file_name("scanner.log");
    std::fs::create_dir_all(path.parent()?).ok()?;
    let file = std::fs::File::create(&path).ok()?;

    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("egui_wgpu", log::LevelFilter::Debug)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("wgpu_hal", log::LevelFilter::Warn)
        .format_timestamp_millis()
        .target(env_logger::Target::Pipe(Box::new(file)))
        .try_init()
        .ok()?;

    Some(path)
}

fn main() -> eframe::Result<()> {
    let log_path = start_logging();
    let transparent = transparency_requested();
    log::info!("starting; transparent window requested: {transparent}");
    if let Some(p) = &log_path {
        log::info!("logging to {}", p.display());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Franktorio Research Scanner")
            .with_inner_size([900.0, 640.0])
            .with_min_inner_size([460.0, 320.0])
            // Frameless: the title bar is ours, so the window can sit over a
            // fullscreen-windowed game without OS chrome.
            .with_decorations(false)
            .with_transparent(transparent)
            // Created hidden and shown by `ScannerApp::new` once rendering is
            // ready. Showing it earlier exposes the window during wgpu's
            // ~1 second of startup, where on Windows it renders as solid white
            // (see `redirection_surface.rs` for the mechanism).
            .with_visible(false)
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
