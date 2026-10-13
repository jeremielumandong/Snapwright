//! "Follow Omarchy": the windows take their colors from the active Omarchy theme.
//!
//! Ported from Switchyard (`crates/app/src/omarchy.rs`, same author, Apache-2.0). Omarchy 4
//! keeps the active theme in `~/.local/state/omarchy/current/theme` (a directory
//! `omarchy-theme-set` replaces on every switch, `theme.name` beside it); Omarchy 3 used a
//! symlink at `~/.config/omarchy/current/theme`. Themes carry a `colors.toml`: Omarchy 3
//! names the terminal colors `color0`..`color15`, Omarchy 4 `red`, `bright_blue`… with
//! `mode = "light"`/`"dark"`. An older theme's `alacritty.toml` is read when `colors.toml`
//! is missing. A `light.mode` file also marks a light theme.
//!
//! Nothing polls: the theme is read at launch, when Hyprland reloads its config (which
//! `omarchy-theme-set` does on every switch, see `hyprland.rs`) and when a window comes to
//! the front. Reads run off the UI thread.

use std::path::{Path, PathBuf};

use gpui_kit::{App, Global, Hsla, rgb};

use crate::theme::Palette;

/// The colors of an Omarchy theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmarchyColors {
    /// Display name ("Tokyo Night").
    pub name: String,
    /// The theme says it is light (`light.mode`, or a light background).
    pub light: bool,
    pub background: u32,
    pub foreground: u32,
    /// The theme's accent, when it names one.
    pub accent: Option<u32>,
    /// Terminal colors 0–15.
    pub ansi: [u32; 16],
    /// The window shades Omarchy 4 themes name; older themes leave them out and the palette
    /// derives them.
    pub shades: Shades,
}

/// Omarchy 4's extra `colors.toml` keys, which the palette uses as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shades {
    pub selection: Option<u32>,
    pub dark_background: Option<u32>,
    pub darker_background: Option<u32>,
    pub lighter_background: Option<u32>,
    pub bright_foreground: Option<u32>,
}

/// The last Omarchy theme read, as a GPUI global.
#[derive(Clone, Debug, Default)]
pub struct OmarchyTheme {
    pub colors: Option<OmarchyColors>,
    pub palette: Option<Palette>,
}

impl Global for OmarchyTheme {}

/// The palette built from the active Omarchy theme, once it has been read.
pub fn palette(cx: &App) -> Option<Palette> {
    cx.try_global::<OmarchyTheme>().and_then(|t| t.palette)
}

/// Stores a freshly read theme. Returns whether it changed.
pub fn set(colors: Option<OmarchyColors>, cx: &mut App) -> bool {
    if cx
        .try_global::<OmarchyTheme>()
        .is_some_and(|t| t.colors == colors)
    {
        return false;
    }
    let palette = colors.as_ref().map(build_palette);
    cx.set_global(OmarchyTheme { colors, palette });
    true
}

/// Re-reads the theme off the UI thread and re-applies the palette when it changed.
pub fn reload(cx: &mut App) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let read = cx.background_executor().spawn(async { load() });
    cx.spawn(async move |cx| {
        let colors = read.await;
        cx.update(|cx| {
            if set(colors, cx) {
                crate::theme::refresh(cx);
            }
        });
    })
    .detach();
}

/// Where the active theme lives: Omarchy 4 first, then Omarchy 3.
fn theme_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        ".local/state/omarchy/current/theme",
        ".config/omarchy/current/theme",
    ]
    .into_iter()
    .map(|d| home.join(d))
    .collect()
}

/// Reads the active Omarchy theme, `None` when this is not Omarchy. Disk I/O: keep it off
/// the UI thread.
pub fn load() -> Option<OmarchyColors> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    theme_dirs().iter().find_map(|d| read_theme(d))
}

