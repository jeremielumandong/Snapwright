//! Preferences that outlive a run, in `settings.toml` in the platform config folder
//! (`~/.config/snapwright` on Linux). One `key = "value"` per line: the theme and the
//! shortcuts that differ from the defaults. Read once before the app starts and written
//! off the UI thread.

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context as _, Result};

use crate::shortcuts::{self, Shortcut, Shortcuts};
use crate::theme::Choice;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settings {
    /// The theme picked from the tray; `None` until the user picks one.
    pub theme: Option<Choice>,
    pub shortcuts: Shortcuts,
}

/// Saves after reading, so two changes saved at once both land.
static WRITING: Mutex<()> = Mutex::new(());

/// Changes the saved settings with `change`. Disk I/O: run it off the UI thread.
pub fn update(change: impl FnOnce(&mut Settings)) -> Result<()> {
    let _guard = WRITING
        .lock()
        .map_err(|_| anyhow::anyhow!("settings lock poisoned"))?;
    let mut settings = load();
    change(&mut settings);
    save(&settings)
}

fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Snapwright")
        .map(|d| d.config_dir().join("settings.toml"))
}

/// The saved settings, or the defaults when there are none.
pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse(&s))
        .unwrap_or_default()
}

/// Writes the settings.
fn save(settings: &Settings) -> Result<()> {
    let path = path().context("no home directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.part");
    std::fs::write(&tmp, render(settings))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn parse(src: &str) -> Settings {
    let mut out = Settings::default();
    for line in src.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let key = key.trim();
        if key == "theme" {
            out.theme = Choice::from_key(value);
        } else if let Some(action) = key
            .strip_prefix("shortcut.")
            .and_then(shortcuts::from_setting_name)
        {
            if value.eq_ignore_ascii_case("off") {
                out.shortcuts.set(action, None);
            } else if let Some(s) = Shortcut::parse(value) {
                out.shortcuts.set(action, Some(s));
            }
        }
    }
    out
}

fn render(settings: &Settings) -> String {
    let mut out = String::new();
    if let Some(theme) = settings.theme {
        out.push_str(&format!("theme = \"{}\"\n", theme.key()));
    }
    let defaults = Shortcuts::default();
    for (action, shortcut) in settings.shortcuts.iter() {
        if shortcut != defaults.get(action) {
            let value = shortcut.map_or_else(|| "off".to_owned(), |s| s.label());
            let name = shortcuts::setting_name(action);
            out.push_str(&format!("shortcut.{name} = \"{value}\"\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::Action;
    use crate::shortcuts::{Key, SHIFT, SUPER};

    #[test]
    fn round_trips() {
        for theme in [
            None,
            Some(Choice::Omarchy),
            Some(Choice::Light),
            Some(Choice::Named("rose-pine")),
        ] {
            let s = Settings {
                theme,
                ..Settings::default()
            };
            assert_eq!(parse(&render(&s)), s);
        }
        let mut shortcuts = Shortcuts::default();
        shortcuts.set(
            Action::Capture,
            Some(Shortcut::new(SUPER | SHIFT, Key::Letter('S'))),
        );
        shortcuts.set(Action::RecordGif, None);
        let s = Settings {
            theme: None,
            shortcuts,
        };
        let text = render(&s);
        assert_eq!(text.lines().count(), 2, "only the changed ones: {text}");
        assert!(text.contains("shortcut.record-gif = \"off\""), "{text}");
        assert_eq!(parse(&text), s);
        // Unreadable shortcuts keep the default.
        assert_eq!(
            parse("shortcut.capture = \"Ctrl+Nope\"\nshortcut.other = \"F1\"").shortcuts,
            Shortcuts::default()
        );
        assert_eq!(parse("# nothing\ntheme = \"sepia\"\n").theme, None);
        assert_eq!(parse("theme=dark").theme, Some(Choice::Dark));
    }
}
