//! Workaround for the opaque white backdrop behind transparent windows on
//! Windows.
//!
//! winit implements `with_transparent` on Windows through
//! `DwmEnableBlurBehindWindow` rather than `WS_EX_NOREDIRECTIONBITMAP`. That
//! leaves the window's GDI redirection surface — a system-managed backing
//! bitmap — in DWM's composition stack, *underneath* the wgpu swapchain.
//! Windows fills that surface with opaque white when the window is first
//! shown, and nothing ever repaints it, because this app draws exclusively
//! through the swapchain. The result is the window's translucent content
//! correctly alpha-blended over solid white instead of over the desktop.
//!
//! Resizing makes the artifact stranger rather than fixing it: during a live
//! resize DWM carries the surface's old content forward, so shrinking
//! truncates the white area and enlarging zero-fills the newly exposed part.
//! Zeroed bytes are premultiplied transparent black — which is why after a
//! shrink-and-regrow the window is transparent *except* a white rectangle the
//! size of the window at its smallest.
//!
//! The fix follows from the same fact: GDI draws land in the redirection
//! surface, and `PatBlt(BLACKNESS)` writes 0x00000000 — transparent black —
//! over all of it. Once zeroed, every copy DWM makes of it stays zero, so this
//! only needs doing at startup and after events that may let Windows repaint
//! it (showing, restoring from minimize, resizes).
//!
//! `WS_EX_NOREDIRECTIONBITMAP` (no surface at all, the path Chromium uses)
//! would be the root-cause fix, but it can only be applied when the window is
//! created, and egui-winit 0.36 offers no way to pass it through.

/// Zeroes the redirection surface for the first few frames after a
/// trigger event (startup, resize, scale change, restore-from-minimize).
///
/// A burst of frames rather than a single shot, because the white fill is
/// asynchronous with respect to our frame loop: it can land after the event
/// that provoked it. A handful of consecutive overwrites closes the race for
/// good, and each is a sub-millisecond GDI fill.
pub struct Cleaner {
    /// `None` when disabled: non-Windows, opaque window, or no Win32 handle.
    hwnd: Option<isize>,
    /// Frames left in the current zeroing burst.
    armed: u32,
    last_size: Option<egui::Vec2>,
    last_ppp: Option<f32>,
    was_minimized: bool,
}

/// Frames per burst. One would do in principle; a few absorb any late fill.
const BURST_FRAMES: u32 = 5;

impl Cleaner {
    /// `transparent` is whether the window was created with an alpha channel;
    /// an opaque window composites nothing, so there is nothing to clean.
    pub fn new(cc: &eframe::CreationContext<'_>, transparent: bool) -> Self {
        Self {
            hwnd: transparent.then(|| win32_handle(cc)).flatten(),
            armed: BURST_FRAMES,
            last_size: None,
            last_ppp: None,
            was_minimized: false,
        }
    }

    /// Call once per frame, before or after drawing — timing does not matter,
    /// because the surface being zeroed is not the one being drawn to.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let Some(hwnd) = self.hwnd else {
            return;
        };

        let size = ctx.content_rect().size();
        let ppp = ctx.pixels_per_point();
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));

        let resized = self.last_size.is_some_and(|s| s != size);
        let rescaled = self.last_ppp.is_some_and(|p| p != ppp);
        let restored = self.was_minimized && !minimized;
        self.last_size = Some(size);
        self.last_ppp = Some(ppp);
        self.was_minimized = minimized;

        if resized || rescaled || restored {
            self.armed = BURST_FRAMES;
        }

        if self.armed > 0 {
            self.armed -= 1;
            zero_redirection_surface(hwnd);
            // The app repaints on events, not continuously; keep frames coming
            // until the burst is done.
            ctx.request_repaint();
        }
    }
}

/// The window's Win32 handle, if there is one.
fn win32_handle(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    match cc.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Fill the client area with transparent black through GDI.
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
            return;
        }
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return;
        }
        PatBlt(hdc, 0, 0, rect.right, rect.bottom, BLACKNESS);
        ReleaseDC(hwnd, hdc);
    }
}

/// The redirection surface is a Windows concept; everywhere else per-surface
/// alpha goes straight to the compositor and there is nothing to clean.
#[cfg(not(target_os = "windows"))]
fn zero_redirection_surface(_hwnd: isize) {}
