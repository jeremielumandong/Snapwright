//! Global shortcuts on Hyprland (Omarchy's compositor). Wayland gives apps no global key
//! grabs, so on Hyprland the app binds its own shortcuts through Hyprland's IPC socket.
//! Each bind runs `snapwright-app --capture-now` (or `--repeat-last`, `--record`,
//! `--record-gif`), which hands the request to the warm app (`instance.rs`) and exits.
//!
//! Binds made this way last until Hyprland reloads its config, which it does when
//! `hyprland.conf` is saved and on every Omarchy theme switch, so a thread on the event
//! socket puts them back on `configreloaded` (and tells the app, which re-reads the Omarchy
//! theme then). A shortcut the user already bound to something else is left alone. The
//! binds stay after the app quits; a press then starts it, which runs the capture.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use futures::channel::mpsc;
use serde::Deserialize;

use crate::hotkey::Action;

/// One of the app's shortcuts, as Hyprland names it.
struct Shortcut {
    action: Action,
    /// Modifiers in `bind` syntax.
    mods: &'static str,
    /// The same modifiers as Hyprland's mask (Shift 1, Ctrl 4, Alt 8, Super 64).
    modmask: u32,
    key: &'static str,
    /// The command-line flag that asks the app for `action`.
    flag: &'static str,
    label: &'static str,
}

/// The same keys as `hotkey.rs` uses on X11 and Windows.
const SHORTCUTS: [Shortcut; 4] = [
    Shortcut {
        action: Action::Capture,
        mods: "CTRL SHIFT",
        modmask: 5,
        key: "2",
        flag: crate::CAPTURE_NOW_ARG,
        label: "Ctrl+Shift+2",
    },
    Shortcut {
        action: Action::RepeatLast,
        mods: "CTRL ALT SHIFT",
        modmask: 13,
        key: "2",
        flag: crate::REPEAT_LAST_ARG,
        label: "Ctrl+Alt+Shift+2",
    },
    Shortcut {
        action: Action::Record,
        mods: "CTRL SHIFT",
        modmask: 5,
        key: "6",
        flag: crate::RECORD_ARG,
        label: "Ctrl+Shift+6",
    },
    Shortcut {
        action: Action::RecordGif,
        mods: "CTRL ALT SHIFT",
        modmask: 13,
        key: "6",
        flag: crate::RECORD_GIF_ARG,
        label: "Ctrl+Alt+Shift+6",
    },
];

/// How long an IPC request may take before the app gives up on it.
const TIMEOUT: Duration = Duration::from_secs(1);

/// The running Hyprland's socket folder, when this is a Hyprland session.
pub fn instance_dir() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .ok()
        .filter(|s| !s.is_empty())?;
    // Hyprland 0.40 moved the sockets from /tmp/hypr to the runtime folder.
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("hypr").join(&sig))
        .filter(|d| d.join(".socket.sock").exists());
    Some(runtime.unwrap_or_else(|| PathBuf::from("/tmp/hypr").join(sig)))
}

/// What a bind should run: the binary on disk (the AppImage itself, not its mount, which
/// goes away when the app quits).
pub fn exe() -> Option<PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
        return Some(PathBuf::from(appimage));
    }
    let exe = std::env::current_exe().ok()?;
    // A binary replaced by a rebuild reads back as "… (deleted)"; the new one is at the path.
    let s = exe.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(path) => PathBuf::from(path),
        None => exe,
    })
}

/// Sends one request to Hyprland's command socket and returns its reply.
fn request(dir: &Path, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(dir.join(".socket.sock"))
        .with_context(|| format!("no Hyprland socket in {}", dir.display()))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    // The leading "/" ends the (empty) flags, so a path in the arguments is not read as one.
    stream.write_all(command.as_bytes())?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

/// A bind as `hyprctl -j binds` lists it.
#[derive(Debug, Deserialize)]
struct Bind {
    modmask: u32,
    key: String,
    #[serde(default)]
    submap: String,
    #[serde(default)]
    dispatcher: String,
    #[serde(default)]
    arg: String,
}

/// The `exec` argument that runs `flag` in this app, quoted for `sh -c`.
fn command(exe: &Path, flag: &str) -> String {
    let exe = exe.to_string_lossy().replace('\'', r"'\''");
    format!("'{exe}' {flag}")
}

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// The key is free: bind it.
    Bind,
    /// An older Snapwright bind (another path): replace it.
    Replace,
    /// Already bound to this app.
    Keep,
    /// The user bound the key to something else.
    Taken,
}

fn plan(shortcut: &Shortcut, wanted: &str, binds: &[Bind]) -> Plan {
    let existing = binds
        .iter()
        .find(|b| b.submap.is_empty() && b.modmask == shortcut.modmask && b.key == shortcut.key);
    match existing {
        None => Plan::Bind,
        Some(b) if b.dispatcher == "exec" && b.arg == wanted => Plan::Keep,
        Some(b)
            if b.dispatcher == "exec"
                && b.arg.contains("snapwright")
                && b.arg.split_whitespace().any(|w| w == shortcut.flag) =>
        {
            Plan::Replace
        }
        Some(_) => Plan::Taken,
    }
}

