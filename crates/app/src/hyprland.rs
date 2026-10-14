//! Global shortcuts on Hyprland (Omarchy's compositor). Wayland gives apps no global key
//! grabs, so on Hyprland the app binds its own shortcuts through Hyprland's IPC socket.
//! Each bind runs `snapwright-app --capture-now` (or `--repeat-last`, `--record`,
//! `--record-gif`), which hands the request to the warm app (`instance.rs`) and exits.
//!
//! Hyprland's legacy config takes `keyword bind`; its Lua config (Omarchy 4) refuses that,
//! and binds are made with `eval` and `hl.bind` there instead. Which one runs is asked
//! each time. The keys are the user's choice (`shortcuts.rs`); our binds on other keys are
//! removed.
//!
//! Binds made this way last until Hyprland reloads its config, which it does when its
//! config is saved and on every Omarchy theme switch, so a thread on the event
//! socket puts them back on `configreloaded` (and tells the app, which re-reads the Omarchy
//! theme then). A shortcut the user already bound to something else is left alone. The
//! binds stay after the app quits; a press then starts it, which runs the capture.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::channel::mpsc;
use serde::Deserialize;

use crate::hotkey::Action;
use crate::shortcuts::{self, Shortcut, Shortcuts};

/// The command-line flag that asks the app for `action`.
fn flag(action: Action) -> &'static str {
    match action {
        Action::Capture => crate::CAPTURE_NOW_ARG,
        Action::RepeatLast => crate::REPEAT_LAST_ARG,
        Action::Record => crate::RECORD_ARG,
        Action::RecordGif => crate::RECORD_GIF_ARG,
    }
}

/// How the binds' descriptions start (Lua binds show up in `hyprctl binds` only by it).
const DESCRIPTION: &str = "Snapwright: ";

/// Which config language the running Hyprland was started with. Omarchy 4 runs Hyprland's
/// Lua config, which refuses `keyword` ("keyword can't work with non-legacy parsers");
/// binds are made with `eval` and `hl.bind` there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Config {
    Legacy,
    Lua,
}

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
    description: String,
    #[serde(default)]
    dispatcher: String,
    #[serde(default)]
    arg: String,
}

impl Bind {
    /// Made by this or an earlier Snapwright: a Lua bind with our description, or an
    /// `exec` of a Snapwright binary with one of its flags.
    fn ours(&self) -> bool {
        self.submap.is_empty()
            && (self.description.starts_with(DESCRIPTION)
                || (self.dispatcher == "exec"
                    && self.arg.contains("snapwright")
                    && self
                        .arg
                        .split_whitespace()
                        .any(|w| shortcuts::ACTIONS.iter().any(|a| flag(*a) == w))))
    }

    fn on(&self, s: &Shortcut) -> bool {
        self.submap.is_empty()
            && self.modmask == u32::from(s.mods)
            && self.key.eq_ignore_ascii_case(&s.hypr_key())
    }
}

/// The `exec` argument that runs `flag` in this app, quoted for `sh -c`.
fn command(exe: &Path, flag: &str) -> String {
    let exe = exe.to_string_lossy().replace('\'', r"'\''");
    format!("'{exe}' {flag}")
}

/// A Lua string literal.
fn lua_string(s: &str) -> String {
    let escaped = s
        .replace('\\', r"\\")
        .replace('"', "\\\"")
        .replace('\n', r"\n");
    format!("\"{escaped}\"")
}

/// The key string `hl.bind` and `hl.unbind` take: `CTRL + SHIFT + 2`.
fn lua_keys(mods: u8, key: &str) -> String {
    let mut parts = shortcuts::hypr_mods(mods);
    parts.push(key);
    parts.join(" + ")
}

