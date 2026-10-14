//! The app's keyboard shortcuts: what each action is bound to, saved in `settings.toml` as
//! `shortcut.capture = "Ctrl+Shift+2"` (`"off"` for none; a missing line is the default),
//! and applied to whichever backend this session has: global hotkeys (X11, macOS, Windows),
//! Hyprland binds (`hyprland.rs`), or nothing (other Wayland compositors, where users bind
//! `snapwright-app --capture-now` themselves).
//!
//! The Shortcuts window (`shortcuts_view.rs`) changes them while the app runs: `set` saves
//! and re-binds, and `suspend`/`resume` lift the binds while it listens for a new key, so
//! pressing the current shortcut there records it instead of starting a capture.

use std::str::FromStr as _;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use global_hotkey::hotkey::HotKey;
use gpui_kit::{App, Global};

use crate::hotkey::{self, Action};
use crate::{settings, tray};

/// Modifier bits, the same as Hyprland's modmask.
pub const SHIFT: u8 = 1;
pub const CTRL: u8 = 4;
pub const ALT: u8 = 8;
/// Cmd on macOS, the Windows key, Super on Linux.
pub const SUPER: u8 = 64;

/// The keys a shortcut can end in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `A` to `Z`, upper case.
    Letter(char),
    /// `0` to `9`.
    Digit(char),
    /// F1 to F24.
    F(u8),
    Print,
}

impl Key {
    fn parse(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let mut chars = lower.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return match c {
                'a'..='z' => Some(Self::Letter(c.to_ascii_uppercase())),
                '0'..='9' => Some(Self::Digit(c)),
                _ => None,
            };
        }
        match lower.as_str() {
            "print" | "printscreen" | "prtsc" => Some(Self::Print),
            f => f
                .strip_prefix('f')
                .and_then(|n| n.parse().ok())
                .filter(|n| (1..=24).contains(n))
                .map(Self::F),
        }
    }

    /// The name in labels and `settings.toml`.
    fn name(self) -> String {
        match self {
            Self::Letter(c) | Self::Digit(c) => c.to_string(),
            Self::F(n) => format!("F{n}"),
            Self::Print => "Print".into(),
        }
    }
}

/// Modifiers plus a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub mods: u8,
    pub key: Key,
}

