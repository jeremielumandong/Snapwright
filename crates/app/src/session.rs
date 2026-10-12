//! One capture session: grab every display, show an overlay per display, deliver the
//! selection, close the overlays.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, point, px, size,
};
#[cfg(not(target_os = "linux"))]
use gpui_kit::{ClipboardItem, Image, ImageFormat};
use snapwright_capture::{Capturer, Display, Frame, Rect, WindowInfo};

use crate::output;
use crate::overlay::Overlay;

/// The capture backend, kept warm for the life of the app. Captures run on a background
/// thread, so it sits behind a mutex.
struct CaptureService(Arc<Mutex<Capturer>>);
impl Global for CaptureService {}

/// The Linux clipboard (GPUI's X11 clipboard writes text only).
#[cfg(target_os = "linux")]
struct Clipboard(Arc<output::LinuxClipboard>);
#[cfg(target_os = "linux")]
impl Global for Clipboard {}

/// Overlay windows of the session in progress, if any.
#[derive(Default)]
struct Active {
    capturing: bool,
    windows: Vec<AnyWindowHandle>,
}
impl Global for Active {}

/// See `open_overlays`.
#[cfg(target_os = "linux")]
const X11_SHIFT: f32 = 2.0;

/// Connects to the platform capture API once, at startup.
pub fn init(cx: &mut App) -> snapwright_capture::Result<()> {
    let capturer = Capturer::new()?;
    cx.set_global(CaptureService(Arc::new(Mutex::new(capturer))));
    cx.set_global(Active::default());
    #[cfg(target_os = "linux")]
    cx.set_global(Clipboard(Arc::new(output::LinuxClipboard::new())));
    warm_up(cx);
    Ok(())
}

/// Opens a hidden 1x1 window at startup so the GPU device, swapchain setup and shaders are
/// ready before the first hotkey press; without it the first overlay pays for all of that.
fn warm_up(cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("snapwright".into()),
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |_, cx| cx.new(|_| gpui_kit::EmptyView)) {
        tracing::warn!(error = %e, "could not pre-create the GPU context");
    }
}

/// Starts a session: captures every display off the UI thread, then shows the overlays.
/// Ignored while a session is already open.
pub fn start(pressed_at: Instant, cx: &mut App) {
    let active = cx.global_mut::<Active>();
    if active.capturing || !active.windows.is_empty() {
        return;
    }
    active.capturing = true;
    let capturer = cx.global::<CaptureService>().0.clone();
    let grab = cx.background_executor().spawn(async move {
        let mut capturer = capturer
            .lock()
            .map_err(|_| anyhow::anyhow!("capture backend poisoned by an earlier panic"))?;
        let displays = capturer.displays()?;
        let shots = displays
            .into_iter()
            .map(|d| capturer.capture_display(&d).map(|f| (d, f)))
            .collect::<Result<Vec<_>, _>>()?;
        // Listed before any overlay opens, so none of ours are in it. Snapping is a nicety:
        // a failure here still shows the overlay.
        let windows = capturer.windows().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "window list unavailable; snapping off");
            Vec::new()
        });
        anyhow::Ok((shots, windows))
    });
    cx.spawn(async move |cx| {
        let shots = grab.await;
        cx.update(|cx| {
            cx.global_mut::<Active>().capturing = false;
            match shots {
                Ok((shots, windows)) => {
                    tracing::debug!(
                        ms = pressed_at.elapsed().as_secs_f64() * 1000.0,
                        displays = shots.len(),
                        windows = windows.len(),
                        "captured"
                    );
                    open_overlays(shots, &windows, pressed_at, cx);
                }
                Err(e) => tracing::error!(error = %e, "capture failed"),
            }
        });
    })
    .detach();
}

/// Snap targets for one display: other processes' windows, in that frame's pixels.
fn snap_targets(display: &Display, frame: &Frame, windows: &[WindowInfo]) -> Vec<Rect> {
    let own = std::process::id();
    let b = display.bounds;
    // Frames can differ from the display's reported size (scaling); map proportionally.
    let sx = frame.width as f32 / b.width.max(1) as f32;
    let sy = frame.height as f32 / b.height.max(1) as f32;
    windows
        .iter()
        .filter(|w| w.pid != own)
        .filter_map(|w| w.bounds.intersect(&b))
        .map(|r| {
            Rect::new(
                ((r.x - b.x) as f32 * sx).round() as i32,
                ((r.y - b.y) as f32 * sy).round() as i32,
                (r.width as f32 * sx).round() as u32,
                (r.height as f32 * sy).round() as u32,
            )
        })
        .collect()
}

