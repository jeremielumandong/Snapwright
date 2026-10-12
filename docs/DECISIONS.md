# Decisions

Append an entry when a call is non-obvious or the user made it. Newest last.

## 2026-10-12: Platforms, macOS and Windows first

Jeremie chose macOS and Windows for the first release, with Linux one phase behind. X11 is
in from the start because it is cheap and testable in CI; Wayland lands in Phase 2.

## 2026-10-12: UI stack, the same as Switchyard

gpui-kit 0.7.1 (gpui-pre from Longbridge's gpui-fast fork + gpui-component 0.7.1), as in
`jeremielumandong/switchyard`. Its theme, zoom, palette, overlays, updater and packaging are
ported from there. Image and editor code comes from `jeremielumandong/emulsion` (MIT).

## 2026-10-12: Capture backends are our own, not GPUI's

GPUI's screen capture is a streaming API built for screen sharing; starting a stream costs
more than one screenshot. `snapwright-capture` calls ScreenCaptureKit's
`SCScreenshotManager`, DXGI Desktop Duplication and X11 MIT-SHM directly. Recording may still
use GPUI's stream later.

## 2026-10-12: Frames are BGRA

macOS and Windows capture in BGRA and GPUI's `RenderImage` takes BGRA, so frames go to the
overlay without conversion. Encoders that need RGBA convert at export time.

## 2026-10-12: `unsafe` only in platform modules

The workspace denies `unsafe_code`; `capture`'s `linux`, `macos` and `windows` modules opt
out, and every `unsafe` block carries a `SAFETY:` comment (clippy `undocumented_unsafe_blocks`).

## 2026-10-12: GPU warm-up window at launch

The first GPUI window creates the GPU device and pipelines: 2.8 s with a software renderer
under Xvfb. The app opens a hidden 1x1 window at launch so the first overlay does not pay
for it. Later overlays then show in 66 to 112 ms on the same software renderer.

## 2026-10-12: `arboard` for clipboard images on Linux

GPUI's X11 clipboard writes text only. On Linux the app puts captures on the clipboard with
`arboard` (MIT/Apache-2.0), kept alive for the app's life because on X11 the owner serves
every paste. macOS and Windows use GPUI's clipboard, which supports PNG.

## 2026-10-12: Tray icon through `tray-icon` with the `ksni` backend

`tray-icon` (MIT/Apache-2.0) gives the native status item on macOS and Windows. On Linux its
default backend needs GTK and libappindicator; the `ksni` backend speaks
StatusNotifierItem over D-Bus instead, so the app links no GTK. Without a D-Bus session (as
under Xvfb) the tray is skipped with a warning and the hotkey still works.

## 2026-10-12: Selection: click snaps to windows, Shift on release keeps it open

Hovering highlights the frontmost window under the pointer and a click takes it; on bare
desktop a click takes the display. A drag still copies on release, the fastest path. Holding
Shift on release keeps the region open: drag inside to move it, arrows nudge, Shift+arrows
resize (Ctrl/Cmd for 10 px), Enter copies. Windows are listed after the frames are grabbed
and before any overlay opens, so ours are never in the list; the app also drops its own pid.

## 2026-10-12: Overlay windows ask for 2 px less on X11

gpui-fast's X11 backend creates every window 2 px right of the requested origin (a Zed
workaround for GNOME). Override-redirect pop-ups keep the shift, so the frozen frame sat 2 px
off the real screen. `session::open_overlays` asks for `x - 2`; a gpui-fast fix would let us
drop that.

## 2026-10-12: P0-12 (reused overlay windows) waits for real-hardware numbers

GPUI has no per-window hide or show, so reusing overlay windows means patching gpui-fast.
Under Xvfb the overlay appears 66 to 95 ms after the hotkey, but that is a software renderer.
Do it only if P0-9's numbers on real machines show window creation is the bottleneck.

## 2026-10-12: Library store and the captures folder

The library keeps each image once, named by the BLAKE3 hash of its PNG, under the platform
data folder, with metadata, tags and an FTS5 index in SQLite (`rusqlite` with bundled
SQLite, MIT). Every capture is also still written to `Pictures/Snapwright` so people find a
plain file where they expect one; that copy becomes a setting once settings exist. Search
matches word prefixes across app, title, URL, note, tags and recognised text; FTS5 syntax in
what people type is treated as plain words.

## 2026-10-12: Repeat-last-region hotkey and delayed capture

Repeat last region is Ctrl+Alt+Shift+2 (Cmd+Option+Shift+2 on macOS), next to the capture
hotkey and clear of the system's Cmd+Shift+3/4/5. It grabs the same display and crops the same
frame pixels with no overlay. Delayed capture has no hotkey yet: tray "Capture in 3 seconds"
or `--capture-in=N`. Hotkeys another app owns are skipped with a warning instead of failing
the rest.

## 2026-10-12: Editor engine is a small crate on Emulsion's rasteriser, not emulsion-core

