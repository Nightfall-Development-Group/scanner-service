//! The eframe application: owns state, drives the engine, draws the UI.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::Color32;
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
    /// Opacity actually in effect. Falls back to fully opaque when the window
    /// has no alpha channel to blend through.
    pub fn effective_opacity(&self) -> f32 {
        if self.transparent {
            self.config.window.opacity
        } else {
            1.0
        }
    }
}

/// Apply the user's opacity to a colour.
///
/// The window itself is transparent and every surface is painted with alpha,
/// rather than asking the OS to make the window translucent. Native window
/// opacity is inconsistent across platforms; this looks identical everywhere.
pub fn with_opacity(color: Color32, opacity: f32) -> Color32 {
    let a = (opacity.clamp(0.15, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

impl eframe::App for ScannerApp {
    /// Transparent, so the alpha we paint with is what the user sees.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
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
    fn opacity_scales_alpha_without_shifting_hue() {
        // Color32 stores premultiplied alpha, so the direct accessors return
        // darkened components by design. The unmultiplied round-trip is what
        // shows the colour itself is unchanged.
        let c = with_opacity(Color32::from_rgb(10, 20, 30), 0.5);
        let [r, g, b, a] = c.to_srgba_unmultiplied();

        assert!((126..=129).contains(&a), "alpha was {a}");
        // Premultiplication is lossy at low alpha; a couple of levels of drift
        // is expected and invisible.
        for (got, want) in [(r, 10), (g, 20), (b, 30)] {
            assert!(
                got.abs_diff(want) <= 2,
                "component drifted: got {got}, want ~{want}"
            );
        }
    }

    #[test]
    fn full_opacity_leaves_a_colour_untouched() {
        let original = Color32::from_rgb(10, 20, 30);
        assert_eq!(with_opacity(original, 1.0), original);
    }

    #[test]
    fn opacity_is_clamped_so_the_window_cannot_vanish() {
        // A slider dragged to zero would otherwise make the app invisible and
        // unrecoverable, since the settings panel would be invisible too.
        assert!(with_opacity(Color32::WHITE, 0.0).a() > 0);
        assert_eq!(with_opacity(Color32::WHITE, 5.0).a(), 255);
    }
}
