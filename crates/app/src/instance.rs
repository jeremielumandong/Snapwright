//! One resident app per session. A second launch hands its request to the running app over
//! a Unix socket and exits, so `snapwright-app --capture-now` bound to a key in the
//! compositor (the way to get a hotkey on Wayland) starts a capture in the warm app, and
//! opening Snapwright from the launcher again shows the library.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Instant;

use futures::channel::mpsc;

/// What a launch asks the app for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Capture,
    RepeatLast,
    CaptureIn(u32),
    Library,
    EditLatest,
}

impl Request {
    fn encode(self) -> String {
        match self {
            Self::Capture => "capture".into(),
            Self::RepeatLast => "repeat".into(),
            Self::CaptureIn(n) => format!("capture-in {n}"),
            Self::Library => "library".into(),
            Self::EditLatest => "edit-latest".into(),
        }
    }

    fn decode(line: &str) -> Option<Self> {
        Some(match line.trim() {
            "capture" => Self::Capture,
            "repeat" => Self::RepeatLast,
            "library" => Self::Library,
            "edit-latest" => Self::EditLatest,
            other => Self::CaptureIn(other.strip_prefix("capture-in ")?.parse().ok()?),
        })
    }
}

/// One socket per display server, so an app under Xvfb (the e2e scripts) is separate from
/// the one on the real desktop.
fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|d| !d.is_empty())
        .or_else(|| std::env::var("DISPLAY").ok())
        .unwrap_or_default()
        .replace(['/', ':'], "");
    Some(PathBuf::from(dir).join(format!("snapwright-{display}.sock")))
}

/// Sends `requests` to a running app (the library when there are none). Returns whether one
/// was there to take them.
pub fn forward(requests: &[Request]) -> bool {
    let Some(path) = socket_path() else {
        return false;
    };
    let Ok(mut stream) = UnixStream::connect(&path) else {
        return false;
    };
    let requests = if requests.is_empty() {
        &[Request::Library][..]
    } else {
        requests
    };
    let message: String = requests.iter().map(|r| r.encode() + "\n").collect();
    match stream.write_all(message.as_bytes()) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "could not reach the running app; starting another");
            false
        }
    }
}

/// Takes requests from later launches. Each arrives with the instant it was read, where
/// hotkey-to-overlay timing starts for a compositor keybinding.
pub fn listen() -> anyhow::Result<mpsc::UnboundedReceiver<(Request, Instant)>> {
    let path = socket_path().ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR is not set"))?;
    // `forward` found no app on it, so a socket file left here is from one that crashed.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let (tx, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name("snapwright-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    match Request::decode(&line) {
                        Some(r) => {
                            if tx.unbounded_send((r, Instant::now())).is_err() {
                                return;
                            }
                        }
                        None => tracing::warn!(%line, "unknown request from another launch"),
                    }
                }
            }
        })?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        for r in [
            Request::Capture,
            Request::RepeatLast,
            Request::CaptureIn(3),
            Request::Library,
            Request::EditLatest,
        ] {
            assert_eq!(Request::decode(&r.encode()), Some(r));
        }
        assert_eq!(Request::decode("capture-in x"), None);
        assert_eq!(Request::decode("reboot"), None);
    }
}