Emulsion's `emulsion-core` carries documents, storyboards, diagrams and cosmic-text; pulling it
in would multiply build time and binary size for what a screenshot editor needs. Snapwright's
`snapwright-editor` instead keeps a small serde scene (arrow, line, rectangle, ellipse,
highlight, text, step, blur, pixelate, crop) and renders it with tiny-skia 0.11, the rasteriser
Emulsion uses, with Emulsion's Gaussian kernel ported (MIT). Text is laid out with ab_glyph and
the bundled Geist font (OFL, copied from Emulsion's assets) as vector paths, so exports match on
every OS. The preview is rendered by the same code as the export, on a background thread, so
what the user sees is what gets copied.

Redactions (blur, pixelate) always apply to the image's own pixels under every other shape, so
nothing drawn later can be undone to reveal them, and exports contain no original pixels there.

## 2026-10-12: Edits live in the library row (schema v2)

"Saved beside the image" means: the scene JSON and the rendered image's hash are columns on the
capture. The rendered image is a second content-addressed file with its own thumbnail and is
what the library shows, copies and opens; the original file is never touched, and "Revert to
original" drops the edit. Closing the editor saves; Done also copies. Old edit files are deleted
once no capture uses them.

## 2026-10-12: Our own windows stay out of captures where the OS allows

The quick-access preview sits on screen for six seconds, often while the next capture starts.
Closing it and waiting for the compositor cost 120 ms per capture, which the latency harness
caught (repeat last region went from 5 ms to 128 ms). Now:

- macOS: the ScreenCaptureKit filter excludes this application, so none of Snapwright's own
  windows (preview, library, editor, overlay) are ever in a capture, and nothing waits.
- Windows: the preview window is marked `WDA_EXCLUDEFROMCAPTURE` (Windows 10 2004+); when that
  fails it falls back to closing and waiting.
- Linux/X11 has no such switch. Repeat last region keeps the preview up unless it covers the
  region; a new overlay still closes it and waits.

Capturing Snapwright's own library or editor window on macOS is therefore not possible; if that
turns out to matter, except those windows in the filter.

## 2026-10-12: Release builds copied from Switchyard

Jeremie asked for Switchyard's release build as is, so the workflows, packaging scripts, upload
and bump scripts and the Windows icon resource (`crates/app/build.rs`) are Switchyard's with the
names changed. Differences:

- The app binary stays `snapwright-app` and the CLI `snapwright`; the AppImage runs the CLI when
  invoked through a symlink named `snapwright`.
- macOS needs 14.0 (ScreenCaptureKit's `SCScreenshotManager`), not 11.0, and the hardened
  runtime takes no exceptions (Switchyard's library-validation exception was for its database
  drivers). Info.plist carries `NSScreenCaptureUsageDescription`.
- Switchyard's Entra client ID and updater key variables are dropped; Snapwright has neither.
- The Windows publisher check reads only `SNAPWRIGHT_SIGN_EXPECTED_SUBJECT`.

Secrets and variables are listed in `docs/RELEASING.md`.

## 2026-10-12: Wayland through wlr-screencopy, layer shell and a keybinding

Pulled forward from Phase 2 because Jeremie runs Hyprland and the app refused to start there.

- Capture uses `wlr-screencopy` into a reused `wl_shm` memfd buffer (wlroots compositors:
  Hyprland, Sway, river, Wayfire). It came first because it is the protocol every wlroots
  compositor has; `ext-image-copy-capture` and the portal (GNOME, KDE) are still to do, and
  those sessions get `CaptureError::Unsupported`.
- `Display::bounds` on Wayland is the output's logical (`xdg-output`) position times its scale,
  with the copied buffer's size, so "physical pixels" holds and the app's `bounds / scale`
  gives the logical rectangle as elsewhere. Rotated and flipped outputs are refused until
  someone can test them.
- Copies wait at most 2 s: a compositor copies on its next repaint, and a display in DPMS
  sleep never repaints.
- The window list comes from Hyprland's IPC socket; other compositors get no snapping.
- Overlay, preview and countdown are `zwlr_layer_shell_v1` surfaces on the overlay layer (a
  normal Wayland window would be tiled). The overlay takes the keyboard exclusively, and its
  GPUI display is found by the UUID GPUI derives from the connector name.
- Wayland has no global key grabs, so the app is single-instance on Linux: a second launch
  sends its flags (`--capture-now`, `--repeat-last`, `--library`, …) to the running app over
  `$XDG_RUNTIME_DIR/snapwright-<display>.sock` and exits. Users bind
  `snapwright-app --capture-now` in their compositor. A launch with no flags opens the library.
- arboard's `wayland-data-control` feature puts images on the Wayland clipboard.

## 2026-10-12: UI refresh from the design handoff

The design (`docs/design/Snapwright Redesign.dc.html` with its Library and Editor pages) is
the reference; `crates/app/src/theme.rs` holds its tokens.

- Windows follow the system light/dark setting (`theme::sync` on appearance changes); the
  capture HUDs are always dark because they sit over arbitrary screen content.
- The palette is pushed into gpui-component's `Theme`, including the `button_*` fields its
  buttons read (they do not use `primary`), so library and editor controls match the custom
  elements around them.
- Geist is bundled as static Regular, Medium and SemiBold (from Switchyard); GPUI does not pick
  weights out of the editor's variable `Geist.ttf`.
- The library grid became a variable-height `list` so each day gets a header row; arrow keys
  move by grid row across those headers (`step_vertical`).
- The note saves when the field loses focus instead of on Enter, since it is now multi-line.
- Not built from the design: drag handles on the overlay selection (no handle dragging yet)
  and the image drop shadow inside thumbnails (`object_fit` boxes have no fitted bounds to
  shadow).
