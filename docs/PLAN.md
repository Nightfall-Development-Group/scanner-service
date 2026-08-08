# Scanner v2 — implementation plan

## Context

`franktorio-pressure-scanner` (v1) is an unmaintained PyQt5 app that tails Roblox
logs, extracts room names, and shows what researchers documented about each room.
It works, but three things make it a dead end: it is Python-and-PyInstaller heavy,
its architecture puts blocking network I/O on the UI thread, and — decisively —
the backend it talks to no longer exists. The web stack was rebuilt around
`db-api.nightfalldivision.com`, and none of v1's endpoints survived.

So v2 is a ground-up rewrite in Rust against the new API. The goal is a small
single-file binary per platform, no runtime dependencies, and an architecture in
which v1's whole class of state-desync bugs cannot be expressed.

Windows is the primary target, Linux second, macOS for parity.

### Decisions taken

| Question | Decision |
|---|---|
| Client auth | Per-user API key, entered in settings. No secret ships in the binary. |
| Write path | **Read-only.** VIEW key only. The client never writes to the corpus. |
| Multiplayer sync | **Dropped.** No realtime service exists, and it was v1's buggiest area. |
| Feature scope | Image carousel, server geolocation, debug console, frameless overlay. |

---

## Architecture

A two-crate workspace. The split is the point: **`scanner-core` has no GUI
dependency**, so the parsing, tailing and API logic is testable headlessly. v1 had
no test surface at all because everything reached into `MainWindow`.

```
crates/core   scanner-core   no GUI deps — runs under `cargo test` with no display
  config      atomic JSON persistence, per-OS paths, holds the user's API key
  logsrc/     finder | tailer | parser
  api/        db-api client: lookup → detail, ETag caching, backoff
  geo         ipinfo.io lookup from the udmux log line
  model       Room, RoomImage, RoomAttributes, LookupResponse
  event       the Event enum crossing the thread boundary
  engine      orchestrator; owns the tokio tasks

crates/app    scanner-app    eframe binary — thin views, no logic
```

### The one rule that shapes everything

Workers never touch the UI. Every background task owns an
`mpsc::UnboundedSender<Event>`; the egui update loop drains the receiver once per
frame and folds events into a single `AppState`. There is no shared mutable state
between threads and no widget handle outside the UI thread.

This is the direct answer to v1's largest defect class — state scattered across
`MainWindow` attributes, initialised in three places, mutated from worker threads
(`widgets.py:716` called `setText()` off the GUI thread). In immediate mode the UI
is a pure function of `AppState`, so "widget disagrees with model" is not a state
the program can reach.

### Correctness rules from the backend

These are contract details that will silently corrupt behaviour if missed:

- **Never derive a slug.** Always `GET /api/rooms/lookup?q=<name>` → use
  `exact.slug`. Only `match ∈ {slug, room_name, squashed}` is authoritative;
  `prefix`/`substring` are candidates, `none` means not in the database.
- **Snowflakes are JSON strings**, not numbers. `documented_by`, `last_edited_by`,
  `uploaded_by` are all `Option<String>`. Never `u64`.
- **Images arrive as absolute URLs** on `cdn.nightfalldivision.com` — do not build
  them. Order by `position`; `is_primary` is the hero.
- **`Authorization: Bearer <key>` on every request.** There is no anonymous access.

### Where a run begins

A run starts at the room named **`Start`**. The lobby, the teleport, and the
`Client:Disconnect` pair that marks that teleport are all noise — the scanner
ignores everything before the most recent `Room Name: Start`.

This has one non-obvious consequence, which `logsrc::resume` exists to handle:
the game server's `UDMUX` address is logged roughly ten seconds *before* the
first room, so seeking straight to `Start` would throw away the address
geolocation needs. Resuming is therefore two results, not one — scan the whole
file to establish state (server address, and the current room if we are joining
mid-run), then tail from the run-start offset.

If no `Start` is present the player is already mid-run, so we resume at end of
file and report only new activity, carrying the current room forward so the UI
is not blank. Reading from byte zero in that situation is what made v1 duplicate
every room.

---

## What changes relative to v1

