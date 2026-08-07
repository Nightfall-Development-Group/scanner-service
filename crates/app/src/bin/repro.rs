//! A bare transparent eframe window containing none of the scanner's code.
//!
//! Purpose: decide whether the white-rectangle-on-resize artifact belongs to
//! eframe/wgpu or to this project. If it appears here, nothing in the scanner
//! can be responsible and the fix has to be a workaround for the backend. If it
//! does not appear here, the cause is ours and this gives a clean baseline to
//! bisect against.
//!
//! Deliberately minimal: no panels, no custom theme, no background tasks, no
//! frameless window. Decorations are left ON so the window is easy to resize,
//! since resizing is what triggers the artifact.
//!
//! Run it, shrink the window right down, then enlarge it again, over something
//! with visible detail (the game, a bright wallpaper) so translucency is
//! obvious.

use std::time::Instant;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("eframe transparency repro")
            .with_inner_size([700.0, 460.0])
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "eframe transparency repro",
        options,
        Box::new(|_cc| Ok(Box::new(Repro::default()))),
    )
}

struct Repro {
    opacity: f32,
    started: Instant,
    resizes: u32,
    last_size: Option<egui::Vec2>,
}

impl Default for Repro {
    fn default() -> Self {
        Self {
            opacity: 0.6,
            started: Instant::now(),
            resizes: 0,
            last_size: None,
        }
    }
}

impl eframe::App for Repro {
    /// Premultiplied, gamma space — a dark grey at the chosen opacity.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let o = self.opacity;
        [0.06 * o, 0.07 * o, 0.09 * o, o]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let size = ui.ctx().content_rect().size();
        if self.last_size.is_some_and(|s| s != size) {
            self.resizes += 1;
        }
        self.last_size = Some(size);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(16)))
            .show(ui, |ui| {
                ui.heading("eframe transparency repro");
                ui.add_space(8.0);
                ui.label("Shrink this window right down, then enlarge it again.");
                ui.label("If a white rectangle appears, the bug is in eframe, not the scanner.");

                ui.add_space(12.0);
                ui.add(egui::Slider::new(&mut self.opacity, 0.1..=1.0).text("opacity"));

                ui.add_space(12.0);
                ui.monospace(format!("backend      {BACKEND}"));
                ui.monospace(format!("system theme {:?}", ui.ctx().system_theme()));
                ui.monospace(format!("active theme {:?}", ui.ctx().theme()));
                ui.monospace(format!("size         {:.0} x {:.0}", size.x, size.y));
                ui.monospace(format!("pixels/point {:.2}", ui.ctx().pixels_per_point()));
                ui.monospace(format!("resizes seen {}", self.resizes));
                ui.monospace(format!(
                    "uptime       {:.0}s",
                    self.started.elapsed().as_secs_f32()
                ));

                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(
                        "The area below is painted opaque. Anything white that is NOT \
                         this bar is the artifact.",
                    )
                    .size(11.0),
                );
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 28.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(4),
                    egui::Color32::from_rgb(90, 150, 220),
                );
            });

        // Keep repainting so the uptime ticks and resizes are caught promptly.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Which renderer eframe linked.
///
/// Not detectable via `cfg!` — that reads *this* crate's features, and the
/// backend is chosen by eframe's. Update alongside the dependency.
const BACKEND: &str = "wgpu (eframe default)";
