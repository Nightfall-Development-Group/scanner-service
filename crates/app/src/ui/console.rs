//! The console panel: user-facing scanner activity.

use egui::{RichText, ScrollArea};

use crate::app::{with_opacity, ScannerApp};
use crate::theme;

pub fn show(ui_root: &mut egui::Ui, app: &mut ScannerApp) {
    let opacity = app.effective_opacity();

    egui::Panel::bottom("console")
        .resizable(true)
        .default_size(140.0)
        .frame(
            egui::Frame::NONE
                .fill(with_opacity(theme::PANEL, opacity))
                .inner_margin(egui::Margin::same(8)),
        )
        .show(ui_root, |ui| {
            ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .id_salt("console_scroll")
                .show(ui, |ui| {
                    // Each line is its own label rather than one giant string.
                    // v1 rebuilt the whole document per message, which was
                    // quadratic and wiped the user's text selection every time.
                    for line in &app.state.console {
                        ui.label(
                            RichText::new(line)
                                .family(egui::FontFamily::Monospace)
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                        );
                    }
                });
        });
}
