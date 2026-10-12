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