fn read_theme(dir: &Path) -> Option<OmarchyColors> {
    let colors = std::fs::read_to_string(dir.join("colors.toml"))
        .ok()
        .and_then(|s| from_colors_toml(&s))
        .or_else(|| {
            std::fs::read_to_string(dir.join("alacritty.toml"))
                .ok()
                .and_then(|s| from_alacritty(&s))
        })?;
    // Newer Omarchy writes the name next to the theme; older ones symlink the directory.
    let slug = dir
        .parent()
        .and_then(|p| std::fs::read_to_string(p.join("theme.name")).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::canonicalize(dir)
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .unwrap_or_default();
    let light = colors.light || dir.join("light.mode").exists() || lum(colors.background) > 0.4;
    Some(OmarchyColors {
        name: display_name(&slug),
        light,
        ..colors
    })
}

/// `tokyo-night` → `Tokyo Night`.
fn display_name(slug: &str) -> String {
    slug.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().chain(ch).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `key = value` pairs of a TOML file, keys prefixed with their `[table]` (`colors.primary.background`).
/// Enough for theme files: no arrays, inline tables or multi-line strings.
fn toml_pairs(src: &str) -> Vec<(String, String)> {
    let mut table = String::new();
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            table = rest
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim()
                .replace(['"', '\''], "");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().replace(['"', '\''], "");
        let value = value.trim();
        let value = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split(['#', ' ', '\t']).next().unwrap_or(""),
        };
        let full = if table.is_empty() {
            key
        } else {
            format!("{table}.{key}")
        };
        out.push((full, value.to_owned()));
    }
    out
}

/// `#rrggbb`, `0xrrggbb` or `rrggbb` (an alpha byte after it is ignored).
fn parse_hex(v: &str) -> Option<u32> {
    let v = v.trim();
    let v = v
        .strip_prefix('#')
        .or_else(|| v.strip_prefix("0x"))
        .or_else(|| v.strip_prefix("0X"))
        .unwrap_or(v);
    if !(v.len() == 6 || v.len() == 8) || !v.is_ascii() {
        return None;
    }
    u32::from_str_radix(&v[..6], 16).ok()
}

fn lookup(pairs: &[(String, String)], key: &str) -> Option<u32> {
    pairs
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| parse_hex(v))
}

/// Terminal colors for a theme that leaves some out (Tokyo Night's).
const FALLBACK_ANSI: [u32; 16] = [
    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868,
    0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
];

/// Fill missing terminal colors: bright from normal, normal from [`FALLBACK_ANSI`].
fn complete_ansi(found: [Option<u32>; 16]) -> [u32; 16] {
    let fallback = FALLBACK_ANSI;
    let mut out = [0; 16];
    for i in 0..16 {
        out[i] = found[i]
            .or(if i >= 8 { found[i - 8] } else { None })
            .unwrap_or(fallback[i]);
    }
    out
}

/// Omarchy 4's names for terminal colors 0–15, each with fallbacks for a key a theme leaves
/// out (black and white are often missing; their nearest named shades stand in).
const NAMED_ANSI: [&[&str]; 16] = [
    &["black", "lighter_background"],
    &["red"],
    &["green"],
    &["yellow"],
    &["blue"],
    &["magenta"],
    &["cyan"],
    &["white", "dark_foreground"],
    &["bright_black", "muted"],
    &["bright_red"],
    &["bright_green"],
    &["bright_yellow"],
    &["bright_blue"],
    &["bright_magenta"],
    &["bright_cyan"],
    &["bright_white", "bright_foreground"],
];

