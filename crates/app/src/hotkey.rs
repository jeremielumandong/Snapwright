//! Global hotkeys, next to the system's own Cmd+Shift+3/4/5 shortcuts on macOS:
//!
//! - Capture: Ctrl+Shift+2 (Cmd+Shift+2 on macOS).
//! - Repeat last region: Ctrl+Alt+Shift+2 (Cmd+Option+Shift+2).
//! - Record the screen (MP4), or stop recording: Ctrl+Shift+6 (Cmd+Shift+6). Shift+5 would
//!   collide with macOS's own recording toolbar.
//! - Record a GIF, or stop: Ctrl+Alt+Shift+6 (Cmd+Option+Shift+6).
//!
//! Configurable hotkeys come with settings.

use std::time::Instant;

use anyhow::Result;
use futures::channel::mpsc;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// What a hotkey asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Capture,
    RepeatLast,
    Record,
    RecordGif,
}

/// Keeps the hotkeys registered for the life of the app.
pub struct Registered {
    _manager: GlobalHotKeyManager,
    /// Human-readable shortcut for each action that registered.
    pub labels: Vec<(Action, String)>,
}

impl Registered {
    pub fn label(&self, action: Action) -> Option<&str> {
        self.labels
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, l)| l.as_str())
    }
}

impl gpui_kit::Global for Registered {}

/// Registers the hotkeys. Each press arrives on the returned stream with the instant the OS
/// delivered it, which is where hotkey-to-overlay timing starts. A hotkey another app
/// already owns is skipped with a warning; the call fails only if none register.
pub fn register() -> Result<(Registered, mpsc::UnboundedReceiver<(Action, Instant)>)> {
    let (primary, primary_label, alt_label) = if cfg!(target_os = "macos") {
        (Modifiers::SUPER, "Cmd", "Option")
    } else {
        (Modifiers::CONTROL, "Ctrl", "Alt")
    };
    let wanted = [
        (
            Action::Capture,
            HotKey::new(Some(primary | Modifiers::SHIFT), Code::Digit2),
            format!("{primary_label}+Shift+2"),
        ),
        (
            Action::RepeatLast,
            HotKey::new(
                Some(primary | Modifiers::ALT | Modifiers::SHIFT),
                Code::Digit2,
            ),
            format!("{primary_label}+{alt_label}+Shift+2"),
        ),
        (
            Action::Record,
            HotKey::new(Some(primary | Modifiers::SHIFT), Code::Digit6),
            format!("{primary_label}+Shift+6"),
        ),
        (
            Action::RecordGif,
            HotKey::new(
                Some(primary | Modifiers::ALT | Modifiers::SHIFT),
                Code::Digit6,
            ),
            format!("{primary_label}+{alt_label}+Shift+6"),
        ),
    ];
    let manager = GlobalHotKeyManager::new()?;
    let mut ids = Vec::new();
    let mut labels = Vec::new();
    for (action, hotkey, label) in wanted {
        match manager.register(hotkey) {
            Ok(()) => {
                ids.push((hotkey.id(), action));
                labels.push((action, label));
            }
            Err(e) => tracing::warn!(hotkey = %label, error = %e, "hotkey unavailable"),
        }
    }
    if ids.is_empty() {
        anyhow::bail!("no hotkey could be registered");
    }
    let (tx, rx) = mpsc::unbounded();
    // The handler runs on the platform's event thread; it only stamps and forwards.
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
            let _ = tx.unbounded_send((*action, Instant::now()));
        }
    }));
    Ok((
        Registered {
            _manager: manager,
            labels,
        },
        rx,
    ))
}
