# Plan

Product plan: [Snapwright: Product & Technical Plan](https://claude.ai/code/artifact/4bcb7cd7-57cc-4ff4-87cc-561af7643f9c).
Work through the current phase in order. Tick a task when it lands and add a one-line note.

## Phase 0: Spikes (weeks 1 to 4)

Gate: hotkey to overlay under 50 ms (p95) on macOS, Windows and X11.

- [x] P0-1 Workspace, toolchain pin, lints, CI on three OSes.
- [x] P0-2 `snapwright-capture`: `Capturer`, `Display`, `Frame` (BGRA), backends for
      macOS (ScreenCaptureKit), Windows (DXGI Desktop Duplication), X11 (MIT-SHM + GetImage).
      Note: X11 grabs 2560x1440 at 3.9 ms p50 under Xvfb; macOS and Windows need a real
      machine to measure (hosted runners lack permission or a GPU).
- [x] P0-3 `snapwright` CLI: `displays`, `capture`, `bench`.
- [ ] P0-4 `snapwright-app`: gpui-kit shell, resident process, tray icon (`tray-icon`).
- [ ] P0-5 Global hotkey (`global-hotkey`) that captures every display.
- [ ] P0-6 Overlay: one borderless top-most window per display showing the frozen frame;
      drag to select, arrow keys nudge, Enter copies, Esc cancels. Loupe with pixel colour.
- [ ] P0-7 Copy to clipboard (PNG) and save to the captures folder.
- [ ] P0-8 Timing: hotkey to overlay visible, selection to clipboard; logged and in `bench`.
- [ ] P0-9 Measure on real macOS, Windows and X11 machines; record numbers here.

## Phase 1: Alpha (weeks 5 to 14)

Gate: the team uses it daily with no capture lost in two weeks.

- [ ] Window capture with snapping; capture delay timer; repeat last region.
- [ ] Quick-access thumbnail after capture.
- [ ] Editor on Emulsion's engine: arrow, rectangle, ellipse, line, text, highlight, blur and
      pixelate, step counter, crop; undo and redo; non-destructive scene saved beside the image.
- [ ] `snapwright-library`: SQLite + FTS5, content-addressed store, metadata (app, window
      title, URL, display, time), thumbnails, grid with virtual scrolling, search, tags.
- [ ] `snapwright-bench` hotkey-to-clipboard harness in CI; signed builds.

## Phase 2: Public beta (weeks 15 to 26)

Gate: 1,000 beta users and 99.5% crash-free sessions.

- [ ] Wayland: `ext-image-copy-capture-v1` / `wlr-screencopy`, portal ScreenCast fallback.
- [ ] OCR index (Vision, Windows.Media.Ocr, `ocrs`); collections; pin to screen.
- [ ] Scrolling capture; smart redaction; beautifier; export formats; HDR tone mapping.

## Phase 3: v1.0 (weeks 27 to 36)

- [ ] Screen recording to MP4 and GIF; S3 uploads; after-capture workflows.
- [ ] CLI library queries; licensing and payments; docs site.

## Open decisions

Licence, pricing, cloud sharing and team size: see the plan's decisions table.

## Follow-ups

- Windows: honour `DXGI_OUTPUT_DESC::Rotation` for portrait monitors.
- Windows: HDR desktops through `DuplicateOutput1` with an FP16 format, tone mapped to sRGB.
- macOS: display names from `NSScreen.localizedName`.