/// Omarchy's `colors.toml`: `color0`..`color15` (Omarchy 3) or named colors (Omarchy 4).
fn from_colors_toml(src: &str) -> Option<OmarchyColors> {
    let pairs = toml_pairs(src);
    let mut ansi = [None; 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        *slot = lookup(&pairs, &format!("color{i}"))
            .or_else(|| NAMED_ANSI[i].iter().find_map(|k| lookup(&pairs, k)));
    }
    let light = pairs
        .iter()
        .rev()
        .find(|(k, _)| k == "mode")
        .is_some_and(|(_, v)| v.eq_ignore_ascii_case("light"));
    Some(OmarchyColors {
        name: String::new(),
        light,
        background: lookup(&pairs, "background")?,
        foreground: lookup(&pairs, "foreground")?,
        accent: lookup(&pairs, "accent"),
        ansi: complete_ansi(ansi),
        shades: Shades {
            selection: lookup(&pairs, "selection")
                .or_else(|| lookup(&pairs, "selection_background")),
            dark_background: lookup(&pairs, "dark_background"),
            darker_background: lookup(&pairs, "darker_background"),
            lighter_background: lookup(&pairs, "lighter_background"),
            bright_foreground: lookup(&pairs, "bright_foreground"),
        },
    })
}

/// A theme's `alacritty.toml` (`[colors.primary]`, `[colors.normal]`, `[colors.bright]`).
fn from_alacritty(src: &str) -> Option<OmarchyColors> {
    const NAMES: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let pairs = toml_pairs(src);
    let mut ansi = [None; 16];
    for (i, n) in NAMES.iter().enumerate() {
        ansi[i] = lookup(&pairs, &format!("colors.normal.{n}"));
        ansi[i + 8] = lookup(&pairs, &format!("colors.bright.{n}"));
    }
    Some(OmarchyColors {
        name: String::new(),
        light: false,
        background: lookup(&pairs, "colors.primary.background")?,
        foreground: lookup(&pairs, "colors.primary.foreground")?,
        accent: None,
        ansi: complete_ansi(ansi),
        shades: Shades::default(),
    })
}

fn channels(c: u32) -> [f32; 3] {
    [(c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff].map(|v| v as f32)
}

/// `a` moved towards `b` by `t` (0–1).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let (x, y) = (channels(a), channels(b));
    let ch = |i: usize| (x[i] + (y[i] - x[i]) * t).round().clamp(0.0, 255.0) as u32;
    ch(0) << 16 | ch(1) << 8 | ch(2)
}

