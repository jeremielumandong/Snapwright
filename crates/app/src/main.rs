//! Snapwright desktop app: a resident process that shows the selection overlay on a global
//! hotkey and puts the selected region on the clipboard.

// Release builds on Windows are GUI-subsystem binaries: no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod hotkey;
mod output;
mod overlay;
mod session;

use std::time::Instant;

use anyhow::Result;
use futures::StreamExt as _;
use tracing_subscriber::EnvFilter;

/// `--capture-now` starts a capture as soon as the app is up (for tests and timing runs).
const CAPTURE_NOW_ARG: &str = "--capture-now";

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("SNAPWRIGHT_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let capture_now = std::env::args().any(|a| a == CAPTURE_NOW_ARG);

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        if let Err(e) = session::init(cx) {
            tracing::error!(error = %e, "screen capture is unavailable");
            cx.quit();
            return;
        }
        match hotkey::register() {
            Ok((hotkey, mut presses)) => {
                tracing::info!(hotkey = %hotkey.label, "press the hotkey to capture");
                cx.set_global(hotkey);
                cx.spawn(async move |cx| {
                    while let Some(pressed_at) = presses.next().await {
                        cx.update(|cx| session::start(pressed_at, cx));
                    }
                })
                .detach();
            }
            Err(e) => tracing::warn!(error = %e, "global hotkey unavailable"),
        }
        if capture_now {
            session::start(Instant::now(), cx);
        }
    });
    Ok(())
}
