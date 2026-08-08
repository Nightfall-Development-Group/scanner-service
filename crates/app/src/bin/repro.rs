//! A bare transparent eframe window containing none of the scanner's code.
//!
//! Originally written to decide whether the white-rectangle-on-resize artifact
//! belongs to eframe/wgpu or to this project. It appears here too, so the
//! scanner is not responsible. The cause, established by experiment on this
//! window: winit implements `with_transparent` on Windows via
//! `DwmEnableBlurBehindWindow`, which leaves the window's GDI redirection
//! surface underneath the wgpu swapchain in DWM's composition. Windows fills
//! that surface opaque white on first show; nothing repaints it; the window's
//! translucent content is then correctly alpha-blended over white instead of
//! the desktop. Live resizes truncate the stale white area (shrink) and
//! zero-fill new area (grow, zero = transparent black), which is why the
//! artifact ends up the size of the window at its smallest.
//!
//! By default this binary applies the same workaround as the scanner: zeroing
//! the redirection surface through GDI at startup and after resizes. Run with
//! `--raw` to disable the workaround and see the artifact itself, e.g. when
//! reproducing for an upstream report.
//!
//! Run it, shrink the window right down, then enlarge it again, over something
//! with visible detail (the game, a bright wallpaper) so translucency is
//! obvious.

use std::time::Instant;

/// Zeroing burst length; mirrors `redirection_surface::BURST_FRAMES` in the
/// scanner. A burst rather than one shot because the system's white fill can
/// land after the event that provoked it.
const BURST_FRAMES: u32 = 5;

/// Fill the window's client area with transparent black *through GDI*.
///
/// GDI draws land in the DWM redirection surface, and `PatBlt(BLACKNESS)`
/// writes 0x00000000 — premultiplied transparent black — over all of it. Once
/// zeroed, DWM's resize copies of the surface stay zero.
#[cfg(target_os = "windows")]
fn zero_redirection_surface(hwnd: isize) {
    use std::ffi::c_void;

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetDC(hwnd: isize) -> *mut c_void;
        fn ReleaseDC(hwnd: isize, hdc: *mut c_void) -> i32;
        fn GetClientRect(hwnd: isize, rect: *mut Rect) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn PatBlt(hdc: *mut c_void, x: i32, y: i32, w: i32, h: i32, rop: u32) -> i32;
    }

    /// Raster op: dest = 0. All four bytes, so alpha becomes 0 too.
    const BLACKNESS: u32 = 0x0042;

    // SAFETY: plain Win32 calls on a handle winit keeps alive for as long as
    // the app runs; each call checks the previous one's result.
    unsafe {
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut rect) == 0 {
            log::warn!("GetClientRect failed; skipping redirection-surface zero");
            return;
        }
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            log::warn!("GetDC failed; skipping redirection-surface zero");
            return;
        }
        let ok = PatBlt(hdc, 0, 0, rect.right, rect.bottom, BLACKNESS);
        ReleaseDC(hwnd, hdc);
        log::info!(
            "zeroed redirection surface ({}x{}), PatBlt ok={}",
            rect.right,
            rect.bottom,
            ok != 0
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn zero_redirection_surface(_hwnd: isize) {}

fn main() -> eframe::Result<()> {
    // eframe and wgpu report the surface's alpha-mode decision through `log`.
    // The line that matters is egui_wgpu's:
    //
    //   "Transparent window was requested, but the active wgpu surface does not
    //    support a `CompositeAlphaMode` with transparency."
    //
    // If it appears, the surface has no per-pixel alpha at all and the window
    // is opaque for that reason instead (as happens when wgpu ends up on DX12,
    // whose windowed swapchains report only opaque composition). This binary
    // has no `windows_subsystem = "windows"`, so it keeps a console and the log
    // is visible as it runs.
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("egui_wgpu", log::LevelFilter::Debug)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("wgpu_hal", log::LevelFilter::Warn)
        .format_timestamp_millis()
        .init();

    let fix = !std::env::args().any(|a| a == "--raw");
    log::info!(
        "repro starting; redirection-surface workaround {}",
        if fix {
            "ON (pass --raw to disable)"
        } else {
            "OFF"
        }
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("eframe transparency repro")
            .with_inner_size([700.0, 460.0])
            // Fixed position so a screenshot harness needs no SetWindowPos,
            // which would itself count as a resize-adjacent event.
            .with_position([100.0, 100.0])
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "eframe transparency repro",
        options,
        Box::new(move |cc| {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
                _ => None,
            };
            Ok(Box::new(Repro {
                hwnd: fix.then_some(hwnd).flatten(),
                ..Repro::default()
            }))
        }),
    )
}

struct Repro {
    opacity: f32,
    started: Instant,
    resizes: u32,
    last_size: Option<egui::Vec2>,
    /// `None` when the workaround is disabled or there is no Win32 handle.
    hwnd: Option<isize>,
    /// Frames left in the current zeroing burst.
    armed: u32,
    zaps: u32,
}

impl Default for Repro {
    fn default() -> Self {
        Self {
            opacity: 0.6,
            started: Instant::now(),
            resizes: 0,
            last_size: None,
            hwnd: None,
            armed: BURST_FRAMES,
            zaps: 0,
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
            // Resize: DWM may have copied stale surface content around.
            self.armed = BURST_FRAMES;
        }
        self.last_size = Some(size);

        if let Some(hwnd) = self.hwnd {
            if self.armed > 0 {
                self.armed -= 1;
                self.zaps += 1;
                zero_redirection_surface(hwnd);
            }
        }

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
                ui.monospace(format!(
                    "workaround   {} ({} zaps)",
                    if self.hwnd.is_some() { "on" } else { "off" },
                    self.zaps
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