fn lum(c: u32) -> f32 {
    let lin = |v: f32| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = channels(c);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn ratio(a: u32, b: u32) -> f32 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `color`, moved towards `toward` until it reads at `min`:1 on `bg` (or as far as it goes).
fn readable(color: u32, bg: u32, toward: u32, min: f32) -> u32 {
    (0..=10)
        .map(|i| mix(color, toward, i as f32 / 10.0))
        .find(|c| ratio(*c, bg) >= min)
        .unwrap_or(toward)
}

fn hsla(c: u32) -> Hsla {
    rgb(c).into()
}

/// The Snapwright palette for an Omarchy theme, as `docs/design/Omarchy Themes.dc.html`
/// maps it (and `site/site.js` for the site): the theme's background is the window, its
/// `dark_background` the sidebar and `darker_background` the canvas, its brightest
/// foreground the text, its accent (blue when it names none) the accent, its red the danger
/// color. Shades a theme leaves out are mixed from its background and foreground.
pub fn build_palette(c: &OmarchyColors) -> Palette {
    let dark = !c.light;
    let (bg, a, sh) = (c.background, c.ansi, c.shades);
    // Some generated themes (Aether) name a pale `bright_foreground` on a light background;
    // the text takes whichever foreground reads better.
    let ink = sh
        .bright_foreground
        .filter(|b| ratio(*b, bg) >= ratio(c.foreground, bg))
        .unwrap_or(c.foreground);
    let toward_ink = |t: f32| mix(bg, ink, t);
    let shade = |t: f32| {
        if dark {
            mix(bg, 0, t)
        } else {
            mix(bg, ink, t * 0.3)
        }
    };
    let panel = sh.dark_background.unwrap_or_else(|| shade(0.25));
    let canvas = sh.darker_background.unwrap_or_else(|| shade(0.45));
    let lighter = sh.lighter_background.unwrap_or_else(|| toward_ink(0.08));
    let acc = c.accent.unwrap_or(a[4]);
    let acc_fg = if lum(acc) > 0.4 { canvas } else { 0xffffff };
    // Kept readable for themes whose foreground sits close to their background.
    let text = |color: u32, min: f32| readable(color, bg, ink, min);
    Palette {
        dark,
        bg: hsla(bg),
        panel: hsla(panel),
        canvas: hsla(canvas),
        fg: hsla(ink),
        fg2: hsla(text(mix(ink, bg, 0.3), 4.5)),
        fg3: hsla(text(mix(ink, bg, 0.5), 2.5)),
        line: hsla(toward_ink(0.1)),
        line2: hsla(toward_ink(0.18)),
        hover: hsla(toward_ink(0.06)),
        sel: hsla(sh.selection.unwrap_or_else(|| toward_ink(0.12))),
        accent: hsla(acc),
        accent_fg: hsla(acc_fg),
        accent_soft: Hsla {
            a: 0.22,
            ..hsla(acc)
        },
        thumb: hsla(if dark { lighter } else { panel }),
        danger: hsla(a[1]),
        warning: hsla(a[3]),
        success: hsla(a[2]),
    }
}

/// An Omarchy theme bundled with the app, so any desktop can use it. Colors from each
/// theme's `colors.toml` in basecamp/omarchy, as the design handoff lists them.
pub struct Bundled {
    pub id: &'static str,
    pub name: &'static str,
    light: bool,
    accent: u32,
    selection: u32,
    background: u32,
    dark_background: u32,
    darker_background: u32,
    lighter_background: u32,
    foreground: u32,
    bright_foreground: u32,
    red: u32,
    yellow: u32,
    green: u32,
}

impl Bundled {
    pub fn colors(&self) -> OmarchyColors {
        let mut ansi = [None; 16];
        ansi[1] = Some(self.red);
        ansi[2] = Some(self.green);
        ansi[3] = Some(self.yellow);
        ansi[4] = Some(self.accent);
        OmarchyColors {
            name: self.name.to_owned(),
            light: self.light,
            background: self.background,
            foreground: self.foreground,
            accent: Some(self.accent),
            ansi: complete_ansi(ansi),
            shades: Shades {
                selection: Some(self.selection),
                dark_background: Some(self.dark_background),
                darker_background: Some(self.darker_background),
                lighter_background: Some(self.lighter_background),
                bright_foreground: Some(self.bright_foreground),
            },
        }
    }

    pub fn palette(&self) -> Palette {
        build_palette(&self.colors())
    }
}

/// Finds a bundled theme by id (`tokyo-night`).
pub fn bundled(id: &str) -> Option<&'static Bundled> {
    BUNDLED.iter().find(|t| t.id == id)
}

#[rustfmt::skip]
macro_rules! theme {
    (@light light) => { true };
    (@light dark) => { false };
    ($id:literal, $name:literal, $mode:ident, $accent:literal, $selection:literal, $bg:literal,
     $dark:literal, $darker:literal, $lighter:literal, $fg:literal, $bright:literal,
     $red:literal, $yellow:literal, $green:literal) => {
        Bundled {
            id: $id, name: $name, light: theme!(@light $mode),
            accent: $accent, selection: $selection, background: $bg, dark_background: $dark,
            darker_background: $darker, lighter_background: $lighter, foreground: $fg,
            bright_foreground: $bright, red: $red, yellow: $yellow, green: $green,
        }
    };
}