| v1 defect | v2 approach |
|---|---|
| Blocking image downloads on the UI thread (`windowed.py:447`) | Downloads are tokio tasks; bytes arrive as `Event::ImageReady`. A frame never blocks. |
| Non-atomic config writes wipe settings on interrupt (`appdata.py:69`) | Write to a tempfile, `fsync`, atomic rename. Saves debounced, not per slider tick. |
| Five daemon threads + three asyncio loops | One tokio runtime; tasks, not threads. |
| GUI signals passed via module globals (`websocket.py:82`) | Typed `Event` enum over a channel. |
| Fixed 0.5 s log poll | `notify` filesystem events, with a slow poll as a safety net. |
| Room name recovered by re-parsing rendered markup (`widgets.py:704`) | Name lives in the model; the view only renders it. |
| Windows-only fonts substituted elsewhere | Fonts embedded in the binary. |
| Hand-computed geometry mixed with layouts | egui layout throughout. |

---

## Portability

Windows is the primary target, Linux second, macOS for parity. Verified status:

| Target | How it is verified |
|---|---|
| Linux | Native build + full test suite |
| Windows | `cargo check --target x86_64-pc-windows-gnu` locally, plus native CI |
| macOS | CI only — cross-compiling needs Apple's SDK, which cannot be installed elsewhere |

**Cross-compiling cannot verify this project**, because rustls needs a C toolchain
built for the target. That is why the CI matrix (`.github/workflows/ci.yml`) builds
and tests natively on all three runners, and why it was set up early rather than
left to the release milestone — it is the only real answer to "does this work on
Windows?"

Two choices follow from that:

- **rustls uses the `ring` provider, not the default `aws-lc-rs`.** Both need a C
  compiler; neither cross-compiles for free. But `aws-lc-sys` additionally wants
  NASM on Windows and compiles a large amount of C, whereas `ring` built for
  `windows-gnu` here with nothing but mingw. This is a build-friction preference,
  not a correctness one — reqwest's `rustls` feature would also work.
- **No `notify` dependency.** The engine polls every 500 ms rather than watching
  the filesystem. Half a second of latency on a room display is imperceptible, and
  a poll cannot miss an event the way a watcher can on network or overlay
  filesystems. Worth revisiting only if profiling says so.

Everything OS-specific is confined to `logsrc/finder` (log directory discovery)
and `config` (per-OS config paths via `directories`, atomic replace via
`tempfile::persist`). No `#[cfg]` blocks appear anywhere else.

## Milestones

Each is independently verifiable; nothing is merged unverified.

**M1 — core skeleton. ✅ done.** Workspace, `config` with atomic writes,
`logsrc/parser` and `logsrc/resume` with fixtures from a real captured session.

**M2 — API client. ✅ done.** `lookup` → `{slug}` flow, typed models, client-side
token bucket, `retry_after` honoured on 429, backoff on 503/5xx, TTL+LRU cache.
Live tests in `tests/live_api.rs` (ignored by default) verify against db-api.

> **Caching note.** Conditional GETs do *not* help here. `304` is implemented only
> on `/api/rooms/export` (`_get_routes.py:414`), which needs `BULK_OPERATIONS`; a
> VIEW key cannot use it. `GET /api/rooms/{slug}` returns a full body every time —
> verified live, `If-None-Match: "1"` still answered `200`. The `ETag` /
> `X-Room-Version` on detail exist for `If-Match` on writes, which a read-only
> client never performs. So caching is an in-memory LRU keyed by slug with a TTL,
> not conditional requests. Room documentation changes rarely, so a generous TTL
> plus the recently-seen dedupe keeps request volume very low.

**M3 — engine. ✅ done.** `logsrc/finder` (per-OS discovery incl. Wine/Proton/Sober
prefix globbing), `logsrc/tailer` (append, truncation, rotation, partial-line
withholding), `geo` (ipinfo.io with a private-address guard), `event`, and
`engine` wiring tail → parse → resolve → `Event`. Verified by
`cargo run -p scanner-core --example replay -- <log>`, which runs the whole
pipeline headlessly.