/// Label order and names: Ctrl, Super, Alt, Shift (Cmd and Option on macOS).
fn mod_names() -> [(u8, &'static str); 4] {
    if cfg!(target_os = "macos") {
        [
            (CTRL, "Ctrl"),
            (SUPER, "Cmd"),
            (ALT, "Option"),
            (SHIFT, "Shift"),
        ]
    } else if cfg!(windows) {
        [
            (CTRL, "Ctrl"),
            (SUPER, "Win"),
            (ALT, "Alt"),
            (SHIFT, "Shift"),
        ]
    } else {
        [
            (CTRL, "Ctrl"),
            (SUPER, "Super"),
            (ALT, "Alt"),
            (SHIFT, "Shift"),
        ]
    }
}

impl Shortcut {
    pub const fn new(mods: u8, key: Key) -> Self {
        Self { mods, key }
    }

    /// Reads `Ctrl+Shift+2` (any case, any modifier order; Cmd, Win, Meta and Super are
    /// the same modifier, as are Alt and Option).
    pub fn parse(text: &str) -> Option<Self> {
        let mut mods = 0;
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            let bit = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => CTRL,
                "alt" | "option" => ALT,
                "shift" => SHIFT,
                "super" | "cmd" | "command" | "win" | "meta" => SUPER,
                _ => {
                    if key.is_some() {
                        return None;
                    }
                    key = Some(Key::parse(part)?);
                    continue;
                }
            };
            mods |= bit;
        }
        let shortcut = Self::new(mods, key?);
        shortcut.usable().then_some(shortcut)
    }

    /// Letters and digits need a modifier besides Shift, so typing never sets one off.
    pub fn usable(&self) -> bool {
        match self.key {
            Key::Letter(_) | Key::Digit(_) => self.mods & !SHIFT != 0,
            Key::F(_) | Key::Print => true,
        }
    }

    /// `Ctrl+Shift+2`, with this platform's modifier names.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = mod_names()
            .into_iter()
            .filter(|(bit, _)| self.mods & bit != 0)
            .map(|(_, name)| name.to_owned())
            .collect();
        parts.push(self.key.name());
        parts.join("+")
    }

    /// The same shortcut for the `global-hotkey` crate.
    pub fn hotkey(&self) -> Option<HotKey> {
        let mut text: String = [
            (CTRL, "ctrl+"),
            (SUPER, "super+"),
            (ALT, "alt+"),
            (SHIFT, "shift+"),
        ]
        .into_iter()
        .filter(|(bit, _)| self.mods & bit != 0)
        .map(|(_, name)| name)
        .collect();
        text.push_str(&match self.key {
            Key::Print => "PrintScreen".to_owned(),
            key => key.name(),
        });
        HotKey::from_str(&text).ok()
    }

    /// Hyprland's names for the modifiers, `CTRL SHIFT`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn hypr_mods(&self) -> String {
        hypr_mods(self.mods).join(" ")
    }

    /// Hyprland's (xkb's) name for the key. Letters are lower case: xkb reads `A` as the
    /// shifted keysym, which an unshifted press never matches.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn hypr_key(&self) -> String {
        match self.key {
            Key::Letter(c) => c.to_ascii_lowercase().to_string(),
            key => key.name(),
        }
    }

    /// A key press in a window, as GPUI reports it (`key` lower case, Shift folded into
    /// the symbol for digits: Shift+2 arrives as `@`). `None` for keys a shortcut cannot
    /// end in, and for letters and digits without a modifier besides Shift.
    pub fn from_keystroke(mods: u8, key: &str) -> Option<Self> {
        // The US layout's shifted digits.
        const SHIFTED: [(char, char); 10] = [
            ('!', '1'),
            ('@', '2'),
            ('#', '3'),
            ('$', '4'),
            ('%', '5'),
            ('^', '6'),
            ('&', '7'),
            ('*', '8'),
            ('(', '9'),
            (')', '0'),
        ];
        let mut chars = key.chars();
        let shifted = match (chars.next(), chars.next()) {
            (Some(c), None) => SHIFTED.iter().find(|(s, _)| *s == c).map(|(_, d)| *d),
            _ => None,
        };
        let shortcut = match shifted {
            Some(digit) => Self::new(mods | SHIFT, Key::Digit(digit)),
            None => Self::new(mods, Key::parse(key)?),
        };
        shortcut.usable().then_some(shortcut)
    }
}

/// Hyprland's modifier names for a modmask, in a fixed order.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn hypr_mods(mask: u8) -> Vec<&'static str> {
    [
        (CTRL, "CTRL"),
        (ALT, "ALT"),
        (SHIFT, "SHIFT"),
        (SUPER, "SUPER"),
    ]
    .into_iter()
    .filter(|(bit, _)| mask & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

/// Every action that can have a shortcut, in the order the Shortcuts window lists them.
pub const ACTIONS: [Action; 4] = [
    Action::Capture,
    Action::RepeatLast,
    Action::Record,
    Action::RecordGif,
];

/// What the action is called in the Shortcuts window and the tray.
pub fn title(action: Action) -> &'static str {
    match action {
        Action::Capture => "Capture region",
        Action::RepeatLast => "Repeat last region",
        Action::Record => "Record screen",
        Action::RecordGif => "Record GIF",
    }
}

/// The action's name in `settings.toml` (`shortcut.<name>`).
pub fn setting_name(action: Action) -> &'static str {
    match action {
        Action::Capture => "capture",
        Action::RepeatLast => "repeat-last",
        Action::Record => "record",
        Action::RecordGif => "record-gif",
    }
}

