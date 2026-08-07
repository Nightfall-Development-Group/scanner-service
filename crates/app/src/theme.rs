//! Palette and visual setup.
//!
//! One place, applied once. v1 had six byte-identical stylesheet dictionaries
//! copy-pasted across files, and one copy had silently lost a hover rule.
//!
//! Fonts need no special handling: egui embeds its own (Ubuntu-Light for
//! proportional, Hack for monospace) and ships them in the binary, so text
//! renders identically on all three platforms. v1 hardcoded "Segoe UI",
//! "Consolas" and "OCR A Extended" in ~20 places, which silently substituted to
//! something else on macOS and Linux.

use egui::{Color32, Context, CornerRadius, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(14, 16, 20);
pub const PANEL: Color32 = Color32::from_rgb(20, 23, 28);
pub const TITLE_BAR: Color32 = Color32::from_rgb(10, 12, 15);
pub const BORDER: Color32 = Color32::from_rgb(38, 43, 52);

pub const TEXT: Color32 = Color32::from_rgb(222, 227, 235);
pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 147, 162);
pub const ACCENT: Color32 = Color32::from_rgb(94, 176, 255);
pub const GOOD: Color32 = Color32::from_rgb(102, 201, 143);
pub const WARN: Color32 = Color32::from_rgb(232, 176, 84);
pub const BAD: Color32 = Color32::from_rgb(235, 108, 108);

pub const ROUNDING: u8 = 5;

/// Install the palette. Called once at startup.
pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = TITLE_BAR;
    visuals.override_text_color = Some(TEXT);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(31, 36, 44);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 51, 62);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.active.bg_fill = Color32::from_rgb(56, 65, 79);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);

    // Pin the theme rather than following the OS.
    //
    // `Context::set_visuals` writes to whichever theme is active *at the moment
    // it is called* — `style_mut_of(self.theme(), …)`. This runs at startup,
    // before Windows has reported its system theme, so it lands on Dark; when
    // the OS then says "light mode", egui switches to the Light style, which we
    // never configured, and the app renders in egui's stock white with dark
    // text. Linux reports no system theme, so it stayed Dark there and the bug
    // was invisible in testing.
    //
    // This is a dark overlay by design, so express that directly and write the
    // palette into both variants so nothing can switch out from under us.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}

/// Colour for a status indicator.
pub fn status_color(status: &scanner_core::event::Status) -> Color32 {
    use scanner_core::event::Status;
    match status {
        Status::Watching => GOOD,
        Status::Searching => WARN,
        Status::Idle => TEXT_DIM,
        Status::Stopped { .. } => BAD,
    }
}

/// Short label for a status.
pub fn status_label(status: &scanner_core::event::Status) -> &'static str {
    use scanner_core::event::Status;
    match status {
        Status::Watching => "Scanning",
        Status::Searching => "Looking for log",
        Status::Idle => "Idle",
        Status::Stopped { .. } => "Stopped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scanner_core::event::Status;

    #[test]
    fn every_status_has_a_label_and_colour() {
        // A missing arm would be a compile error, but this pins the mapping so a
        // reordering does not silently swap "Scanning" and "Stopped".
        for (status, label, colour) in [
            (Status::Watching, "Scanning", GOOD),
            (Status::Searching, "Looking for log", WARN),
            (Status::Idle, "Idle", TEXT_DIM),
            (Status::Stopped { reason: "x".into() }, "Stopped", BAD),
        ] {
            assert_eq!(status_label(&status), label);
            assert_eq!(status_color(&status), colour);
        }
    }

    #[test]
    fn stopped_is_visually_distinct_from_scanning() {
        assert_ne!(
            status_color(&Status::Watching),
            status_color(&Status::Stopped {
                reason: String::new()
            })
        );
    }
}