**M4 — UI shell. ✅ done.** eframe 0.36, frameless always-on-top viewport, custom
title bar with painted icons, opacity/scale, room detail, run history, console,
status bar, first-run settings. Verified by running against the captured log and
the live API under a virtual display.

Two things worth remembering from building it:

- **Icons are painted, not typed.** egui bundles its own fonts and symbols
  outside the common blocks are not in them — U+25CF (status dot) and U+2715
  (close cross) both rendered as tofu boxes. egui paints its own window close
  button with line segments for exactly this reason, so we do the same. The
  upside of the bundled fonts is that text is identical on all three platforms
  with no work, where v1 hardcoded Windows-only font names in ~20 places.
- **Translucency needs a compositor.** The overlay paints its own alpha rather
  than asking the OS for window opacity, which is the portable choice — but on
  X11 with no compositing WM the alpha renders as solid black, and the settings
  panel is invisible too, so the user cannot recover. `SCANNER_OPAQUE=1` forces
  an opaque window, and `effective_opacity()` ignores the opacity setting when
  the window has no alpha channel.
- **`Context::set_visuals` writes to whichever theme is active when it is
  called**, not to a fixed palette. Calling it at startup — before Windows has
  reported its system theme — landed the palette on the wrong `Theme` variant
  once the OS's actual preference came through, so the app rendered in egui's
  unconfigured stock theme on Windows despite looking correct in every local
  (Linux, no reported system theme) test. Fixed by pinning
  `ThemePreference::Dark` explicitly and writing the palette into both `Theme`
  variants, so nothing can select an unconfigured one.
