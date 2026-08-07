//! The eframe application: owns state, drives the engine, draws the UI.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use scanner_core::api::ApiClient;
use scanner_core::config::Config;
use scanner_core::engine;
use scanner_core::event::{Event, Status};
use tokio::sync::{mpsc, watch};

use crate::state::AppState;
use crate::{theme, ui};

/// Settings are saved this long after the last change rather than on every one.
///
/// v1 bound its opacity slider to `valueChanged` and wrote the whole JSON file
/// synchronously per tick — roughly 70 read-modify-writes for a single drag.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

pub struct ScannerApp {
    pub state: AppState,
    pub config: Config,
    config_path: Option<PathBuf>,
    /// Filled by a background task, drained once per frame.
    events: Arc<Mutex<VecDeque<Event>>>,
    /// Held for the process lifetime; dropping it would abort the engine.
    runtime: tokio::runtime::Runtime,
    shutdown: Option<watch::Sender<bool>>,
    pub ui: UiState,
    dirty_since: Option<Instant>,
    /// Surfaced in the UI rather than silently swallowed, so a config we could
    /// not read never gets quietly overwritten.
    pub config_error: Option<String>,
    /// Whether the window was created with an alpha channel. When it was not,
    /// surfaces are painted fully opaque regardless of the opacity setting,
    /// because alpha against a non-composited window reads as black.
    transparent: bool,
}

/// Purely presentational state: which panels are open, in-progress text entry.
#[derive(Default)]
pub struct UiState {
    pub show_settings: bool,
    pub show_console: bool,
    pub key_draft: String,
    pub log_path_draft: String,
}

impl ScannerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, transparent: bool) -> Self {
        theme::apply(&cc.egui_ctx);

        let config_path = Config::default_path().ok();
        let (config, config_error) = match &config_path {
            Some(p) => match Config::load(p) {
                Ok(c) => (c, None),
                // Start from defaults in memory but keep the error visible, and
                // critically do not save over the unreadable file.
                Err(e) => (Config::default(), Some(e.to_string())),
            },
            None => (
                Config::default(),
                Some("could not locate a config directory".into()),
            ),
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");

        let ui = UiState {
            // First run with no key: the app cannot do anything until one is
            // entered, so open settings rather than showing an empty window.
            show_settings: config.api_key.trim().is_empty(),
            key_draft: config.api_key.clone(),
            log_path_draft: config
                .log_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            show_console: true,
        };

        let mut app = Self {
            state: AppState::default(),
            config,
            config_path,
            events: Arc::new(Mutex::new(VecDeque::new())),
            runtime,
            shutdown: None,
            ui,
            dirty_since: None,
            config_error,
            transparent,
        };

        app.apply_window_settings(&cc.egui_ctx);
        if !app.config.api_key.trim().is_empty() {
            app.start_scan(&cc.egui_ctx);
        }
        app
    }

    /// Build a client and spawn the engine. Replaces any running scan.
    pub fn start_scan(&mut self, ctx: &egui::Context) {
        self.stop_scan();

        let client = match ApiClient::new(self.config.api_key.clone()) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                self.state.apply(Event::Status(Status::Stopped {
                    reason: e.to_string(),
                }));
                return;
            }
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = watch::channel(false);
        self.shutdown = Some(stop_tx);

        // Forwarder: moves events into the queue and wakes the UI. Waking on
        // arrival means the window repaints immediately when something happens
        // and costs nothing while idle — no polling repaint loop.
        let queue = Arc::clone(&self.events);
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Ok(mut q) = queue.lock() {
                    q.push_back(event);
                }
                ctx.request_repaint();
            }
        });

        let log_path = self.config.log_path.clone();
        self.runtime
            .spawn(engine::run(client, tx, log_path, stop_rx));
    }

    pub fn stop_scan(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        self.state.apply(Event::Status(Status::Idle));
    }

    /// Move queued events into state. Called once per frame.
    fn drain_events(&mut self) {
        let drained: Vec<Event> = match self.events.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => return,
        };
        for event in drained {
            self.state.apply(event);
        }
    }

    /// Note that settings changed; the write happens once they settle.
    pub fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    fn save_if_settled(&mut self) {
        let Some(since) = self.dirty_since else {
            return;
        };
        if since.elapsed() < SAVE_DEBOUNCE {
            return;
        }
        self.dirty_since = None;
        self.save_now();
    }

    pub fn save_now(&mut self) {
        // Refusing to write over a config we failed to parse is deliberate: the
        // user's real settings may still be in that file.
        if self.config_error.is_some() {
            return;
        }
        if let Some(path) = &self.config_path {
            if let Err(e) = self.config.save(path) {
                self.state
                    .apply(Event::Warning(format!("could not save settings: {e}")));
            }
        }
    }

    /// Push window-level settings to the platform.
    pub fn apply_window_settings(&self, ctx: &egui::Context) {
        ctx.set_zoom_factor(self.config.window.scale);
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            if self.config.window.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            },
        ));
    }

    /// Discard the config error, allowing saves again. Used by the settings
    /// panel's explicit "overwrite" action.
    pub fn clear_config_error(&mut self) {
        self.config_error = None;
        self.mark_dirty();
    }
}

