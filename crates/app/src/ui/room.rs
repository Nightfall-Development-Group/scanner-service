//! The main panel: the room the player is in, plus this run's history.

use egui::{RichText, ScrollArea};
use scanner_core::api::{Room, RoomAttributes};

use crate::app::ScannerApp;
use crate::theme;

/// Cap on the carousel's on-screen height. The image itself may be smaller;
/// `egui::Image` never upscales past this, only shrinks to fit within it.
const IMAGE_MAX_HEIGHT: f32 = 260.0;

pub fn show(ui_root: &mut egui::Ui, app: &mut ScannerApp) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(12)))
        .show(ui_root, |ui| {
            if let Some(err) = app.config_error.clone() {
                config_error_banner(ui, app, &err);
            }
            if let Some(warning) = &app.state.warning {
                banner(ui, theme::WARN, warning);
            }

            // `.is_some()` rather than matching `&app.state.current` directly,
            // so the borrow does not outlive this check — `current_room` needs
            // `app` mutably, for the carousel's texture cache and navigation.
            if app.state.current.is_some() {
                current_room(ui, app);
            } else {
                waiting(ui, app);
            }

            if !app.state.history.is_empty() {
                ui.add_space(10.0);
                ui.separator();
                history(ui, app);
            }
        });
}

fn current_room(ui: &mut egui::Ui, app: &mut ScannerApp) {
    // Cloned rather than borrowed: rendering the carousel below needs `app`
    // mutably (texture lookups touch the LRU's recency order, navigation
    // buttons mutate `app.state`), which a live borrow of `app.state.current`
    // would forbid. A `Room` is a handful of strings and a short `Vec` of
    // images — cheap enough to clone once per frame next to the cost of
    // rebuilding the rest of the UI tree, which immediate-mode does anyway.
    let entry = app
        .state
        .current
        .clone()
        .expect("caller checked current.is_some()");

    ui.horizontal(|ui| {
        ui.label(RichText::new(entry.title()).size(22.0).strong());
        if let Some(room) = &entry.room {
            ui.label(
                RichText::new(&room.roomtype)
                    .color(theme::ACCENT)
                    .size(12.0),
            );
        }
    });

    match &entry.room {
        Some(room) => documented(ui, app, room),
        None => {
            ui.add_space(4.0);
            ui.label(
                RichText::new("Not documented yet \u{2014} or still resolving.")
                    .color(theme::TEXT_DIM)
                    .italics(),
            );
        }
    }
}

fn documented(ui: &mut egui::Ui, app: &mut ScannerApp, room: &Room) {
    if !room.tags.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for tag in &room.tags {
                ui.label(RichText::new(tag).size(11.0).color(theme::TEXT_DIM));
            }
        });
    }

    ui.add_space(8.0);
    ScrollArea::vertical()
        .max_height(220.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if room.description.trim().is_empty() {
                ui.label(
                    RichText::new("No description.")
                        .color(theme::TEXT_DIM)
                        .italics(),
                );
            } else {
                ui.label(RichText::new(&room.description).color(theme::TEXT));
            }
        });

    if let Some(attrs) = &room.attributes {
        ui.add_space(8.0);
        attributes(ui, attrs);
    }

    if !room.images.is_empty() {
        ui.add_space(8.0);
        carousel(ui, app);
    }

    if let Some(c) = &room.contributor {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("documented by {}", c.display_name))
                .size(11.0)
                .color(theme::TEXT_DIM),
        );
    }
}

/// The image gallery: current image, prev/next, a position counter, caption.
fn carousel(ui: &mut egui::Ui, app: &mut ScannerApp) {
    handle_carousel_keys(ui, app);

    let count = app.state.image_count();
    let index = app.state.current_image_index;
    let current = app.state.current_image().cloned();

    egui::Frame::NONE
        .fill(theme::PANEL)
        .corner_radius(egui::CornerRadius::same(theme::ROUNDING))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_min_height(IMAGE_MAX_HEIGHT * 0.4);
            ui.vertical_centered(|ui| {
                let Some(image) = &current else { return };
                // An if/else chain rather than `match .. { .. if .. }`: a match
                // guard keeps the first arm's borrow of `app.images` alive
                // while evaluating the second arm's condition, which then
                // cannot borrow it again. Each condition here is its own
                // statement, so each borrow ends before the next begins.
                if let Some(texture) = app.images.get(&image.image_url) {
                    ui.add(
                        egui::Image::from_texture(texture)
                            .max_height(IMAGE_MAX_HEIGHT)
                            .max_width(ui.available_width())
                            .corner_radius(egui::CornerRadius::same(theme::ROUNDING)),
                    );
                } else if app.images.failed(&image.image_url) {
                    loading_placeholder(ui, "Image failed to load");
                } else {
                    ui.add_space(IMAGE_MAX_HEIGHT * 0.15);
                    ui.spinner();
                }
            });
        });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(count > 1, egui::Button::new("< Prev"))
            .clicked()
        {
            app.retreat_image();
        }
        ui.label(
            RichText::new(format!("{} / {count}", index + 1))
                .size(11.0)
                .color(theme::TEXT_DIM),
        );
        if ui
            .add_enabled(count > 1, egui::Button::new("Next >"))
            .clicked()
        {
            app.advance_image();
        }
        if let Some(caption) = current.as_ref().and_then(|i| i.caption.as_deref()) {
            ui.label(
                RichText::new(caption)
                    .size(11.0)
                    .color(theme::TEXT_DIM)
                    .italics(),
            );
        }
    });
}