/// Binds the app's shortcuts. Returns the ones that are bound to it now, with their labels.
pub fn bind(dir: &Path, exe: &Path) -> Result<Vec<(Action, String)>> {
    let binds: Vec<Bind> =
        serde_json::from_str(&request(dir, "j/binds")?).context("unexpected reply to `binds`")?;
    let mut bound = Vec::new();
    for s in &SHORTCUTS {
        let wanted = command(exe, s.flag);
        let plan = plan(s, &wanted, &binds);
        if plan == Plan::Taken {
            tracing::info!(shortcut = s.label, "already bound in Hyprland; leaving it");
            continue;
        }
        if plan == Plan::Replace {
            request(dir, &format!("/keyword unbind {}, {}", s.mods, s.key))?;
        }
        if plan != Plan::Keep {
            let reply = request(
                dir,
                &format!("/keyword bind {}, {}, exec, {wanted}", s.mods, s.key),
            )?;
            if reply.trim() != "ok" {
                bail!("Hyprland refused the {} bind: {}", s.label, reply.trim());
            }
        }
        bound.push((s.action, s.label.to_owned()));
    }
    Ok(bound)
}

/// Watches Hyprland's event socket: after each config reload, binds the shortcuts again and
/// sends `()` on the returned stream.
pub fn watch(dir: PathBuf, exe: PathBuf) -> Result<mpsc::UnboundedReceiver<()>> {
    let events =
        UnixStream::connect(dir.join(".socket2.sock")).context("no Hyprland event socket")?;
    let (tx, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name("snapwright-hyprland".into())
        .spawn(move || {
            for line in BufReader::new(events).lines().map_while(Result::ok) {
                if !line.starts_with("configreloaded>>") {
                    continue;
                }
                if let Err(e) = bind(&dir, &exe) {
                    tracing::warn!(error = %e, "could not bind the shortcuts in Hyprland again");
                }
                if tx.unbounded_send(()).is_err() {
                    return;
                }
            }
        })?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    fn bind_json(modmask: u32, key: &str, dispatcher: &str, arg: &str) -> Bind {
        Bind {
            modmask,
            key: key.into(),
            submap: String::new(),
            dispatcher: dispatcher.into(),
            arg: arg.into(),
        }
    }

    #[test]
    fn quotes_the_binary_for_the_shell() {
        assert_eq!(
            command(Path::new("/opt/Snap wright/it's"), "--capture-now"),
            r"'/opt/Snap wright/it'\''s' --capture-now"
        );
    }

    #[test]
    fn plans_around_existing_binds() {
        let capture = &SHORTCUTS[0];
        let wanted = command(Path::new("/usr/bin/snapwright-app"), capture.flag);
        assert_eq!(plan(capture, &wanted, &[]), Plan::Bind);
        // Same keys, other modifiers: free.
        let other_mods = [bind_json(64 | 1, "2", "movetoworkspace", "2")];
        assert_eq!(plan(capture, &wanted, &other_mods), Plan::Bind);
        assert_eq!(
            plan(capture, &wanted, &[bind_json(5, "2", "exec", &wanted)]),
            Plan::Keep
        );
        let moved = [bind_json(
            5,
            "2",
            "exec",
            "'/tmp/old/snapwright-app' --capture-now",
        )];
        assert_eq!(plan(capture, &wanted, &moved), Plan::Replace);
        assert_eq!(
            plan(capture, &wanted, &[bind_json(5, "2", "exec", "grim")]),
            Plan::Taken
        );
        // A user's own bind in a submap does not count.
        let in_submap = [Bind {
            submap: "resize".into(),
            ..bind_json(5, "2", "exec", "grim")
        }];
        assert_eq!(plan(capture, &wanted, &in_submap), Plan::Bind);
    }

    /// A stand-in for Hyprland's command socket: answers `binds` with `existing` and
    /// `ok` to anything else, and records the requests.
    fn fake_hyprland(dir: &Path, existing: &'static str) -> Arc<Mutex<Vec<String>>> {
        let listener = UnixListener::bind(dir.join(".socket.sock")).expect("bind");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let reply = if req == "j/binds" { existing } else { "ok" };
                let _ = stream.write_all(reply.as_bytes());
                log.lock().expect("lock").push(req);
            }
        });
        seen
    }

    #[test]
    fn binds_free_keys_over_ipc() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Ctrl+Shift+6 is the user's; Ctrl+Shift+2 is an older Snapwright bind.
        let existing = r#"[
            {"locked":false,"modmask":5,"submap":"","key":"6","keycode":0,"dispatcher":"exec","arg":"wf-recorder"},
            {"locked":false,"modmask":5,"submap":"","key":"2","keycode":0,"dispatcher":"exec","arg":"/old/snapwright-app --capture-now"}
        ]"#;
        let seen = fake_hyprland(dir.path(), existing);
        let bound = bind(dir.path(), Path::new("/usr/bin/snapwright-app")).expect("bind");
        assert_eq!(
            bound.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            [Action::Capture, Action::RepeatLast, Action::RecordGif]
        );
        let seen = seen.lock().expect("lock").clone();
        assert_eq!(
            seen,
            [
                "j/binds",
                "/keyword unbind CTRL SHIFT, 2",
                "/keyword bind CTRL SHIFT, 2, exec, '/usr/bin/snapwright-app' --capture-now",
                "/keyword bind CTRL ALT SHIFT, 2, exec, '/usr/bin/snapwright-app' --repeat-last",
                "/keyword bind CTRL ALT SHIFT, 6, exec, '/usr/bin/snapwright-app' --record-gif",
            ]
        );
    }
}