impl ScannerApp {
    /// Opacity actually in effect.
    ///
    /// Forced to fully opaque when the window has no alpha channel, because
    /// alpha against a non-composited window reads as solid black.
    pub fn effective_opacity(&self) -> f32 {
        if self.transparent {
            usable_opacity(self.config.window.opacity)
        } else {
            1.0
        }
    }
}

/// Lower bound on opacity, so the window can never become invisible — the
/// settings panel would vanish with it and the user could not turn it back up.
const MIN_OPACITY: f32 = 0.15;

/// Clamp a configured opacity into the usable range.
fn usable_opacity(configured: f32) -> f32 {
    if configured.is_finite() {
        configured.clamp(MIN_OPACITY, 1.0)
    } else {
        1.0
    }
}

impl eframe::App for ScannerApp {
    /// The window background, faded by the opacity setting.
    ///
    /// This is a *framebuffer clear*, which is the whole point: it covers every
    /// pixel of the window every frame, at whatever size the window currently
    /// is. Painting the backdrop as a rectangle instead makes coverage depend on
    /// layout, so a stale or mis-sized rect leaves part of the window bright and
    /// part of it clear — visible as a rectangle that only disappears once a
    /// resize forces a relayout.
    ///
    /// Values must be premultiplied and in gamma space, per the trait docs.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let opacity = self.effective_opacity();
        let [r, g, b, _] = theme::BG.to_normalized_gamma_f32();
        [r * opacity, g * opacity, b * opacity, opacity]
    }

    /// Non-drawing work. eframe calls this before every `ui` pass, and also
    /// while the window is hidden — so events keep being folded into state even
    /// when nothing is being painted, rather than piling up in the queue.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.save_if_settled();

        // A pending save needs a later pass to actually happen.
        if self.dirty_since.is_some() {
            ctx.request_repaint_after(SAVE_DEBOUNCE);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Panels in 0.36 nest inside the root Ui; order fixes their placement.
        let ctx = ui.ctx().clone();

        // Fade the contents by the same factor as the background.
        //
        // Fading only the background is what made lower opacity look *whiter*:
        // text and buttons stayed fully opaque, so as the backdrop dropped away
        // the bright elements were all that remained. This multiplies the alpha
        // of everything drawn from here on, so the whole window fades together,
        // which is what an opacity slider is expected to do.
        ui.set_opacity(self.effective_opacity());

        ui::title_bar::show(ui, self);
        ui::status_bar::show(ui, self);
        if self.ui.show_console {
            ui::console::show(ui, self);
        }
        ui::room::show(ui, self);

        // A free-floating window, so it takes the Context rather than a Ui.
        ui::settings::show(&ctx, self);
    }

    fn on_exit(&mut self) {
        self.dirty_since = None;
        self.save_now();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_never_reaches_invisible() {
        // At zero the settings panel would be invisible too, so the user could
        // never turn opacity back up.
        assert_eq!(usable_opacity(0.0), MIN_OPACITY);
        assert_eq!(usable_opacity(-5.0), MIN_OPACITY);
        assert!(usable_opacity(0.0) > 0.0);
    }

    #[test]
    fn opacity_passes_through_in_range_and_caps_above() {
        assert_eq!(usable_opacity(0.5), 0.5);
        assert_eq!(usable_opacity(1.0), 1.0);
        assert_eq!(usable_opacity(3.0), 1.0);
    }

    #[test]
    fn a_corrupt_opacity_falls_back_to_opaque() {
        // A hand-edited config could contain NaN; clamp() would propagate it and
        // the window would render as nothing at all.
        assert_eq!(usable_opacity(f32::NAN), 1.0);
    }

    #[test]
    fn clear_colour_is_premultiplied() {
        // eframe sends these floats to the renderer as-is, so the RGB has to be
        // scaled by alpha or the background blends far too bright.
        let opacity = 0.5;
        let [r, g, b, _] = theme::BG.to_normalized_gamma_f32();
        let cleared = [r * opacity, g * opacity, b * opacity, opacity];

        assert!(cleared[0] <= cleared[3], "red exceeds alpha: {cleared:?}");
        assert!(cleared[1] <= cleared[3], "green exceeds alpha: {cleared:?}");
        assert!(cleared[2] <= cleared[3], "blue exceeds alpha: {cleared:?}");
    }
}