fn bind_request(config: Config, action: Action, s: &Shortcut, cmd: &str) -> String {
    match config {
        Config::Legacy => format!(
            "/keyword bind {}, {}, exec, {cmd}",
            s.hypr_mods(),
            s.hypr_key()
        ),
        Config::Lua => format!(
            "/eval hl.bind({}, hl.dsp.exec_cmd({}), {{ description = {} }})",
            lua_string(&lua_keys(s.mods, &s.hypr_key())),
            lua_string(cmd),
            lua_string(&format!("{DESCRIPTION}{}", shortcuts::title(action))),
        ),
    }
}

fn unbind_request(config: Config, b: &Bind) -> String {
    let mods = u8::try_from(b.modmask).unwrap_or(0);
    match config {
        Config::Legacy => format!(
            "/keyword unbind {}, {}",
            shortcuts::hypr_mods(mods).join(" "),
            b.key
        ),
        Config::Lua => format!("/eval hl.unbind({})", lua_string(&lua_keys(mods, &b.key))),
    }
}

/// What to send to make `wanted` the app's binds, given the binds Hyprland has: requests
/// that unbind our binds on other keys (or, in Lua, all of ours: their commands are not
/// listed, so they are made anew), then requests that bind the free keys. A key the user
/// bound to something else is left alone. Returns the bind requests with their actions.
fn plan(
    config: Config,
    exe: &Path,
    wanted: &Shortcuts,
    binds: &[Bind],
) -> (Vec<String>, Vec<(Action, Shortcut, String)>) {
    let wanted: Vec<(Action, Shortcut, String)> = wanted
        .iter()
        .filter_map(|(a, s)| s.map(|s| (a, s, command(exe, flag(a)))))
        .collect();
    // A legacy bind that already runs what is wanted on that key stays.
    let exact = |b: &Bind, s: &Shortcut, cmd: &str| {
        config == Config::Legacy && b.dispatcher == "exec" && b.on(s) && b.arg == cmd
    };
    let in_place = |b: &Bind| wanted.iter().any(|(_, s, cmd)| exact(b, s, cmd));
    let unbinds = binds
        .iter()
        .filter(|b| b.ours() && !in_place(b))
        .map(|b| unbind_request(config, b))
        .collect();
    let binds_to_make = wanted
        .iter()
        .filter(|(_, s, _)| {
            let taken = binds.iter().any(|b| !b.ours() && b.on(s));
            if taken {
                tracing::info!(shortcut = %s.label(), "already bound in Hyprland; leaving it");
            }
            !taken
        })
        .map(|(a, s, cmd)| {
            let kept = binds.iter().any(|b| b.ours() && exact(b, s, cmd));
            (*a, *s, if kept { String::new() } else { cmd.clone() })
        })
        .collect();
    (unbinds, binds_to_make)
}

/// Asks the running Hyprland which config language it runs: only the Lua one has `eval`.
fn config(dir: &Path) -> Result<Config> {
    Ok(if request(dir, "/eval return true")?.trim() == "ok" {
        Config::Lua
    } else {
        Config::Legacy
    })
}

/// Makes `wanted` the app's binds, removing its binds on other keys. Returns the ones that
/// are bound to it now, with their labels.
pub fn bind(dir: &Path, exe: &Path, wanted: &Shortcuts) -> Result<Bound> {
    let config = config(dir)?;
    let binds: Vec<Bind> =
        serde_json::from_str(&request(dir, "j/binds")?).context("unexpected reply to `binds`")?;
    let (unbinds, to_bind) = plan(config, exe, wanted, &binds);
    for req in unbinds {
        let reply = request(dir, &req)?;
        if reply.trim() != "ok" {
            tracing::warn!(request = %req, reply = %reply.trim(), "Hyprland refused an unbind");
        }
    }
    let mut bound = Vec::new();
    for (action, s, cmd) in to_bind {
        // An empty command: the bind is already there.
        if !cmd.is_empty() {
            let reply = request(dir, &bind_request(config, action, &s, &cmd))?;
            if reply.trim() != "ok" {
                tracing::warn!(shortcut = %s.label(), reply = %reply.trim(), "Hyprland refused a bind");
                continue;
            }
        }
        bound.push((action, s.label()));
    }
    Ok(bound)
}