- **Windows transparent windows carry a hidden opaque layer underneath wgpu's
  swapchain.** winit implements `with_transparent` on Windows via
  `DwmEnableBlurBehindWindow` rather than `WS_EX_NOREDIRECTIONBITMAP`, which
  leaves the window's GDI redirection surface — a system-managed backing bitmap
  — in DWM's composition stack *underneath* the swapchain. Windows fills that
  surface opaque white on first show, and since the app draws exclusively
  through the swapchain, nothing ever repaints it: the translucent content
  composites correctly over solid white instead of the desktop. A live resize
  makes this look like a completely different bug — DWM carries the surface's
  stale content forward through the resize, truncating it on shrink and
  zero-filling new area on grow (zero = transparent black), so post-resize the
  window is correctly transparent *except* a solid rectangle exactly the size
  of the window at its smallest, which looked at first like a swapchain/surface
  bug rather than a leftover fill underneath it.

  This was invisible from Linux entirely — no virtual display here has DWM, so
  every earlier attempt (repainting per-panel alpha differently, switching to
  glow, chasing wgpu's `CompositeAlphaMode` selection) was reasoning from
  screenshots without being able to reproduce the failure. `crates/app/src/bin/repro.rs`
  is a bare eframe window with none of the app's code, used to confirm the bug
  was upstream rather than in the scanner before spending more effort on it —
  it also has a `--raw` flag to show the artifact undisguised. The eventual fix,
  in `redirection_surface.rs`: GDI draws land in that same redirection surface,
  and `PatBlt` with the `BLACKNESS` raster op writes all four bytes to zero —
  premultiplied transparent black — over it. Doing that at startup and after
  resize/scale/restore events (a short burst of frames, since the system's
  white fill can land asynchronously after the triggering event) keeps the
  surface transparent for good, since DWM's later copies of it stay zero. The
  window is also created hidden and shown only once the renderer is
  initialized, so the white startup fill is never on screen at all.

  `WS_EX_NOREDIRECTIONBITMAP` would remove the redirection surface outright and
  is the root-cause fix, but it can only be set at window-creation time and
  egui-winit 0.36 has no way to pass extended window styles through.

**M5 — images. ✅ done.** `core::images` (pure-Rust decode via `image` with only
`png`/`jpeg`/`webp` enabled — no libwebp, keeps the small-binary/easy-cross-compile
story) downloads and decodes off the GUI thread; `app::textures::TextureCache` is
a bounded LRU of `egui::TextureHandle` (v1's equivalent cache was unbounded —
`sync_window.py:609`). Carousel has prev/next buttons, a position counter,
caption, `,`/`.` and arrow-key navigation (suppressed while a text field has
focus), and auto-rotate on a per-room timer. Verified live against the real log
and API under Xvfb, including a real decoded image rendering in the carousel.

Two things worth remembering from building it:

- **Texture creation must happen on the thread that owns the `egui::Context`.**
  `DecodedImage` (raw RGBA bytes) is a `core` type with no GUI dependency, sent
  over the existing `Event` channel like everything else; the app intercepts
  `Event::ImageReady`/`ImageFailed` in `drain_events` *before* they reach
  `AppState::apply`, since only the app crate is allowed to touch
  `egui::Context::load_texture`. `AppState` keeps an exhaustive match arm for
  both variants that does nothing, so a future event can't be silently missed.
- **Don't request images for a room the player has already left.** The first
  version of this requested every resolved room's images unconditionally, so a
  session that resolves many rooms in a burst (replaying a full 41-room log
  reads and processes the entire file about as fast as the API rate limiter
  allows, not at the original session's real pacing) left the *current* room's
  own images queued behind ~150+ images for rooms already superseded —
  confirmed live, a spinner still showing after 90+ seconds. The direct fix is
  [`fetch_images_for`](../crates/core/src/engine.rs): for a batch of parsed log
  events, only the *last* `RoomEntered` is worth fetching images for, since
  anything earlier in the same batch is already superseded by the time that
  batch finishes processing — the room never reached the screen, so its
  pictures were never going to be seen. A pure function, tested without a
  `Scanner` or the network. In ordinary live play this changes nothing (a
  poll every 500 ms against someone walking normally almost always finds at
  most one new room), so it only ever activates on a backlog. Re-ran the exact
  41-room burst that previously left a 90-second spinner: the current room's
  image is now fully loaded and rendering by 55 seconds — because the ~40
  superseded rooms' images are simply never requested, not merely
  deprioritized. A second, small dedicated semaphore for exactly one image per
  room (`MAX_CONCURRENT_PRIORITY_DOWNLOADS`) stays on as defence-in-depth for
  what the batch fix alone doesn't cover — consecutive batches can each still
  contribute one "current" room, so more than one room's images can
  legitimately be in flight when catching up a backlog spanning more than one
  poll.

**Known issue, not fixed here — out of scope for images specifically:** a room's
`contributor.display_name` is a free-form Discord display name and can contain
any Unicode script (confirmed live: a real contributor's name is `仁`). egui's
bundled fonts only cover a Latin/Western subset, so any such name renders as a
tofu box next to "documented by" — the same class of problem as the M4 title-bar
icons, but unlike a fixed icon, arbitrary user text can't be hand-painted; the
real fix is bundling a broader fallback font (e.g. Noto Sans CJK), which is a
binary-size trade-off (tens of MB) deserving its own decision rather than a
silent fix mid-carousel.

**M6 — polish.** Debug console as a second viewport, geolocation, first-run API key
entry, settings.

**M7 — release.** Cross-platform CI producing three binaries, mirroring v1's
`deploy.yml` shape but for cargo.

---

## Verification

- `cargo test -p scanner-core` — runs with no display. Parser fixtures assert room
  extraction, the udmux IP, and the double-disconnect debounce.
- A headless `replay` example in core: feed it a log file, print the resulting
  `Event` stream. Exercises the whole pipeline minus rendering.
- Live smoke test against db-api with a VIEW key: lookup a known room, fetch
  detail, confirm a 304 on the second request.
- Manual: run beside the game on Windows, confirm rooms appear and the overlay
  does not steal focus.

---

## What I still need from you

1. **A VIEW key** for local development against db-api.
2. **Sample Roblox log files** — ideally a full session including a disconnect.
   v1's parser keys on the substrings `"room name"`, `"udmux"` and
   `"[flog::network] client:disconnect"`. I want to build fixtures from real logs
   rather than trusting v1's assumptions, since that parsing is the one part with
   no backend to validate it against.
3. Confirmation that **`RoomNames.txt`** in database-service is the canonical name
   list — useful for testing lookup coverage.