/// The same list, in the same order, as `site/site.js`.
#[rustfmt::skip]
pub const BUNDLED: [Bundled; 16] = [
    theme!("tokyo-night", "Tokyo Night", dark, 0x7aa2f7, 0x292e42, 0x1a1b26, 0x13141c, 0x0e0e14, 0x24283b, 0xa9b1d6, 0xc0caf5, 0xf7768e, 0xe0af68, 0x9ece6a),
    theme!("catppuccin", "Catppuccin", dark, 0x89b4fa, 0x45475a, 0x1e1e2e, 0x161622, 0x101019, 0x313244, 0xcdd6f4, 0xcdd6f4, 0xf38ba8, 0xf9e2af, 0xa6e3a1),
    theme!("catppuccin-latte", "Catppuccin Latte", light, 0x1e66f5, 0xccd0da, 0xeff1f5, 0xe3e4e8, 0xd7d8dc, 0xdce0e8, 0x4c4f69, 0x4c4f69, 0xd20f39, 0xdf8e1d, 0x40a02b),
    theme!("ethereal", "Ethereal", dark, 0x7d82d9, 0x252e56, 0x060b1e, 0x040816, 0x030610, 0x131a3a, 0xffcead, 0xffcead, 0xed5b5a, 0xe9bb4f, 0x92a593),
    theme!("everforest", "Everforest", dark, 0x7fbbb3, 0x3d484d, 0x2d353b, 0x21272c, 0x181d20, 0x343f44, 0xd3c6aa, 0xd3c6aa, 0xe67e80, 0xdbbc7f, 0xa7c080),
    theme!("flexoki-light", "Flexoki Light", light, 0x205ea6, 0xcecdc3, 0xfffcf0, 0xf2efe4, 0xe5e2d8, 0xe6e4d9, 0x100f0f, 0x100f0f, 0xd14d41, 0xd0a215, 0x879a39),
    theme!("gruvbox", "Gruvbox", dark, 0x7daea3, 0x504945, 0x282828, 0x1e1e1e, 0x161616, 0x3c3836, 0xd4be98, 0xd4be98, 0xea6962, 0xd8a657, 0xa9b665),
    theme!("hackerman", "Hackerman", dark, 0x82fb9c, 0x1f253a, 0x0b0c16, 0x080910, 0x06060c, 0x151828, 0xddf7ff, 0xddf7ff, 0x50f872, 0x50f7d4, 0x4fe88f),
    theme!("kanagawa", "Kanagawa", dark, 0xdcd7ba, 0x363646, 0x1f1f28, 0x17171e, 0x111116, 0x223249, 0xdcd7ba, 0xdcd7ba, 0xc34043, 0xc0a36e, 0x76946a),
    theme!("lumon", "Lumon", dark, 0x8bc9eb, 0x243d56, 0x16242d, 0x101b21, 0x0b1216, 0x1b2d40, 0xd6e2ee, 0xf2fcff, 0x4d86b0, 0x6fa4c9, 0x5e95bc),
    theme!("matte-black", "Matte Black", dark, 0xe68e0d, 0x2a2a2a, 0x121212, 0x0d0d0d, 0x090909, 0x1e1e1e, 0xbebebe, 0xbebebe, 0xd35f5f, 0xb91c1c, 0xffc107),
    theme!("nord", "Nord", dark, 0x81a1c1, 0x434c5e, 0x2e3440, 0x222730, 0x191c23, 0x3b4252, 0xd8dee9, 0xd8dee9, 0xbf616a, 0xebcb8b, 0xa3be8c),
    theme!("osaka-jade", "Osaka Jade", dark, 0x509475, 0x32473b, 0x111c18, 0x0c1512, 0x090f0d, 0x23372b, 0xc1c497, 0xf7e8b2, 0xff5345, 0x459451, 0x549e6a),
    theme!("ristretto", "Ristretto", dark, 0xf38d70, 0x403e41, 0x2c2525, 0x211b1b, 0x181414, 0x3d2f2a, 0xe6d9db, 0xe6d9db, 0xfd6883, 0xf9cc6c, 0xadda78),
    theme!("rose-pine", "Rosé Pine", light, 0x56949f, 0xdfdad9, 0xfaf4ed, 0xede7e1, 0xe1dbd5, 0xf2e9e1, 0x575279, 0x575279, 0xb4637a, 0xea9d34, 0x286983),
    theme!("white", "White", light, 0x6e6e6e, 0xc0c0c0, 0xffffff, 0xf5f5f5, 0xe8e8e8, 0xc0c0c0, 0x000000, 0x000000, 0x2a2a2a, 0x4a4a4a, 0x3a3a3a),
];