pub fn from_setting_name(name: &str) -> Option<Action> {
    ACTIONS.into_iter().find(|a| setting_name(*a) == name)
}

/// The shortcut for each action; `None` is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcuts([Option<Shortcut>; 4]);

impl Default for Shortcuts {
    /// Next to the system's own Cmd+Shift+3/4/5 on macOS; Shift+5 would collide with its
    /// recording toolbar, hence 6 for recording.
    fn default() -> Self {
        let primary = if cfg!(target_os = "macos") {
            SUPER
        } else {
            CTRL
        };
        Self([
            Some(Shortcut::new(primary | SHIFT, Key::Digit('2'))),
            Some(Shortcut::new(primary | ALT | SHIFT, Key::Digit('2'))),
            Some(Shortcut::new(primary | SHIFT, Key::Digit('6'))),
            Some(Shortcut::new(primary | ALT | SHIFT, Key::Digit('6'))),
        ])
    }
}

impl Shortcuts {
    /// No shortcuts at all (while the Shortcuts window listens for a key).
    pub const fn none() -> Self {
        Self([None; 4])
    }

    fn index(action: Action) -> usize {
        ACTIONS.iter().position(|a| *a == action).unwrap_or(0)
    }

    pub fn get(&self, action: Action) -> Option<Shortcut> {
        self.0[Self::index(action)]
    }

    /// Sets `action`'s shortcut, taking it off any other action that had it.
    pub fn set(&mut self, action: Action, shortcut: Option<Shortcut>) {
        if shortcut.is_some() {
            for s in &mut self.0 {
                if *s == shortcut {
                    *s = None;
                }
            }
        }
        self.0[Self::index(action)] = shortcut;
    }

    /// Each action with its shortcut, in `ACTIONS` order.
    pub fn iter(&self) -> impl Iterator<Item = (Action, Option<Shortcut>)> + '_ {
        ACTIONS.into_iter().zip(self.0)
    }
}

/// Where the shortcuts go in this session.
enum Backend {
    Hotkeys(hotkey::Hotkeys),
    #[cfg(target_os = "linux")]
    Hyprland {
        dir: std::path::PathBuf,
        exe: std::path::PathBuf,
        /// What the reload watcher binds again after Hyprland reloads its config.
        wanted: Arc<Mutex<Shortcuts>>,
    },
    /// A Wayland compositor without an API for this; the user binds keys themselves.
    Unsupported,
}

/// The shortcuts in use and the backend that holds them.
pub struct Service {
    backend: Backend,
    current: Shortcuts,
    suspended: bool,
}

impl Global for Service {}

/// The saved shortcuts in use.
pub fn current(cx: &App) -> Shortcuts {
    cx.try_global::<Service>()
        .map_or_else(Shortcuts::default, |s| s.current)
}

/// Whether this session can bind shortcuts at all.
pub fn supported(cx: &App) -> bool {
    cx.try_global::<Service>()
        .is_some_and(|s| !matches!(s.backend, Backend::Unsupported))
}

