//! Snapwright desktop app: a resident process that shows the selection overlay on a global
//! hotkey and puts the selected region on the clipboard.

// Release builds on Windows are GUI-subsystem binaries: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod countdown;
mod editor_view;
mod hotkey;
#[cfg(target_os = "linux")]
mod hyprland;
#[cfg(target_os = "linux")]
mod instance;
mod library_view;
mod ocr;
mod omarchy;
mod output;
mod overlay;
mod recording;
mod selection;
mod session;
mod settings;
mod theme;
mod toast;
mod tray;

use std::time::Instant;

use anyhow::Result;
use futures::StreamExt as _;
use snapwright_record::Format;
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
    Record,
    RecordGif,
}

/// `--capture-now` starts a capture as soon as the app is up, or in the running app (what
/// the Hyprland binds run; bind it to a key in other Wayland compositors).
const CAPTURE_NOW_ARG: &str = "--capture-now";
/// `--repeat-last` captures the last region again.
const REPEAT_LAST_ARG: &str = "--repeat-last";
/// `--library` opens the library window at launch.
const LIBRARY_ARG: &str = "--library";
/// `--edit-latest` opens the newest capture in the editor (for tests).
const EDIT_LATEST_ARG: &str = "--edit-latest";
/// `--capture-in=N` starts a capture after an N-second countdown.
const CAPTURE_IN_ARG: &str = "--capture-in=";
/// `--record` starts recording the screen to MP4 (or stops a recording in progress).
const RECORD_ARG: &str = "--record";
/// `--record-gif` does the same for a GIF.
const RECORD_GIF_ARG: &str = "--record-gif";
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
    // Read before the UI thread starts, so the first window opens in the right colors.
    let omarchy = omarchy::load();
    let theme = settings::load().theme.unwrap_or(if omarchy.is_some() {
        theme::Choice::Omarchy
    } else {
        theme::Choice::System
    });

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let has_omarchy = omarchy.is_some();
            theme::init(theme, omarchy, cx);
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
            let labels = if snapwright_capture::wayland_session() {
                wayland_shortcuts(cx)
            } else {
                x11_hotkeys(cx)
            };
            cx.set_global(labels.clone());
            match tray::create(&labels, theme, has_omarchy) {
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

/// Registers the global hotkeys (X11, macOS, Windows) and returns their tray labels.
fn x11_hotkeys(cx: &mut gpui_kit::App) -> tray::Labels {
    let (hotkeys, mut presses) = match hotkey::register() {
        Ok(registered) => registered,
        Err(e) => {
            tracing::warn!(error = %e, "global hotkeys unavailable");
            return tray::Labels::default();
        }
    };
    for (action, label) in &hotkeys.labels {
        tracing::info!(hotkey = %label, ?action, "hotkey registered");
    }
    let labels = tray::Labels::from_actions(&hotkeys.labels);
    cx.set_global(hotkeys);
    cx.spawn(async move |cx| {
        while let Some((action, pressed_at)) = presses.next().await {
            cx.update(|cx| match action {
                hotkey::Action::Capture => session::start(pressed_at, cx),
                hotkey::Action::RepeatLast => session::repeat_last(pressed_at, cx),
                hotkey::Action::Record => recording::toggle(Format::Mp4, pressed_at, cx),
                hotkey::Action::RecordGif => recording::toggle(Format::Gif, pressed_at, cx),
            });
        }
    })
    .detach();
    labels
}

/// Wayland has no global key grabs (X11 grabs under XWayland only fire while an X window
/// has focus). On Hyprland the app binds its shortcuts in the compositor; elsewhere the user
/// binds `snapwright-app --capture-now` themselves.
#[cfg(target_os = "linux")]
fn wayland_shortcuts(cx: &mut gpui_kit::App) -> tray::Labels {
    let (Some(dir), Some(exe)) = (hyprland::instance_dir(), hyprland::exe()) else {
        tracing::info!(
            "no global hotkeys on this Wayland compositor; bind `snapwright-app --capture-now` \
             (and `--repeat-last`) to keys in its config"
        );
        return tray::Labels::default();
    };
    let bound = match hyprland::bind(&dir, &exe) {
        Ok(bound) => bound,
        Err(e) => {
            tracing::warn!(error = %e, "could not bind the shortcuts in Hyprland");
            return tray::Labels::default();
        }
    };
    for (action, label) in &bound {
        tracing::info!(hotkey = %label, ?action, "Hyprland bind added");
    }
    // Omarchy switches themes with a Hyprland reload, so this is also when to re-read it.
    match hyprland::watch(dir, exe) {
        Ok(mut reloads) => {
            cx.spawn(async move |cx| {
                while reloads.next().await.is_some() {
                    cx.update(omarchy::reload);
                }
            })
            .detach();
        }
        Err(e) => tracing::warn!(error = %e, "Hyprland events unavailable"),
    }
    tray::Labels::from_actions(&bound)
}

#[cfg(not(target_os = "linux"))]
fn wayland_shortcuts(_cx: &mut gpui_kit::App) -> tray::Labels {
    tray::Labels::default()
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
    if has(RECORD_ARG) {
        out.push(Request::Record);
    }
    if has(RECORD_GIF_ARG) {
        out.push(Request::RecordGif);
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
        Request::Record => recording::toggle(Format::Mp4, at, cx),
        Request::RecordGif => recording::toggle(Format::Gif, at, cx),
    }
}

fn on_tray(action: tray::Action, cx: &mut gpui_kit::App) {
    match action {
        tray::Action::Capture => session::start(Instant::now(), cx),
        tray::Action::CaptureDelayed => countdown::start(TRAY_DELAY_SECS, cx),
        tray::Action::RepeatLast => session::repeat_last(Instant::now(), cx),
        tray::Action::Record => recording::toggle(Format::Mp4, Instant::now(), cx),
        tray::Action::RecordGif => recording::toggle(Format::Gif, Instant::now(), cx),
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
        tray::Action::Theme(choice) => {
            theme::choose(choice, cx);
            if let Some(tray) = cx.try_global::<tray::Tray>() {
                tray.set_theme(choice);
            }
        }
        tray::Action::Quit => cx.quit(),
    }
}