#[cfg(test)]
mod tests {
    use super::*;

    const COLORS_TOML: &str = r##"
accent = "#7aa2f7"
cursor = "#c0caf5"
foreground = "#a9b1d6"
background = "#1a1b26"
selection_foreground = "#c0caf5"
selection_background = "#7aa2f7"

color0 = "#32344a"
color1 = "#f7768e"
color2 = "#9ece6a"
color3 = "#e0af68"
color4 = "#7aa2f7"
color5 = "#ad8ee6"
color6 = "#449dab"
color7 = "#787c99"
color8 = "#444b6a"
color9 = "#ff7a93"
color10 = "#b9f27c"
color11 = "#ff9e64"
color12 = "#7da6ff"
color13 = "#bb9af7"
color14 = "#0db9d7"
color15 = "#acb0d0"
"##;

    /// Omarchy 4 (Aether): named colors, no black/white, a `mode`.
    const COLORS_TOML_V4: &str = r##"
# Generated by Aether for Omarchy v4.
mode = "light"

accent = "#b86a69"
muted = "#726c68"
background = "#f4ecd8"
lighter_background = "#35261f"
foreground = "#1E0E06"
dark_foreground = "#bbaa8a"
bright_foreground = "#fbe9ca"

red = "#c58964"
yellow = "#ffe78e"
green = "#ebbe70"
cyan = "#fbdc67"
blue = "#b86a69"
magenta = "#f19379"

bright_red = "#e69a6c"
bright_blue = "#d87878"
"##;

    const ALACRITTY: &str = r##"
[colors]
[colors.primary]
background = '#eff1f5' # base
foreground = "0x4c4f69"

[colors.normal]
black = "#5c5f77"
red = "#d20f39"
green = "#40a02b"
yellow = "#df8e1d"
blue = "#1e66f5"
magenta = "#ea76cb"
cyan = "#179299"
white = "#acb0be"

[colors.bright]
black = "#6c6f85"
red = "#d20f39"
"##;

    #[test]
    fn reads_colors_toml() {
        let c = from_colors_toml(COLORS_TOML).expect("parsed");
        assert_eq!(c.background, 0x1a1b26);
        assert_eq!(c.foreground, 0xa9b1d6);
        assert_eq!(c.accent, Some(0x7aa2f7));
        assert_eq!(c.ansi[1], 0xf7768e);
        assert_eq!(c.ansi[15], 0xacb0d0);
    }

    #[test]
    fn reads_omarchy_4_named_colors() {
        let c = from_colors_toml(COLORS_TOML_V4).expect("parsed");
        assert!(c.light);
        assert_eq!(c.background, 0xf4ecd8);
        assert_eq!(c.accent, Some(0xb86a69));
        assert_eq!(c.ansi[0], 0x35261f);
        assert_eq!(c.ansi[1], 0xc58964);
        assert_eq!(c.ansi[4], 0xb86a69);
        assert_eq!(c.ansi[7], 0xbbaa8a);
        assert_eq!(c.ansi[8], 0x726c68);
        assert_eq!(c.ansi[12], 0xd87878);
        // No bright green: the normal one.
        assert_eq!(c.ansi[10], 0xebbe70);
        assert_eq!(c.ansi[15], 0xfbe9ca);
        assert!(
            !from_colors_toml(
                "mode = \"dark\"\nbackground = \"#000000\"\nforeground = \"#ffffff\""
            )
            .expect("parsed")
            .light
        );
    }

    #[test]
    fn reads_alacritty_and_fills_missing_bright_colors() {
        let c = from_alacritty(ALACRITTY).expect("parsed");
        assert_eq!(c.background, 0xeff1f5);
        assert_eq!(c.foreground, 0x4c4f69);
        assert_eq!(c.accent, None);
        assert_eq!(c.ansi[4], 0x1e66f5);
        assert_eq!(c.ansi[8], 0x6c6f85);
        // No bright green: the normal one.
        assert_eq!(c.ansi[10], 0x40a02b);
    }

