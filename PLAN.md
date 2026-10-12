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
- [x] P0-4 `snapwright-app`: gpui-kit shell and resident process, with a hidden window at
      launch that sets up the GPU before the first hotkey. Tray icon moved to P0-10.
- [x] P0-5 Global hotkey (`global-hotkey`): Ctrl+Shift+2, Cmd+Shift+2 on macOS.
- [x] P0-6 Overlay: one borderless pop-up window per display showing the frozen frame
      (BGRA straight into `RenderImage`, no copy); drag to select with a size label, Enter
      or release copies, a click takes the whole display, Esc cancels.
      Note: under Xvfb with a software GPU (llvmpipe), capture takes 7 to 16 ms and the
      overlay is visible 66 to 112 ms after the hotkey. Arrow-key nudge and loupe moved to P0-11.
- [x] P0-7 Copy to clipboard (PNG; `arboard` on Linux, GPUI elsewhere) and save to
      `Pictures/Snapwright`.
- [x] P0-8 Timing logged: hotkey to overlay visible, hotkey to clipboard.
      `scripts/overlay-e2e.sh` checks the whole flow under Xvfb in CI.
- [ ] P0-9 Measure on real macOS, Windows and X11 machines; record numbers here.
      First data from hosted CI runners (`snapwright bench`, VMs without a GPU):
      macOS (ScreenCaptureKit, 1024x768): first 273 ms, p50 15.5 ms, p95 26.9 ms.
      Windows (DXGI, 1024x768): first 10.7 ms, p50 0.57 ms (unchanged screen reuses the
      last frame). X11 (Xvfb, 2560x1440): first 26 ms, p50 2.5 ms, p95 2.7 ms.
- [x] P0-10 Tray icon (`tray-icon`) with Capture, Open captures folder, Quit.
      Note: Linux uses StatusNotifierItem (`ksni`), no GTK.
- [x] P0-11 Overlay polish: arrow keys nudge and Shift+arrows resize the selection; loupe
      with pixel colour; window snapping preview.
      Note: hover highlights the window under the pointer, a click takes it; Shift on release
      keeps a drag open for adjusting; `C` copies the colour under the pointer as hex.
      `Capturer::windows()` and `snapwright windows` list windows on all three OSes.
- [ ] P0-12 Pre-create one hidden overlay window per display and reuse it, if real-hardware
      numbers show window creation dominates. Waiting on P0-9: GPUI has no per-window
      hide/show, so this needs a gpui-fast patch (see DECISIONS).

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

- macOS: `SCScreenshotManager` costs ~15 ms a shot and ~270 ms cold. Try keeping an
  `SCStream` paused between captures, or starting one when the hotkey's modifiers go down.

- Windows: honour `DXGI_OUTPUT_DESC::Rotation` for portrait monitors.
- Windows: HDR desktops through `DuplicateOutput1` with an FP16 format, tone mapped to sRGB.
- macOS: display names from `NSScreen.localizedName`.
- macOS: confirm `SCShareableContent.windows` comes front to back on a real machine; if not,
  order by `CGWindowListCopyWindowInfo`.
- gpui-fast: upstream a fix for the 2 px X11 window shift, then drop `X11_SHIFT`.
