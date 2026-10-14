//! Global hotkeys on X11, macOS and Windows, for the shortcuts in `shortcuts.rs`
//! (Ctrl+Shift+2 to capture by default; Cmd on macOS). They can be changed while the app
//! runs: `Hotkeys::set` swaps the registered set.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use futures::channel::mpsc;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::shortcuts::Shortcuts;

/// What a hotkey asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Capture,
    RepeatLast,
    Record,
    RecordGif,
}

/// Keeps the hotkeys registered for the life of the app.
pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    /// Hotkey ids and their actions, read by the event handler.
    ids: Arc<Mutex<Vec<(u32, Action)>>>,
    registered: Vec<HotKey>,
}

/// Starts listening for hotkeys, with none registered yet (`Hotkeys::set` adds them). Each
/// press arrives on the returned stream with the instant the OS delivered it, which is
/// where hotkey-to-overlay timing starts.
pub fn start() -> Result<(Hotkeys, mpsc::UnboundedReceiver<(Action, Instant)>)> {
    let manager = GlobalHotKeyManager::new()?;
    let ids: Arc<Mutex<Vec<(u32, Action)>>> = Arc::default();
    let (tx, rx) = mpsc::unbounded();
    let lookup = ids.clone();
    // The handler runs on the platform's event thread; it only stamps and forwards.
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        let action = lookup.lock().ok().and_then(|ids| {
            ids.iter()
                .find(|(id, _)| *id == event.id)
                .map(|(_, action)| *action)
        });
        if let Some(action) = action {
            let _ = tx.unbounded_send((action, Instant::now()));
        }
    }));
    Ok((
        Hotkeys {
            manager,
            ids,
            registered: Vec::new(),
        },
        rx,
    ))
}

impl Hotkeys {
    /// Registers `shortcuts` in place of the current set. Returns the ones that registered,
    /// with their labels; one another app already owns is skipped with a warning.
    pub fn set(&mut self, shortcuts: &Shortcuts) -> Vec<(Action, String)> {
        for hotkey in self.registered.drain(..) {
            if let Err(e) = self.manager.unregister(hotkey) {
                tracing::warn!(error = %e, "could not unregister a hotkey");
            }
        }
        let mut ids = Vec::new();
        let mut labels = Vec::new();
        for (action, shortcut) in shortcuts.iter() {
            let Some(shortcut) = shortcut else {
                continue;
            };
            let label = shortcut.label();
            let Some(hotkey) = shortcut.hotkey() else {
                tracing::warn!(hotkey = %label, "no such hotkey on this platform");
                continue;
            };
            match self.manager.register(hotkey) {
                Ok(()) => {
                    tracing::info!(hotkey = %label, ?action, "hotkey registered");
                    ids.push((hotkey.id(), action));
                    self.registered.push(hotkey);
                    labels.push((action, label));
                }
                Err(e) => tracing::warn!(hotkey = %label, error = %e, "hotkey unavailable"),
            }
        }
        if let Ok(mut current) = self.ids.lock() {
            *current = ids;
        }
        labels
    }
}
