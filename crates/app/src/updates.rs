//! Update state and the tray's update item, ported from Switchyard's `updates.rs`. Release
//! builds check GitHub shortly after launch and once a day after that; a newer release is
//! downloaded and verified in the background ([`crate::update`]), and the tray item turns
//! into "Restart to update". The new AppImage or Windows installer starts after the event
//! loop has returned ([`run_after_exit`]); on macOS the verified disk image is opened.
//!
//! Set `SNAPWRIGHT_NO_UPDATE_CHECK` to skip the automatic checks; "Check for updates" in
//! the tray still works.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::{App, Global};

use crate::update::{self, CURRENT_VERSION, Install, ReadyUpdate, UpdateInfo, UpdateStatus};

/// First check after launch: late enough not to compete with the warm-up window.
const FIRST_CHECK: Duration = Duration::from_secs(30);
/// The app stays resident for days; look again this often.
const RECHECK: Duration = Duration::from_secs(24 * 60 * 60);

/// What the updater is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing checked yet.
    #[default]
    Idle,
    /// Asking GitHub.
    Checking,
    /// The running version is the latest.
    UpToDate,
    /// A newer release this copy can't install itself (the release page is offered).
    Available(UpdateInfo),
    /// Downloading and verifying it.
    Downloading(UpdateInfo),
    /// Verified and staged; installs on restart.
    Ready(ReadyUpdate),
    /// A failed check, or a failed download of `info`.
    Failed {
        /// The release whose download failed (`None`: the check failed).
        info: Option<UpdateInfo>,
        /// What went wrong.
        error: String,
    },
}

struct Updates {
    phase: Phase,
    install: Install,
}
impl Global for Updates {}

/// The update to start once the app has quit.
static AFTER_EXIT: Mutex<Option<ReadyUpdate>> = Mutex::new(None);

/// Start what "Restart to update" staged. Called by `main` after the event loop returns;
/// does nothing otherwise.
pub fn run_after_exit() {
    let ready = AFTER_EXIT.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(ready) = ready {
        update::run_after_exit(&ready);
    }
}

/// Automatic checks run in release builds only (debug builds would query GitHub on every
/// launch) and never when `SNAPWRIGHT_NO_UPDATE_CHECK` is set (tests, benchmarks).
fn automatic_checks_allowed() -> bool {
    !cfg!(debug_assertions) && std::env::var_os("SNAPWRIGHT_NO_UPDATE_CHECK").is_none()
}

/// Staged downloads live here; only one pending update is kept.
fn updates_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Snapwright").map(|d| d.data_dir().join("updates"))
}

/// Sets up the state and schedules the automatic checks.
pub fn init(cx: &mut App) {
    cx.set_global(Updates {
        phase: Phase::Idle,
        install: Install::detect(),
    });
    refresh_tray(cx);
    if !automatic_checks_allowed() {
        return;
    }
    cx.spawn(async move |cx| {
        let mut wait = FIRST_CHECK;
        loop {
            cx.background_executor().timer(wait).await;
            wait = RECHECK;
            cx.update(|cx| {
                // A download in flight or staged needs no new check.
                if !matches!(
                    phase(cx),
                    Phase::Checking | Phase::Downloading(_) | Phase::Ready(_)
                ) {
                    check(cx);
                }
            });
        }
    })
    .detach();
}

fn phase(cx: &App) -> Phase {
    cx.try_global::<Updates>()
        .map(|u| u.phase.clone())
        .unwrap_or_default()
}

fn set_phase(phase: Phase, cx: &mut App) {
    if cx.has_global::<Updates>() {
        cx.global_mut::<Updates>().phase = phase;
    }
    refresh_tray(cx);
}

/// Ask GitHub; download a newer release this copy can install.
fn check(cx: &mut App) {
    let Some(install) = cx.try_global::<Updates>().map(|u| u.install.clone()) else {
        return;
    };
    set_phase(Phase::Checking, cx);
    let task = cx
        .background_executor()
        .spawn(async move { update::check(&install) });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(UpdateStatus::UpToDate) => set_phase(Phase::UpToDate, cx),
            Ok(UpdateStatus::Available(info)) => {
                tracing::info!(version = %info.version, "update available");
                if info.asset.is_some() {
                    download(info, cx);
                } else {
                    set_phase(Phase::Available(info), cx);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check failed");
                set_phase(
                    Phase::Failed {
                        info: None,
                        error: e.to_string(),
                    },
                    cx,
                );
            }
        });
    })
    .detach();
}

