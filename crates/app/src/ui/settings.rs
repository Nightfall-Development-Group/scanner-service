//! Settings, including first-run API key entry.

use egui::RichText;

use crate::app::ScannerApp;
use crate::theme;

pub fn show(ctx: &egui::Context, app: &mut ScannerApp) {
    let mut open = app.ui.show_settings;
    if !open {
        return;
    }

    let mut restart_scan = false;
    let mut window_changed = false;

    egui::Window::new("Settings")
        .open(&mut open)
        .resizable(false)
        .default_width(380.0)
        .anchor(egui::Align2::RIGHT_TOP, [-12.0, 46.0])
        .show(ctx, |ui| {
            ui.label(RichText::new("API key").strong());
            ui.label(
                RichText::new(
                    "The database has no anonymous access, so the scanner needs your \
                     personal key. It is stored only on this machine.",
                )
                .size(11.0)
                .color(theme::TEXT_DIM),
            );

            // Masked: this is a credential, and the window may be on screen
            // while the user is streaming or screen-sharing.
            let key = egui::TextEdit::singleline(&mut app.ui.key_draft)
                .password(true)
                .hint_text("paste your key")
                .desired_width(f32::INFINITY);
            ui.add(key);

            ui.horizontal(|ui| {
                let changed = app.ui.key_draft.trim() != app.config.api_key;
                if ui
                    .add_enabled(changed, egui::Button::new("Save and reconnect"))
                    .clicked()
                {
                    app.config.api_key = app.ui.key_draft.trim().to_string();
                    app.mark_dirty();
                    restart_scan = true;
                }
                if app.config.api_key.is_empty() {
                    ui.label(RichText::new("required").size(11.0).color(theme::BAD));
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Log file").strong());
            ui.label(
                RichText::new("Leave blank to detect Roblox's log directory automatically.")
                    .size(11.0)
                    .color(theme::TEXT_DIM),
            );

            let path = egui::TextEdit::singleline(&mut app.ui.log_path_draft)
                .hint_text("auto-detect")
                .desired_width(f32::INFINITY);
            ui.add(path);

            ui.horizontal(|ui| {
                let current = app
                    .config
                    .log_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let changed = app.ui.log_path_draft.trim() != current;
                if ui
                    .add_enabled(changed, egui::Button::new("Apply"))
                    .clicked()
                {
                    let trimmed = app.ui.log_path_draft.trim();
                    // Unlike v1, this takes effect immediately: the path is read
                    // when the scan starts, not once at module import, so there
                    // is no ">> RESTART REQUIRED <<".
                    app.config.log_path = (!trimmed.is_empty()).then(|| trimmed.into());
                    app.mark_dirty();
                    restart_scan = true;
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Window").strong());

            let opacity = ui.add(
                egui::Slider::new(&mut app.config.window.opacity, 0.3..=1.0)
                    .text("Opacity")
                    .fixed_decimals(2),
            );
            let scale = ui.add(
                egui::Slider::new(&mut app.config.window.scale, 0.7..=2.0)
                    .text("Scale")
                    .fixed_decimals(2),
            );
            let on_top = ui.checkbox(&mut app.config.window.always_on_top, "Always on top");

            // Saving on release rather than on every tick. v1 wrote the entire
            // config file synchronously per slider step.
            if opacity.changed() || scale.changed() || on_top.changed() {
                app.mark_dirty();
                window_changed = true;
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Images").strong());
            if ui
                .checkbox(&mut app.config.images.auto_rotate, "Rotate automatically")
                .changed()
            {
                app.mark_dirty();
            }
            if app.config.images.auto_rotate
                && ui
                    .add(
                        egui::Slider::new(&mut app.config.images.rotate_interval_secs, 2.0..=20.0)
                            .text("Seconds")
                            .fixed_decimals(0),
                    )
                    .changed()
            {
                app.mark_dirty();
            }
        });

    app.ui.show_settings = open;

    if window_changed {
        app.apply_window_settings(ctx);
    }
    if restart_scan && !app.config.api_key.trim().is_empty() {
        app.start_scan(ctx);
    }
}