/// Whether the shortcuts are bound in Hyprland (they then show up in its keybindings).
pub fn in_hyprland(cx: &App) -> bool {
    #[cfg(target_os = "linux")]
    {
        cx.try_global::<Service>()
            .is_some_and(|s| matches!(s.backend, Backend::Hyprland { .. }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        false
    }
}

/// Picks the backend, binds `shortcuts` and starts listening for presses. Sets the
/// `tray::Labels` global (Hyprland fills it in once its binds are made).
pub fn init(shortcuts: Shortcuts, cx: &mut App) {
    let backend = if snapwright_capture::wayland_session() {
        wayland_backend(shortcuts)
    } else {
        match hotkey::start() {
            Ok((hotkeys, mut presses)) => {
                cx.spawn(async move |cx| {
                    while let Some((action, pressed_at)) = presses.next().await {
                        cx.update(|cx| crate::on_hotkey(action, pressed_at, cx));
                    }
                })
                .detach();
                Backend::Hotkeys(hotkeys)
            }
            Err(e) => {
                tracing::warn!(error = %e, "global hotkeys unavailable");
                Backend::Unsupported
            }
        }
    };
    cx.set_global(tray::Labels::default());
    #[cfg(target_os = "linux")]
    if let Backend::Hyprland { dir, exe, wanted } = &backend {
        match crate::hyprland::watch(dir.clone(), exe.clone(), wanted.clone()) {
            Ok(mut reloads) => {
                cx.spawn(async move |cx| {
                    while let Some(bound) = reloads.next().await {
                        cx.update(|cx| {
                            // Omarchy switches themes with a Hyprland reload, so this is also
                            // when to re-read it.
                            crate::omarchy::reload(cx);
                            if let Some(bound) = bound {
                                publish(&bound, cx);
                            }
                        });
                    }
                })
                .detach();
            }
            Err(e) => tracing::warn!(error = %e, "Hyprland events unavailable"),
        }
    }
    cx.set_global(Service {
        backend,
        current: shortcuts,
        suspended: false,
    });
    apply(shortcuts, cx);
}

#[cfg(target_os = "linux")]
fn wayland_backend(shortcuts: Shortcuts) -> Backend {
    match (crate::hyprland::instance_dir(), crate::hyprland::exe()) {
        (Some(dir), Some(exe)) => Backend::Hyprland {
            dir,
            exe,
            wanted: Arc::new(Mutex::new(shortcuts)),
        },
        _ => {
            tracing::info!(
                "no global hotkeys on this Wayland compositor; bind `snapwright-app \
                 --capture-now` (and `--repeat-last`) to keys in its config"
            );
            Backend::Unsupported
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn wayland_backend(_shortcuts: Shortcuts) -> Backend {
    Backend::Unsupported
}

/// Saves `shortcuts` and binds them.
pub fn set(shortcuts: Shortcuts, cx: &mut App) {
    if cx.has_global::<Service>() {
        let service = cx.global_mut::<Service>();
        service.current = shortcuts;
        service.suspended = false;
    }
    apply(shortcuts, cx);
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = settings::update(|s| s.shortcuts = shortcuts) {
                tracing::warn!(error = %e, "could not save the shortcuts");
            }
        })
        .detach();
}

/// Lifts every bind until `resume` (the Shortcuts window is listening for a key).
pub fn suspend(cx: &mut App) {
    if !cx.has_global::<Service>() {
        return;
    }
    let service = cx.global_mut::<Service>();
    if !service.suspended {
        service.suspended = true;
        apply(Shortcuts::none(), cx);
    }
}

/// Binds the saved shortcuts again after `suspend`.
pub fn resume(cx: &mut App) {
    if !cx.has_global::<Service>() {
        return;
    }
    let service = cx.global_mut::<Service>();
    if service.suspended {
        service.suspended = false;
        let current = service.current;
        apply(current, cx);
    }
}

/// Binds `shortcuts` in the backend and shows the ones that took.
fn apply(shortcuts: Shortcuts, cx: &mut App) {
    if !cx.has_global::<Service>() {
        return;
    }
    let service = cx.global_mut::<Service>();
    match &mut service.backend {
        Backend::Hotkeys(hotkeys) => {
            let bound = hotkeys.set(&shortcuts);
            publish(&bound, cx);
        }
        #[cfg(target_os = "linux")]
        Backend::Hyprland { dir, exe, wanted } => {
            if let Ok(mut w) = wanted.lock() {
                *w = shortcuts;
            }
            let (dir, exe) = (dir.clone(), exe.clone());
            // IPC with the compositor: off the UI thread.
            let task = cx
                .background_executor()
                .spawn(async move { crate::hyprland::bind(&dir, &exe, &shortcuts) });
            cx.spawn(async move |cx| match task.await {
                Ok(bound) => {
                    for (action, label) in &bound {
                        tracing::info!(hotkey = %label, ?action, "Hyprland bind added");
                    }
                    cx.update(|cx| publish(&bound, cx));
                }
                Err(e) => tracing::warn!(error = %e, "could not bind the shortcuts in Hyprland"),
            })
            .detach();
        }
        Backend::Unsupported => {}
    }
}

/// Shows the shortcuts that are bound now beside the tray items and in the windows.
fn publish(bound: &[(Action, String)], cx: &mut App) {
    // While suspended the labels keep showing the saved shortcuts (and a late Hyprland
    // reply from before `suspend` is stale); `resume` publishes again.
    if cx.try_global::<Service>().is_some_and(|s| s.suspended) {
        return;
    }
    let labels = tray::Labels::from_actions(bound);
    if let Some(tray) = cx.try_global::<tray::Tray>() {
        tray.set_labels(&labels);
    }
    cx.set_global(labels);
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_labels() {
        let s = Shortcut::parse("shift + ctrl + 2").expect("parses");
        assert_eq!(s, Shortcut::new(CTRL | SHIFT, Key::Digit('2')));
        if !cfg!(any(target_os = "macos", windows)) {
            assert_eq!(s.label(), "Ctrl+Shift+2");
            assert_eq!(
                Shortcut::parse("Cmd+Option+P").map(|s| s.label()),
                Some("Super+Alt+P".into())
            );
        }
        assert_eq!(Shortcut::parse("F5"), Some(Shortcut::new(0, Key::F(5))));
        assert_eq!(
            Shortcut::parse("Shift+Print"),
            Some(Shortcut::new(SHIFT, Key::Print))
        );
        // A bare or Shift-only letter would fire while typing.
        assert_eq!(Shortcut::parse("A"), None);
        assert_eq!(Shortcut::parse("Shift+2"), None);
        assert_eq!(Shortcut::parse("Ctrl+2+3"), None);
        assert_eq!(Shortcut::parse("Ctrl+F25"), None);
        assert_eq!(Shortcut::parse("Ctrl+"), None);
        for (_, s) in Shortcuts::default().iter() {
            let s = s.expect("default");
            assert_eq!(Shortcut::parse(&s.label()), Some(s));
            assert!(s.hotkey().is_some());
        }
    }

    #[test]
    fn reads_window_key_presses() {
        // Shift+2 arrives as "@" with Shift folded in.
        assert_eq!(
            Shortcut::from_keystroke(CTRL, "@"),
            Some(Shortcut::new(CTRL | SHIFT, Key::Digit('2')))
        );
        assert_eq!(
            Shortcut::from_keystroke(SUPER | SHIFT, "s"),
            Some(Shortcut::new(SUPER | SHIFT, Key::Letter('S')))
        );
        assert_eq!(
            Shortcut::from_keystroke(0, "f12"),
            Some(Shortcut::new(0, Key::F(12)))
        );
        assert_eq!(Shortcut::from_keystroke(0, "@"), None);
        assert_eq!(Shortcut::from_keystroke(CTRL, "tab"), None);
    }

    #[test]
    fn names_keys_for_hyprland() {
        let s = Shortcut::new(CTRL | ALT | SHIFT, Key::Letter('S'));
        assert_eq!(s.hypr_mods(), "CTRL ALT SHIFT");
        assert_eq!(s.hypr_key(), "s");
        assert_eq!(Shortcut::new(SUPER, Key::Print).hypr_key(), "Print");
    }

    #[test]
    fn a_shortcut_moves_between_actions() {
        let mut s = Shortcuts::default();
        let capture = s.get(Action::Capture);
        s.set(Action::Record, capture);
        assert_eq!(s.get(Action::Record), capture);
        assert_eq!(s.get(Action::Capture), None);
        s.set(Action::RepeatLast, None);
        assert_eq!(s.get(Action::RepeatLast), None);
    }
}