    #[test]
    fn missing_background_is_not_a_theme() {
        assert_eq!(from_colors_toml("foreground = \"#ffffff\""), None);
        assert_eq!(
            from_alacritty("[colors.primary]\nbackground = \"nope\""),
            None
        );
    }

    #[test]
    fn reads_a_theme_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let themes = root.path().join("themes/catppuccin-latte");
        std::fs::create_dir_all(&themes).expect("mkdir");
        std::fs::write(themes.join("alacritty.toml"), ALACRITTY).expect("write");
        std::fs::write(themes.join("light.mode"), "").expect("write");
        let current = root.path().join("current");
        std::fs::create_dir_all(&current).expect("mkdir");
        #[cfg(unix)]
        let link = {
            let link = current.join("theme");
            std::os::unix::fs::symlink(&themes, &link).expect("symlink");
            link
        };
        #[cfg(not(unix))]
        let link = themes.clone();
        let c = read_theme(&link).expect("read");
        assert_eq!(c.name, "Catppuccin Latte");
        assert!(c.light);
        assert!(read_theme(&root.path().join("missing")).is_none());
    }

    fn hex(h: Hsla) -> u32 {
        let r = gpui_kit::Rgba::from(h);
        let b = |v: f32| (v * 255.0).round() as u32;
        b(r.r) << 16 | b(r.g) << 8 | b(r.b)
    }

    #[test]
    fn palettes_stay_readable() {
        let parsed = [
            from_colors_toml(COLORS_TOML),
            from_colors_toml(COLORS_TOML_V4),
            from_alacritty(ALACRITTY).map(|c| OmarchyColors { light: true, ..c }),
        ]
        .map(|c| c.expect("parsed"));
        for c in parsed
            .into_iter()
            .chain(BUNDLED.iter().map(Bundled::colors))
        {
            let p = build_palette(&c);
            assert_eq!(p.dark, !c.light, "{}", c.name);
            let s = c.background;
            assert_eq!(hex(p.bg), s);
            assert!(ratio(hex(p.fg), s) >= 4.5, "{}", c.name);
            assert!(ratio(hex(p.fg2), s) >= 4.5, "{}", c.name);
            assert!(ratio(hex(p.fg3), s) >= 2.5, "{}", c.name);
        }
    }

    #[test]
    fn bundled_themes_use_their_named_shades() {
        let t = bundled("tokyo-night").expect("bundled");
        let p = t.palette();
        assert_eq!(hex(p.panel), 0x13141c);
        assert_eq!(hex(p.canvas), 0x0e0e14);
        assert_eq!(hex(p.fg), 0xc0caf5);
        assert_eq!(hex(p.sel), 0x292e42);
        assert_eq!(hex(p.thumb), 0x24283b);
        assert_eq!(hex(p.accent), 0x7aa2f7);
        assert_eq!(hex(p.accent_fg), 0xffffff);
        // A light accent takes the darkest shade for its text.
        let hackerman = bundled("hackerman").expect("bundled").palette();
        assert_eq!(hex(hackerman.accent_fg), 0x06060c);
        assert_eq!(
            hex(bundled("nord").expect("bundled").palette().accent_fg),
            0xffffff
        );
        let latte = bundled("catppuccin-latte").expect("bundled").palette();
        assert!(!latte.dark);
        assert_eq!(hex(latte.thumb), 0xe3e4e8);
        assert_eq!(BUNDLED.len(), 16);
    }

    #[test]
    fn names_from_slugs() {
        assert_eq!(display_name("tokyo-night"), "Tokyo Night");
        assert_eq!(display_name("rose_pine"), "Rose Pine");
        assert_eq!(display_name(""), "");
    }
}
