//! Snapwright desktop app: a resident process that shows the selection overlay on a global
//! hotkey and puts the selected region on the clipboard.

// Release builds on Windows are GUI-subsystem binaries: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod countdown;
mod editor_view;
mod hotkey;
#[cfg(target_os = "linux")]
mod instance;
mod library_view;
mod ocr;
mod output;
mod overlay;
mod selection;
mod session;
mod theme;
mod toast;
mod tray;

use std::time::Instant;

use anyhow::Result;
use futures::StreamExt as _;
use tracing_subscriber::EnvFilter;

#[cfg(target_os = "linux")]
use instance::Request;

/// What a launch asks for (on Linux, also what a later launch hands to the running app).
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Capture,
    RepeatLast,
    CaptureIn(u32),
    Library,
    EditLatest,
}

/// `--capture-now` starts a capture as soon as the app is up, or in the running app (bind it
/// to a key in a Wayland compositor).
const CAPTURE_NOW_ARG: &str = "--capture-now";
/// `--repeat-last` captures the last region again.
const REPEAT_LAST_ARG: &str = "--repeat-last";
/// `--library` opens the library window at launch.
const LIBRARY_ARG: &str = "--library";
/// `--edit-latest` opens the newest capture in the editor (for tests).
const EDIT_LATEST_ARG: &str = "--edit-latest";
/// `--capture-in=N` starts a capture after an N-second countdown.
const CAPTURE_IN_ARG: &str = "--capture-in=";
/// Delay for the tray's "Capture in 3 seconds".
const TRAY_DELAY_SECS: u32 = 3;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("SNAPWRIGHT_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let requests = requests(std::env::args());
    #[cfg(target_os = "linux")]
    if instance::forward(&requests) {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    let later_launches = match instance::listen() {
        Ok(rx) => Some(rx),
        Err(e) => {
            tracing::warn!(error = %e, "single-instance socket unavailable");
            None
        }
    };

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            if let Err(e) = session::init(cx) {
                tracing::error!(error = %e, "screen capture is unavailable");
                cx.quit();
                return;
            }
            ocr::init(cx);
            #[cfg(target_os = "linux")]
            if let Some(mut launches) = later_launches {
                cx.spawn(async move |cx| {
                    while let Some((request, at)) = launches.next().await {
                        cx.update(|cx| handle(request, at, cx));
                    }
                })
                .detach();
            }
            let hotkeys = if snapwright_capture::wayland_session() {
                // X11 key grabs under XWayland only fire while an X window has focus.
                tracing::info!(
                    "no global hotkeys on Wayland; bind `snapwright-app --capture-now` \
                     (and `--repeat-last`) to keys in your compositor"
                );
                Err(anyhow::anyhow!("Wayland session"))
            } else {
                hotkey::register()
            };
            let labels = match hotkeys {
                Ok((hotkeys, mut presses)) => {
                    for (action, label) in &hotkeys.labels {
                        tracing::info!(hotkey = %label, ?action, "hotkey registered");
                    }
                    let labels = tray::Labels {
                        capture: hotkeys.label(hotkey::Action::Capture).map(str::to_owned),
                        repeat: hotkeys.label(hotkey::Action::RepeatLast).map(str::to_owned),
                    };
                    cx.set_global(hotkeys);
                    cx.spawn(async move |cx| {
                        while let Some((action, pressed_at)) = presses.next().await {
                            cx.update(|cx| match action {
                                hotkey::Action::Capture => session::start(pressed_at, cx),
                                hotkey::Action::RepeatLast => session::repeat_last(pressed_at, cx),
                            });
                        }
                    })
                    .detach();
                    labels
                }
                Err(e) => {
                    tracing::warn!(error = %e, "global hotkeys unavailable");
                    tray::Labels::default()
                }
            };
            match tray::create(&labels) {
                Ok((tray, mut actions)) => {
                    cx.set_global(tray);
                    cx.spawn(async move |cx| {
                        while let Some(action) = actions.next().await {
                            cx.update(|cx| on_tray(action, cx));
                        }
                    })
                    .detach();
                }
                Err(e) => tracing::warn!(error = %e, "tray icon unavailable"),
            }
            let now = Instant::now();
            for request in &requests {
                handle(*request, now, cx);
            }
        });
    Ok(())
}

/// What the command line asks for, in the order the app does it.
fn requests(args: impl Iterator<Item = String>) -> Vec<Request> {
    let args: Vec<String> = args.collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let mut out = Vec::new();
    if has(CAPTURE_NOW_ARG) {
        out.push(Request::Capture);
    }
    if has(REPEAT_LAST_ARG) {
        out.push(Request::RepeatLast);
    }
    if has(LIBRARY_ARG) {
        out.push(Request::Library);
    }
    if let Some(n) = args.iter().find_map(|a| {
        a.strip_prefix(CAPTURE_IN_ARG)
            .and_then(|n| n.parse::<u32>().ok())
    }) {
        out.push(Request::CaptureIn(n));
    }
    if has(EDIT_LATEST_ARG) {
        out.push(Request::EditLatest);
    }
    out
}

fn handle(request: Request, at: Instant, cx: &mut gpui_kit::App) {
    match request {
        Request::Capture => session::start(at, cx),
        Request::RepeatLast => session::repeat_last(at, cx),
        Request::CaptureIn(seconds) => countdown::start(seconds, cx),
        Request::Library => library_view::open(cx),
        Request::EditLatest => library_view::edit_latest(cx),
    }
}

fn on_tray(action: tray::Action, cx: &mut gpui_kit::App) {
    match action {
        tray::Action::Capture => session::start(Instant::now(), cx),
        tray::Action::CaptureDelayed => countdown::start(TRAY_DELAY_SECS, cx),
        tray::Action::RepeatLast => session::repeat_last(Instant::now(), cx),
        tray::Action::OpenLibrary => library_view::open(cx),
        tray::Action::OpenFolder => match output::captures_dir() {
            Ok(dir) => {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    tracing::warn!(error = %e, "could not create the captures folder");
                }
                cx.open_with_system(&dir);
            }
            Err(e) => tracing::warn!(error = %e, "no captures folder"),
        },
        tray::Action::Quit => cx.quit(),
    }
}