fn download(info: UpdateInfo, cx: &mut App) {
    let Some(install) = cx.try_global::<Updates>().map(|u| u.install.clone()) else {
        return;
    };
    let Some(dir) = updates_dir() else {
        set_phase(
            Phase::Failed {
                info: Some(info),
                error: "no data folder".into(),
            },
            cx,
        );
        return;
    };
    set_phase(Phase::Downloading(info.clone()), cx);
    let task = cx.background_executor().spawn({
        let info = info.clone();
        async move { update::download(&info, &install, &dir) }
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(ready) => {
                tracing::info!(version = %ready.version, file = %ready.file.display(), "update ready");
                set_phase(Phase::Ready(ready), cx);
            }
            Err(e) => {
                tracing::warn!(error = %e, version = %info.version, "update download failed");
                set_phase(
                    Phase::Failed {
                        info: Some(info),
                        error: e.to_string(),
                    },
                    cx,
                );
            }
        });
    })
    .detach();
}

/// The tray's update item was clicked.
pub fn on_tray(cx: &mut App) {
    match phase(cx) {
        Phase::Checking | Phase::Downloading(_) => {}
        Phase::Idle | Phase::UpToDate | Phase::Failed { info: None, .. } => check(cx),
        Phase::Available(info) => cx.open_url(&info.release_url),
        Phase::Failed {
            info: Some(info), ..
        } => download(info, cx),
        Phase::Ready(ready) => apply(ready, cx),
    }
}

/// Install a staged update: open the macOS disk image, or quit and start the new AppImage or
/// the Windows installer.
fn apply(ready: ReadyUpdate, cx: &mut App) {
    if matches!(ready.install, Install::MacApp(_)) {
        // Finder mounts the image; dragging Snapwright to Applications replaces this copy.
        cx.open_with_system(&ready.file);
        return;
    }
    if crate::recording::is_busy(cx) {
        tracing::info!("not restarting during a recording");
        return;
    }
    *AFTER_EXIT.lock().unwrap_or_else(|p| p.into_inner()) = Some(ready);
    cx.quit();
}

/// What the tray item says, and whether it can be clicked.
fn label(phase: &Phase) -> (String, bool) {
    match phase {
        Phase::Idle => ("Check for updates".into(), true),
        Phase::Checking => ("Checking for updates…".into(), false),
        Phase::UpToDate => (format!("Snapwright {CURRENT_VERSION} is up to date"), true),
        Phase::Available(info) => (format!("Download Snapwright {}", info.version), true),
        Phase::Downloading(info) => (format!("Downloading Snapwright {}…", info.version), false),
        Phase::Ready(r) if matches!(r.install, Install::MacApp(_)) => {
            (format!("Install Snapwright {}", r.version), true)
        }
        Phase::Ready(r) => (format!("Restart to update to {}", r.version), true),
        Phase::Failed { info: None, .. } => ("Update check failed · Retry".into(), true),
        Phase::Failed { info: Some(i), .. } => {
            (format!("Update to {} failed · Retry", i.version), true)
        }
    }
}

fn refresh_tray(cx: &mut App) {
    let (text, enabled) = label(&phase(cx));
    if let Some(tray) = cx.try_global::<crate::tray::Tray>() {
        tray.set_update(&text, enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> UpdateInfo {
        UpdateInfo {
            version: "9.0.0".into(),
            release_url: "https://github.com/x/y/releases/tag/v9.0.0".into(),
            asset: None,
        }
    }

    #[test]
    fn busy_phases_disable_the_item() {
        assert!(!label(&Phase::Checking).1);
        assert!(!label(&Phase::Downloading(info())).1);
        assert!(label(&Phase::Idle).1);
        assert!(label(&Phase::Available(info())).1);
    }

    #[test]
    fn ready_says_how_it_installs() {
        let ready = |install| {
            Phase::Ready(ReadyUpdate {
                version: "9.0.0".into(),
                file: "x".into(),
                install,
            })
        };
        assert_eq!(
            label(&ready(Install::WindowsInstaller)).0,
            "Restart to update to 9.0.0"
        );
        assert_eq!(
            label(&ready(Install::MacApp("/A/Snapwright.app".into()))).0,
            "Install Snapwright 9.0.0"
        );
    }
}