fn loading_placeholder(ui: &mut egui::Ui, text: &str) {
    ui.add_space(IMAGE_MAX_HEIGHT * 0.15);
    ui.label(RichText::new(text).color(theme::TEXT_DIM).italics());
}

/// `,`/`.` (matching v1) and the arrow keys, but only when no widget has
/// keyboard focus — otherwise these would fight with cursor movement while
/// typing in the API key or log path fields in Settings.
fn handle_carousel_keys(ui: &egui::Ui, app: &mut ScannerApp) {
    if app.state.image_count() <= 1 {
        return;
    }
    let ctx = ui.ctx();
    if ctx.memory(|m| m.focused().is_some()) {
        return;
    }
    let (prev, next) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Comma) || i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::Period) || i.key_pressed(egui::Key::ArrowRight),
        )
    });
    if prev {
        app.retreat_image();
    }
    if next {
        app.advance_image();
    }
}

/// Render observed properties.
///
/// Only fields that were actually observed are shown. A `None` means nobody has
/// checked, which is genuinely different from "checked and found none" — showing
/// "no turrets" for an unexamined room would be inventing data.
fn attributes(ui: &mut egui::Ui, attrs: &RoomAttributes) {
    let mut hazards: Vec<&str> = Vec::new();
    for (present, label) in [
        (attrs.has_water, "water"),
        (attrs.has_fire, "fire"),
        (attrs.has_pit, "pit"),
        (attrs.has_lava, "lava"),
        (attrs.has_electricity, "electricity"),
        (attrs.has_turrets, "turrets"),
        (attrs.has_vents, "vents"),
        (attrs.has_steam, "steam"),
        (attrs.has_fans, "fans"),
        (attrs.has_siderooms, "siderooms"),
        (attrs.has_code_breacher_door, "code breacher door"),
    ] {
        if present == Some(true) {
            hazards.push(label);
        }
    }

    if !hazards.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Contains:").size(11.0).color(theme::TEXT_DIM));
            ui.label(
                RichText::new(hazards.join(", "))
                    .size(11.0)
                    .color(theme::WARN),
            );
        });
    }

    let keycard = [
        (attrs.guaranteed_keycard, "guaranteed keycard"),
        (attrs.sometimes_keycard, "sometimes keycard"),
        (attrs.purple_keycard, "purple keycard"),
    ]
    .into_iter()
    .find(|(v, _)| *v == Some(true))
    .map(|(_, l)| l);

    if let Some(k) = keycard {
        ui.label(RichText::new(k).size(11.0).color(theme::GOOD));
    }

    if let Some(entrances) = attrs.entrances {
        ui.label(
            RichText::new(format!("{entrances} entrance(s)"))
                .size(11.0)
                .color(theme::TEXT_DIM),
        );
    }
}

fn waiting(ui: &mut egui::Ui, app: &ScannerApp) {
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        let message = if app.config.api_key.trim().is_empty() {
            "Enter your API key in Settings to begin."
        } else if app.state.is_scanning() {
            "Waiting for a room \u{2014} join a game and start a run."
        } else {
            "Press Start scan."
        };
        ui.label(RichText::new(message).color(theme::TEXT_DIM).size(14.0));
    });
}

fn history(ui: &mut egui::Ui, app: &ScannerApp) {
    ui.label(
        RichText::new(format!(
            "This run \u{2014} {} rooms",
            app.state.history.len()
        ))
        .size(11.0)
        .color(theme::TEXT_DIM),
    );
    ui.add_space(4.0);

    ScrollArea::vertical()
        .max_height(140.0)
        .auto_shrink([false, true])
        .id_salt("history")
        .show(ui, |ui| {
            for entry in app.state.recent() {
                ui.horizontal(|ui| {
                    let colour = if entry.is_documented() {
                        theme::TEXT
                    } else {
                        theme::TEXT_DIM
                    };
                    ui.label(RichText::new(entry.title()).size(12.0).color(colour));
                    if let Some(room) = &entry.room {
                        ui.label(
                            RichText::new(&room.roomtype)
                                .size(10.0)
                                .color(theme::TEXT_DIM),
                        );
                    }
                });
            }
        });
}

fn banner(ui: &mut egui::Ui, colour: egui::Color32, text: &str) {
    egui::Frame::NONE
        .fill(colour.linear_multiply(0.15))
        .inner_margin(egui::Margin::same(6))
        .corner_radius(egui::CornerRadius::same(theme::ROUNDING))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(colour).size(12.0));
        });
    ui.add_space(6.0);
}

fn config_error_banner(ui: &mut egui::Ui, app: &mut ScannerApp, err: &str) {
    egui::Frame::NONE
        .fill(theme::BAD.linear_multiply(0.15))
        .inner_margin(egui::Margin::same(6))
        .corner_radius(egui::CornerRadius::same(theme::ROUNDING))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("Settings could not be read: {err}"))
                    .color(theme::BAD)
                    .size(12.0),
            );
            ui.label(
                RichText::new(
                    "Your saved settings are intact on disk and will not be overwritten. \
                     Choose to replace them only if you are happy to lose them.",
                )
                .color(theme::TEXT_DIM)
                .size(11.0),
            );
            if ui.button("Replace with defaults").clicked() {
                app.clear_config_error();
            }
        });
    ui.add_space(6.0);
}