/// The shortcuts that are bound to the app now, with their labels.
pub type Bound = Vec<(Action, String)>;

/// Watches Hyprland's event socket: after each config reload, binds what `wanted` holds
/// again and sends the result (`None` if that failed) on the returned stream.
pub fn watch(
    dir: PathBuf,
    exe: PathBuf,
    wanted: Arc<Mutex<Shortcuts>>,
) -> Result<mpsc::UnboundedReceiver<Option<Bound>>> {
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
                let shortcuts = wanted.lock().map(|w| *w).unwrap_or_default();
                let bound = bind(&dir, &exe, &shortcuts)
                    .inspect_err(|e| {
                        tracing::warn!(error = %e, "could not bind the shortcuts in Hyprland again");
                    })
                    .ok();
                if tx.unbounded_send(bound).is_err() {
                    return;
                }
            }
        })?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;
    use crate::shortcuts::{ALT, CTRL, Key, SHIFT};

    fn bind_json(modmask: u32, key: &str, dispatcher: &str, arg: &str) -> Bind {
        Bind {
            modmask,
            key: key.into(),
            submap: String::new(),
            description: String::new(),
            dispatcher: dispatcher.into(),
            arg: arg.into(),
        }
    }

    const EXE: &str = "/usr/bin/snapwright-app";

    fn capture_only() -> Shortcuts {
        let mut s = Shortcuts::none();
        s.set(
            Action::Capture,
            Some(Shortcut::new(CTRL | SHIFT, Key::Digit('2'))),
        );
        s
    }

    #[test]
    fn quotes_the_binary_for_the_shell_and_lua() {
        assert_eq!(
            command(Path::new("/opt/Snap wright/it's"), "--capture-now"),
            r"'/opt/Snap wright/it'\''s' --capture-now"
        );
        assert_eq!(lua_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }

    #[test]
    fn plans_around_existing_binds() {
        let exe = Path::new(EXE);
        let wanted = capture_only();
        let cmd = command(exe, crate::CAPTURE_NOW_ARG);
        let plan_legacy = |binds: &[Bind]| {
            let (unbinds, binds) = plan(Config::Legacy, exe, &wanted, binds);
            let binds: Vec<String> = binds.into_iter().map(|(_, _, c)| c).collect();
            (unbinds, binds)
        };
        // Free: bind it.
        assert_eq!(plan_legacy(&[]), (vec![], vec![cmd.clone()]));
        // Same key, other modifiers: still free.
        let other_mods = [bind_json(64 | 1, "2", "movetoworkspace", "2")];
        assert_eq!(plan_legacy(&other_mods), (vec![], vec![cmd.clone()]));
        // Already bound to this app: nothing to send.
        assert_eq!(
            plan_legacy(&[bind_json(5, "2", "exec", &cmd)]),
            (vec![], vec![String::new()])
        );
        // An older Snapwright bind (another path): replaced.
        let moved = [bind_json(
            5,
            "2",
            "exec",
            "'/tmp/old/snapwright-app' --capture-now",
        )];
        assert_eq!(
            plan_legacy(&moved),
            (
                vec!["/keyword unbind CTRL SHIFT, 2".into()],
                vec![cmd.clone()]
            )
        );
        // The user's own bind: left alone, and the shortcut is not ours.
        assert_eq!(
            plan_legacy(&[bind_json(5, "2", "exec", "grim")]),
            (vec![], vec![])
        );
        // Our bind on a key the shortcut moved away from, and on one that is now off.
        let stale = [
            bind_json(13, "s", "exec", "'/usr/bin/snapwright-app' --capture-now"),
            bind_json(5, "6", "exec", "'/usr/bin/snapwright-app' --record"),
        ];
        assert_eq!(
            plan_legacy(&stale),
            (
                vec![
                    "/keyword unbind CTRL ALT SHIFT, s".into(),
                    "/keyword unbind CTRL SHIFT, 6".into()
                ],
                vec![cmd.clone()]
            )
        );
        // A user's own bind in a submap does not count.
        let in_submap = [Bind {
            submap: "resize".into(),
            ..bind_json(5, "2", "exec", "grim")
        }];
        assert_eq!(plan_legacy(&in_submap), (vec![], vec![cmd]));
    }

    #[test]
    fn lua_binds_are_made_anew() {
        let exe = Path::new(EXE);
        let ours = Bind {
            description: "Snapwright: Capture region".into(),
            ..bind_json(5, "2", "__lua", "12")
        };
        let theirs = Bind {
            description: "Agent".into(),
            ..bind_json(u32::from(CTRL | ALT | SHIFT), "2", "__lua", "13")
        };
        let mut wanted = Shortcuts::default();
        wanted.set(Action::Record, None);
        wanted.set(Action::RecordGif, None);
        let (unbinds, binds) = plan(Config::Lua, exe, &wanted, &[ours, theirs]);
        assert_eq!(unbinds, [r#"/eval hl.unbind("CTRL + SHIFT + 2")"#]);
        // Ctrl+Alt+Shift+2 is the user's.
        assert_eq!(
            binds.iter().map(|(a, _, _)| *a).collect::<Vec<_>>(),
            [Action::Capture]
        );
        let (action, s, cmd) = &binds[0];
        assert_eq!(
            bind_request(Config::Lua, *action, s, cmd),
            r#"/eval hl.bind("CTRL + SHIFT + 2", hl.dsp.exec_cmd("'/usr/bin/snapwright-app' --capture-now"), { description = "Snapwright: Capture region" })"#
        );
    }

    /// A stand-in for Hyprland's command socket: answers `binds` with `existing`, `eval`
    /// as the Lua or the legacy config does, and `ok` to anything else, and records the
    /// requests.
    fn fake_hyprland(dir: &Path, lua: bool, existing: &'static str) -> Arc<Mutex<Vec<String>>> {
        let listener = UnixListener::bind(dir.join(".socket.sock")).expect("bind");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let reply = if req == "j/binds" {
                    existing
                } else if req.starts_with("/eval") && !lua {
                    "eval is only supported with the lua config manager"
                } else if req.starts_with("/keyword") && lua {
                    "keyword can't work with non-legacy parsers"
                } else {
                    "ok"
                };
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
        let seen = fake_hyprland(dir.path(), false, existing);
        let bound = bind(dir.path(), Path::new(EXE), &Shortcuts::default()).expect("bind");
        assert_eq!(
            bound.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            [Action::Capture, Action::RepeatLast, Action::RecordGif]
        );
        let seen = seen.lock().expect("lock").clone();
        assert_eq!(
            seen,
            [
                "/eval return true",
                "j/binds",
                "/keyword unbind CTRL SHIFT, 2",
                "/keyword bind CTRL SHIFT, 2, exec, '/usr/bin/snapwright-app' --capture-now",
                "/keyword bind CTRL ALT SHIFT, 2, exec, '/usr/bin/snapwright-app' --repeat-last",
                "/keyword bind CTRL ALT SHIFT, 6, exec, '/usr/bin/snapwright-app' --record-gif",
            ]
        );
    }

    #[test]
    fn binds_with_lua_on_omarchy_4() {
        let dir = tempfile::tempdir().expect("tempdir");
        let existing = r#"[
            {"locked":false,"modmask":64,"submap":"","submap_universal":"false","key":"SPACE","keycode":0,"description":"Omarchy menu","dispatcher":"__lua","arg":"3"}
        ]"#;
        let seen = fake_hyprland(dir.path(), true, existing);
        let bound = bind(dir.path(), Path::new(EXE), &capture_only()).expect("bind");
        assert_eq!(
            bound,
            [(
                Action::Capture,
                Shortcut::new(CTRL | SHIFT, Key::Digit('2')).label()
            )]
        );
        let seen = seen.lock().expect("lock").clone();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(
            seen[2].starts_with(r#"/eval hl.bind("CTRL + SHIFT + 2""#),
            "{seen:?}"
        );
    }
}
