//! The global capture hotkey: Ctrl+Shift+2 (Cmd+Shift+2 on macOS), next to the system's own
//! Cmd+Shift+3/4/5 shortcuts. Configurable hotkeys come with settings.

use std::time::Instant;

use anyhow::Result;
use futures::channel::mpsc;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// Keeps the hotkey registered for the life of the app.
pub struct Registered {
    _manager: GlobalHotKeyManager,
    /// Human-readable shortcut.
    pub label: String,
}

impl gpui_kit::Global for Registered {}

/// Registers the capture hotkey. Each press arrives on the returned stream with the instant
/// the OS delivered it, which is where hotkey-to-overlay timing starts.
pub fn register() -> Result<(Registered, mpsc::UnboundedReceiver<Instant>)> {
    let (modifier, label) = if cfg!(target_os = "macos") {
        (Modifiers::SUPER, "Cmd+Shift+2")
    } else {
        (Modifiers::CONTROL, "Ctrl+Shift+2")
    };
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = HotKey::new(Some(modifier | Modifiers::SHIFT), Code::Digit2);
    manager.register(hotkey)?;
    let id = hotkey.id();
    let (tx, rx) = mpsc::unbounded();
    // The handler runs on the platform's event thread; it only stamps and forwards.
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == id && event.state == HotKeyState::Pressed {
            let _ = tx.unbounded_send(Instant::now());
        }
    }));
    Ok((
        Registered {
            _manager: manager,
            label: label.into(),
        },
        rx,
    ))
}
