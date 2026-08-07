//! A single line of context: what we are watching, and where the server is.

use egui::RichText;

use crate::app::ScannerApp;
use crate::theme;

pub fn show(ui_root: &mut egui::Ui, app: &mut ScannerApp) {
    egui::Panel::bottom("status_bar")
        .exact_size(24.0)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 0)))
        .show(ui_root, |ui| {
            ui.horizontal_centered(|ui| {
                let small = |t: String| RichText::new(t).size(10.0).color(theme::TEXT_DIM);

                match &app.state.watching {
                    Some(file) => ui.label(small(format!("log: {file}"))),
                    None => ui.label(small("log: none".into())),
                };

                if let Some(server) = &app.state.server {
                    ui.separator();
                    ui.label(small(format!("server: {}", server.describe())))
                        .on_hover_text(&server.ip);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(small(format!("v{}", env!("CARGO_PKG_VERSION"))));
                });
            });
        });
}
