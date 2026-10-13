//! Preferences that outlive a run, in `settings.toml` in the platform config folder
//! (`~/.config/snapwright` on Linux). One `key = "value"` per line; for now only the theme.
//! Read once before the app starts and written off the UI thread.

use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::theme::Choice;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settings {
    /// The theme picked from the tray; `None` until the user picks one.
    pub theme: Option<Choice>,
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

/// Writes the settings. Disk I/O: run it off the UI thread.
pub fn save(settings: &Settings) -> Result<()> {
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
        if key.trim() == "theme" {
            out.theme = Choice::from_key(value);
        }
    }
    out
}

fn render(settings: &Settings) -> String {
    let mut out = String::new();
    if let Some(theme) = settings.theme {
        out.push_str(&format!("theme = \"{}\"\n", theme.key()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for theme in [None, Some(Choice::Omarchy), Some(Choice::Light)] {
            let s = Settings { theme };
            assert_eq!(parse(&render(&s)), s);
        }
        assert_eq!(parse("# nothing\ntheme = \"sepia\"\n").theme, None);
        assert_eq!(parse("theme=dark").theme, Some(Choice::Dark));
    }
}
