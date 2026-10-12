//! Snapwright desktop app: a resident process that shows the selection overlay on a global
//! hotkey and puts the selected region on the clipboard.

// Release builds on Windows are GUI-subsystem binaries: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod countdown;
mod editor_view;
mod hotkey;
mod library_view;
mod output;
mod overlay;
mod selection;
mod session;
mod toast;
mod tray;

use std::time::Instant;

use anyhow::Result;
use futures::StreamExt as _;
use tracing_subscriber::EnvFilter;

/// `--capture-now` starts a capture as soon as the app is up (for tests and timing runs).
const CAPTURE_NOW_ARG: &str = "--capture-now";
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
    let capture_now = std::env::args().any(|a| a == CAPTURE_NOW_ARG);
    let show_library = std::env::args().any(|a| a == LIBRARY_ARG);
    let edit_latest = std::env::args().any(|a| a == EDIT_LATEST_ARG);
    let capture_in = std::env::args().find_map(|a| {
        a.strip_prefix(CAPTURE_IN_ARG)
            .and_then(|n| n.parse::<u32>().ok())
    });

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            if let Err(e) = session::init(cx) {
                tracing::error!(error = %e, "screen capture is unavailable");
                cx.quit();
                return;
            }
            let labels = match hotkey::register() {
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
            if capture_now {
                session::start(Instant::now(), cx);
            }
            if show_library {
                library_view::open(cx);
            }
            if let Some(seconds) = capture_in {
                countdown::start(seconds, cx);
            }
            if edit_latest {
                library_view::edit_latest(cx);
            }
        });
    Ok(())
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
