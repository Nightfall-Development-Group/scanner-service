//! The main panel: the room the player is in, plus this run's history.

use egui::{RichText, ScrollArea};
use scanner_core::api::{Room, RoomAttributes};

use crate::app::{with_opacity, ScannerApp};
use crate::state::RoomEntry;
use crate::theme;

pub fn show(ui_root: &mut egui::Ui, app: &mut ScannerApp) {
    let opacity = app.effective_opacity();

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(with_opacity(theme::BG, opacity))
                .inner_margin(egui::Margin::same(12)),
        )
        .show(ui_root, |ui| {
            if let Some(err) = app.config_error.clone() {
                config_error_banner(ui, app, &err);
            }
            if let Some(warning) = &app.state.warning {
                banner(ui, theme::WARN, warning);
            }

            match &app.state.current {
                Some(entry) => current_room(ui, entry),
                None => waiting(ui, app),
            }

            if !app.state.history.is_empty() {
                ui.add_space(10.0);
                ui.separator();
                history(ui, app);
            }
        });
}

fn current_room(ui: &mut egui::Ui, entry: &RoomEntry) {
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
        Some(room) => documented(ui, room),
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

fn documented(ui: &mut egui::Ui, room: &Room) {
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

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // Image count only for now; the carousel lands in M5.
        ui.label(
            RichText::new(format!("{} image(s)", room.images.len()))
                .size(11.0)
                .color(theme::TEXT_DIM),
        );
        if let Some(c) = &room.contributor {
            ui.label(
                RichText::new(format!("documented by {}", c.display_name))
                    .size(11.0)
                    .color(theme::TEXT_DIM),
            );
        }
    });
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
