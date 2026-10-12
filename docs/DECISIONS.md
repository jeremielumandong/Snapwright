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