fn open_overlays(
    shots: Vec<(Display, Frame)>,
    on_screen: &[WindowInfo],
    pressed_at: Instant,
    cx: &mut App,
) {
    let screens = cx.displays();
    let mut windows = Vec::with_capacity(shots.len());
    for (display, frame) in shots {
        let b = display.bounds;
        let s = display.scale;
        let wanted = Bounds::new(
            point(px(b.x as f32 / s), px(b.y as f32 / s)),
            size(px(b.width as f32 / s), px(b.height as f32 / s)),
        );
        // Prefer GPUI's own idea of the display, so the window lands exactly on it.
        let screen = screens.iter().find(|d| {
            let o = d.bounds().origin;
            (o.x - wanted.origin.x).abs() < px(2.) && (o.y - wanted.origin.y).abs() < px(2.)
        });
        let bounds = screen.map(|d| d.bounds()).unwrap_or(wanted);
        // GPUI's X11 backend creates every window 2 px right of where it was asked (a Zed
        // workaround for GNOME). Override-redirect pop-ups keep that shift, which would offset
        // the frozen frame from the real screen, so ask for 2 px less.
        #[cfg(target_os = "linux")]
        let bounds = Bounds::new(
            point(bounds.origin.x - px(X11_SHIFT), bounds.origin.y),
            bounds.size,
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            display_id: screen.map(|d| d.id()),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Opaque,
            app_id: Some("snapwright".into()),
            ..Default::default()
        };
        let targets = snap_targets(&display, &frame, on_screen);
        match cx.open_window(options, |window, cx| {
            cx.new(|cx| Overlay::new(frame, targets, pressed_at, window, cx))
        }) {
            Ok(handle) => windows.push(handle.into()),
            Err(e) => {
                let display_id = display.id;
                tracing::error!(error = %e, display_id, "overlay window failed");
            }
        }
    }
    cx.global_mut::<Active>().windows = windows;
}

/// Closes every overlay of the session. `current` is the overlay whose event handler is
/// running: it is mid-update, so it can only be removed directly, not through its handle.
pub fn finish(current: &mut Window, cx: &mut App) {
    let windows = std::mem::take(&mut cx.global_mut::<Active>().windows);
    let this = current.window_handle();
    for handle in windows {
        if handle == this {
            current.remove_window();
        } else if let Err(e) = handle.update(cx, |_, window, _| window.remove_window()) {
            tracing::warn!(error = %e, "could not close an overlay");
        }
    }
}

/// Ends the session with a selection: closes the overlays at once, then encodes off the UI
/// thread and puts the PNG on the clipboard and in the captures folder.
pub fn deliver(frame: Frame, pressed_at: Instant, current: &mut Window, cx: &mut App) {
    finish(current, cx);
    #[cfg(target_os = "linux")]
    let clipboard = cx.global::<Clipboard>().0.clone();
    let encode = cx.background_executor().spawn(async move {
        let rgba = frame.to_rgba();
        let png = output::encode_png(frame.width, frame.height, &rgba)?;
        #[cfg(target_os = "linux")]
        clipboard.set_image(frame.width, frame.height, rgba)?;
        anyhow::Ok(png)
    });
    cx.spawn(async move |cx| {
        let png = match encode.await {
            Ok(png) => png,
            Err(e) => {
                tracing::error!(error = %e, "copying the capture failed");
                return;
            }
        };
        #[cfg(not(target_os = "linux"))]
        cx.update(|cx| {
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                png.clone(),
            )));
        });
        tracing::info!(
            ms = pressed_at.elapsed().as_secs_f64() * 1000.0,
            "on the clipboard (from hotkey, including selection time)"
        );
        let save = cx
            .background_executor()
            .spawn(async move { output::save(&png) });
        match save.await {
            Ok(path) => tracing::info!(path = %path.display(), "saved"),
            Err(e) => tracing::error!(error = %e, "saving the capture failed"),
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_targets_are_display_local_clipped_and_skip_our_windows() {
        let display = Display {
            id: 1,
            name: "right".into(),
            bounds: Rect::new(1920, 0, 2560, 1440),
            scale: 1.0,
            primary: false,
        };
        let frame = Frame {
            display_id: 1,
            width: 2560,
            height: 1440,
            data: Vec::new(),
        };
        let window = |pid, bounds| WindowInfo {
            id: 0,
            pid,
            app: String::new(),
            title: String::new(),
            bounds,
        };
        let windows = [
            window(1, Rect::new(2000, 100, 800, 600)),
            window(1, Rect::new(1800, 50, 300, 300)), // straddles both displays
            window(1, Rect::new(0, 0, 500, 500)),     // other display only
            window(std::process::id(), Rect::new(1920, 0, 2560, 1440)),
        ];
        assert_eq!(
            snap_targets(&display, &frame, &windows),
            vec![Rect::new(80, 100, 800, 600), Rect::new(0, 50, 180, 300)]
        );
    }
}
