# Scanner

A desktop overlay that follows the Roblox client log and shows what researchers
have documented about the room you are standing in.

This is a ground-up Rust rewrite of
[franktorio-pressure-scanner](https://github.com/Franktorio/franktorio-pressure-scanner),
which is unmaintained and talks to an API that no longer exists. See
[docs/PLAN.md](docs/PLAN.md) for the architecture and what changed.

Windows, macOS and Linux, single binary, no runtime dependencies.

## Getting an API key

The research database has no anonymous access, so the scanner needs a key.
Paste it into **Settings** on first run; it is stored only on your machine, in
your platform's config directory.

## Building

```sh
cargo run -p scanner-app
```

Linux additionally needs the windowing and GPU headers:

```sh
sudo apt-get install libgtk-3-dev libxkbcommon-dev libxkbcommon-x11-0 \
                     libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

## Layout

| Crate | What it is |
|---|---|
| `crates/core` | Log parsing, tailing, the database client, the scanner engine. No GUI dependency, so it runs under `cargo test` with no display. |
| `crates/app` | The egui front end. Views only; no logic. |

## Testing

```sh
cargo test --workspace                 # offline, hermetic, no display needed
```

Tests that hit the live API are `#[ignore]`d. To run them:

```sh
NFD_API_KEY=<key> cargo test -p scanner-core --test live_api -- --ignored
```

To replay a log through the whole pipeline without a GUI — useful for
diagnosing a user's log:

```sh
NFD_API_KEY=<key> cargo run -p scanner-core --example replay -- path/to/player.log
```

## Troubleshooting

**The window is solid black on Linux.** The overlay is translucent, which needs
a compositing window manager. Without one, X11 renders the alpha as black. Run
with `SCANNER_OPAQUE=1` for an ordinary opaque window.

**No log file found.** The scanner looks in the usual place for your platform,
including Wine, Proton and Sober prefixes on Linux. If yours is elsewhere, set
the path in Settings — it applies immediately, no restart.
