//! The custom title bar.
//!
//! The window is frameless so it can sit over the game without OS chrome, which
//! means dragging, minimising and closing are ours to implement.
//!
//! Interaction order matters: the drag region is registered for the whole bar
//! first, then the buttons are drawn. egui gives later widgets pointer priority,
//! so a click on a button does not also start a window drag.
//!
//! Icons are **painted, not typed**. egui bundles its own fonts, and symbols
//! outside the common blocks — U+25CF for a status dot, U+2715 for a close
//! cross — are not in them, so they render as tofu boxes. egui paints its own
//! window close button with line segments for the same reason.

use egui::{Align, CornerRadius, Layout, Response, RichText, Sense, ViewportCommand};

use crate::app::{with_opacity, ScannerApp};
use crate::theme;

pub const HEIGHT: f32 = 34.0;

enum Icon {
    Close,
    Minimise,
}

/// A button whose glyph is drawn rather than rendered from a font.
fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 22.0), Sense::click());
    let visuals = ui.style().interact(&response);

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(3), visuals.bg_fill);
    }

    let glyph = rect.shrink2(egui::vec2(10.0, 7.0));
    let stroke = visuals.fg_stroke;
    match icon {
        Icon::Close => {
            ui.painter()
                .line_segment([glyph.left_top(), glyph.right_bottom()], stroke);
            ui.painter()
                .line_segment([glyph.right_top(), glyph.left_bottom()], stroke);
        }
        Icon::Minimise => {
            ui.painter()
                .line_segment([glyph.left_center(), glyph.right_center()], stroke);
        }
    }

    response.on_hover_text(tooltip)
}

/// A filled circle showing scanner status.
fn status_dot(ui: &mut egui::Ui, colour: egui::Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, colour);
    response
}

pub fn show(ui_root: &mut egui::Ui, app: &mut ScannerApp) {
    let opacity = app.effective_opacity();

    egui::Panel::top("title_bar")
        .exact_size(HEIGHT)
        .frame(
            egui::Frame::NONE
                .fill(with_opacity(theme::TITLE_BAR, opacity))
                .inner_margin(egui::Margin::symmetric(8, 0)),
        )
        .show(ui_root, |ui| {
            let ctx = ui.ctx().clone();

            let bar_rect = ui.max_rect();
            let drag = ui.interact(bar_rect, ui.id().with("drag"), Sense::click_and_drag());
            if drag.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }

            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("Scanner").strong().color(theme::TEXT));

                let status = app.state.status.clone();
                status_dot(ui, theme::status_color(&status)).on_hover_text(match &status {
                    scanner_core::event::Status::Stopped { reason } => reason.clone(),
                    other => theme::status_label(other).to_string(),
                });
                ui.label(
                    RichText::new(theme::status_label(&status))
                        .color(theme::TEXT_DIM)
                        .size(11.0),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, Icon::Close, "Close").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if icon_button(ui, Icon::Minimise, "Minimise").clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    }

                    ui.separator();

                    if ui
                        .selectable_label(app.ui.show_console, "Console")
                        .on_hover_text("Toggle the console")
                        .clicked()
                    {
                        app.ui.show_console = !app.ui.show_console;
                    }

                    if ui
                        .selectable_label(app.ui.show_settings, "Settings")
                        .clicked()
                    {
                        app.ui.show_settings = !app.ui.show_settings;
                    }

                    let scanning = app.state.is_scanning();
                    let label = if scanning { "Stop" } else { "Start scan" };
                    if ui.button(label).clicked() {
                        if scanning {
                            app.stop_scan();
                        } else {
                            app.start_scan(&ctx);
                        }
                    }
                });
            });
        });
}
