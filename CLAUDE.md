# CLAUDE.md — Snapwright

Snapwright is a fast, cross-platform screenshot tool with a searchable capture library,
written in Rust with GPUI (through gpui-kit).

- Product plan: [Snapwright: Product & Technical Plan](https://claude.ai/code/artifact/4bcb7cd7-57cc-4ff4-87cc-561af7643f9c)
- Task plan: `PLAN.md` (work through the current phase in order)
- Decisions log: `docs/DECISIONS.md` (append when you make or need a non-obvious call)
- Sister projects to borrow from: `jeremielumandong/switchyard` (GPUI app shell, theme,
  palette, updater, packaging) and `jeremielumandong/emulsion` (MIT: canvas engine, filters,
  export, thumbnails, local AI models). Check licences before copying anything else.

## How to work in this repo

1. Open `PLAN.md` and take the first unchecked task in the current phase.
2. State a short plan: files you will touch, types you will add, how you will test.
3. Implement only that task. Note drive-by ideas under "Follow-ups" in `PLAN.md`.
4. Run the checks below. All must pass.
5. Tick the task in `PLAN.md` with a one-line note.

## Commands

```bash
cargo build --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
xvfb-run -a -s "-screen 0 1920x1080x24" cargo test -p snapwright-capture --test x11 -- --ignored
cargo run --release -p snapwright-cli -- bench --runs 100
cargo run --release -p snapwright-app -- --capture-now   # SNAPWRIGHT_LOG=debug for timings
xvfb-run -a -s "-screen 0 1600x1000x24" scripts/overlay-e2e.sh target/release
xvfb-run -a -s "-screen 0 1920x1080x24" scripts/latency-bench.sh target/release   # p50/p95
```

Linux builds of the app need `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev
libvulkan-dev libfontconfig-dev libfreetype-dev`, and `mesa-vulkan-drivers` to open windows
under Xvfb.

Type-check other platforms from Linux (no linking):

```bash
cargo clippy -p snapwright-capture --target aarch64-apple-darwin -- -D warnings
CC_x86_64_pc_windows_msvc=clang cargo clippy -p snapwright-capture --target x86_64-pc-windows-msvc -- -D warnings
```

## Workspace layout

```
crates/
  app/      snapwright-app      GPUI binary: global hotkey, tray, per-display overlay, clipboard, save
  capture/  snapwright-capture  Capturer, Display, Frame; one backend module per OS
  cli/      snapwright-cli      `snapwright` binary: displays, windows, capture, bench
  editor/   snapwright-editor   annotation scene (shapes, crop), tiny-skia renderer, undo history
  library/  snapwright-library  capture library: content-addressed store, SQLite + FTS5, tags, trash
scripts/overlay-e2e.sh          hotkey to clipboard under Xvfb (needs xdotool, xclip, ImageMagick)
```

## Rules

- **Speed is the product.** Every change on the capture path is measured with
  `snapwright bench` before and after. Targets: overlay visible < 50 ms after the hotkey,
  selection on the clipboard < 100 ms, idle CPU 0%.
- **Frames are BGRA, tightly packed, alpha opaque.** Convert only at export.
- **Keep OS state warm.** Backends hold devices, duplications, SHM segments and filters
  between captures; never set them up per capture.
- **`unsafe` only in platform modules**, each block with a `SAFETY:` comment.
- **The UI thread never does I/O or encoding.** Encoding, thumbnails and OCR run off it.
- No `unwrap()` / `expect()` outside tests. Errors are `thiserror` enums in library crates,
  `anyhow` in binaries. Logging with `tracing`.

## Gotchas

- macOS: `CGDisplayCreateImage` is obsolete since macOS 15; use ScreenCaptureKit.
  `SCDisplay` sizes are in points; multiply by `SCContentFilter::pointPixelScale`.
- macOS: objc2-foundation needs the `NSEnumerator` feature for `NSArray::iter`.
- Windows: Desktop Duplication needs per-monitor DPI awareness, set in `Backend::new`.
  After the first frame, `AcquireNextFrame` only returns when the screen changed; on a
  timeout reuse the last staging texture.
- X11: Xvfb resets the root window when its last client disconnects; tests keep a
  connection open while they paint. Tests that paint the root window must not run in parallel.
- Wayland: an X11 `GetImage` under XWayland sees only X clients, so Wayland sessions are
  refused until the portal backend exists.
- GPUI: `window.remove_window()` through a window's own handle fails while that window is
  being updated (its event handlers run inside that update). Close the current window
  directly and the others through their handles (`session::finish`).
- GPUI: the first window pays for GPU setup (seconds on a software renderer). The app opens a
  hidden window at launch to take that cost before the first hotkey.
- GPUI's X11 clipboard writes text only; images go through `arboard` on Linux.
- GPUI's X11 backend creates windows 2 px right of the requested origin; override-redirect
  pop-ups go through `session::popup_bounds`, which asks for 2 px less.
- The selection logic lives in `selection.rs` in frame pixels with no GPUI types; test
  interaction changes there first.
- `tray-icon` on Linux needs a D-Bus session; without one the tray is skipped.
- The library lives in the platform data folder (`~/.local/share/snapwright/library` on
  Linux); tests that set `HOME` get their own. Library calls run on background threads
  behind `session::LibraryService`'s mutex, never on the UI thread.
- `cargo run --release -p snapwright-app -- --edit-latest` opens the newest capture in the editor;
  `cargo run -p snapwright-editor --example sample -- out.png` renders every shape kind.
- `cargo run --release -p snapwright-app -- --library` opens the library window at launch;
  `--capture-in=3` starts a countdown capture.
- Anything shown on screen right after a capture (the quick-access preview) must close
  before the next grab: `session::start` and `repeat_last` dismiss it and wait
  `toast::SETTLE`.
- `RenderImage` takes BGRA pixels in an `image::RgbaImage` buffer; the channel order is not
  converted, so capture frames go in as they are. `image` must be the version GPUI links.
- Xvfb has no window manager: keyboard focus follows the pointer, which is fine for the
  full-screen overlay. `tracing` macros shadow a local named `display`.
