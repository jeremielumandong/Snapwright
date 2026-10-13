//! Design tokens from the UI refresh (`docs/design/`): a light and a dark palette for the
//! windows, one always-dark material for the capture HUDs, and the bridge into
//! gpui-component's theme so its inputs and buttons match. The app follows the system
//! appearance.

use std::borrow::Cow;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    App, Div, FontWeight, Global, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, WindowAppearance, div, px, rgb, rgba,
};

/// UI font, bundled (SIL OFL 1.1, see `assets/fonts/OFL-Geist.txt`).
pub const SANS: &str = "Geist";

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
];

/// Window colors. Names follow the CSS variables in the design.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub panel: Hsla,
    pub canvas: Hsla,
    pub fg: Hsla,
    pub fg2: Hsla,
    pub fg3: Hsla,
    pub line: Hsla,
    pub line2: Hsla,
    pub hover: Hsla,
    pub sel: Hsla,
    pub accent: Hsla,
    pub accent_fg: Hsla,
    pub accent_soft: Hsla,
    pub thumb: Hsla,
    pub danger: Hsla,
    /// The dot on the editor's "Unsaved" pill.
    pub warning: Hsla,
    /// The dot on the editor's "Saved" pill.
    pub success: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex_rgba: u32) -> Hsla {
    rgba(hex_rgba).into()
}

impl Palette {
    pub fn light() -> Self {
        Self {
            dark: false,
            bg: c(0xffffff),
            panel: c(0xf7f7f8),
            canvas: c(0xefeff1),
            fg: c(0x18181d),
            fg2: c(0x5d5d66),
            fg3: c(0x8b8b94),
            line: c(0xe7e7ea),
            line2: c(0xd6d6db),
            hover: c(0xf0f0f2),
            sel: c(0xe7e7eb),
            accent: c(0x2f6fd6),
            accent_fg: c(0xffffff),
            accent_soft: c(0xdce8fb),
            thumb: c(0xf4f4f6),
            danger: c(0xd63b40),
            warning: c(0xe5a10a),
            success: c(0x30a46c),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: c(0x18181d),
            panel: c(0x141418),
            canvas: c(0x0e0e11),
            fg: c(0xececef),
            fg2: c(0xa6a6b0),
            fg3: c(0x74747e),
            line: c(0x27272d),
            line2: c(0x36363e),
            hover: c(0x222228),
            sel: c(0x2a2a31),
            accent: c(0x4b8af0),
            accent_fg: c(0xffffff),
            accent_soft: ca(0x4b8af038),
            thumb: c(0x202026),
            danger: c(0xf0676b),
            warning: c(0xe5a10a),
            success: c(0x3dd68c),
        }
    }

    fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }
}

/// The capture HUDs (overlay, loupe, preview, countdown): always dark, since they sit over
/// arbitrary screen content.
pub mod hud {
    use super::{c, ca};
    use gpui_kit::Hsla;

    /// Panel fill.
    pub fn bg() -> Hsla {
        ca(0x18181df0)
    }
    /// Hairline border on panels.
    pub fn border() -> Hsla {
        ca(0xffffff1f)
    }
    pub fn fg() -> Hsla {
        c(0xffffff)
    }
    /// Body text on panels.
    pub fn fg2() -> Hsla {
        c(0xc9c9d0)
    }
    /// Secondary text (units, coordinates).
    pub fn fg3() -> Hsla {
        c(0x8b8b94)
    }
    /// Keycap fill and button fill.
    pub fn key() -> Hsla {
        ca(0xffffff1a)
    }
    pub fn key_border() -> Hsla {
        ca(0xffffff24)
    }
    /// The dimmed screen outside a selection.
    pub fn shade() -> Hsla {
        ca(0x0a0a0e80)
    }
    pub fn shadow() -> Hsla {
        ca(0x00000066)
    }
    pub fn accent() -> Hsla {
        c(0x4b8af0)
    }
    pub fn success() -> Hsla {
        c(0x3dd68c)
    }
    pub fn success_soft() -> Hsla {
        ca(0x30a46c33)
    }
    /// The recording dot.
    pub fn record() -> Hsla {
        c(0xf0524b)
    }
}

/// The active palette.
pub struct Active(pub Palette);
impl Global for Active {}

/// The active window palette.
pub fn palette(cx: &App) -> Palette {
    cx.try_global::<Active>()
        .map_or_else(Palette::light, |a| a.0)
}

/// Loads the bundled fonts and applies the palette for the system appearance.
pub fn init(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!(error = %e, "could not load the bundled fonts");
    }
    apply(cx.window_appearance(), cx);
}

/// Follows a window's appearance (the system light or dark setting) when it changes.
pub fn sync(window: &Window, cx: &mut App) {
    let dark = matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    if dark != palette(cx).dark {
        apply(window.appearance(), cx);
    }
}

/// Stores the palette for `appearance` and restyles gpui-component to match.
fn apply(appearance: WindowAppearance, cx: &mut App) {
    let p = Palette::for_appearance(appearance);
    cx.set_global(Active(p));
    Theme::change(
        if p.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    Theme::update(cx, |t| {
        t.font_family = SharedString::from(SANS);
        t.font_size = px(13.);
        t.radius = px(8.);
        t.radius_lg = px(12.);
        t.shadow = false;
        t.background = p.bg;
        t.foreground = p.fg;
        t.border = p.line;
        t.input = p.line2;
        t.ring = p.accent;
        t.caret = p.accent;
        t.selection = p.accent_soft;
        t.primary = p.accent;
        t.primary_hover = p.accent;
        t.primary_active = p.accent;
        t.primary_foreground = p.accent_fg;
        t.button_primary = p.accent;
        t.button_primary_hover = p.accent;
        t.button_primary_active = p.accent;
        t.button_primary_foreground = p.accent_fg;
        t.button = p.bg;
        t.button_hover = p.hover;
        t.button_active = p.sel;
        t.button_foreground = p.fg;
        t.button_danger = p.danger;
        t.button_danger_hover = p.danger;
        t.button_danger_active = p.danger;
        t.button_danger_foreground = p.accent_fg;
        t.secondary = p.bg;
        t.secondary_hover = p.hover;
        t.secondary_active = p.sel;
        t.secondary_foreground = p.fg;
        t.muted = p.canvas;
        t.muted_foreground = p.fg3;
        t.accent = p.hover;
        t.accent_foreground = p.fg;
        t.popover = p.bg;
        t.popover_foreground = p.fg;
        t.list_hover = p.hover;
        t.list_active = p.sel;
        t.list_active_border = p.accent;
        t.danger = p.danger;
        t.danger_foreground = p.accent_fg;
        t.sidebar = p.panel;
        t.sidebar_foreground = p.fg;
        t.sidebar_border = p.line;
        t.sidebar_accent = p.sel;
        t.sidebar_accent_foreground = p.fg;
        t.title_bar = p.panel;
        t.title_bar_border = p.line;
    });
    cx.refresh_windows();
}

/// Key names for this platform, as keycaps show them.
pub fn key_label(key: &str) -> &str {
    match key {
        "secondary" if cfg!(target_os = "macos") => "⌘",
        "secondary" => "Ctrl",
        "alt" if cfg!(target_os = "macos") => "⌥",
        "alt" => "Alt",
        "shift" => "⇧",
        "enter" => "↵",
        // Geist has no ⌫ (nor ⌘ ⌥ ⌃); macOS falls back to the system font for them.
        "backspace" if cfg!(target_os = "macos") => "⌫",
        "backspace" => "Del",
        "left" => "←",
        "right" => "→",
        "up" => "↑",
        "down" => "↓",
        "escape" => "Esc",
        other => other,
    }
}

/// A keycap in a window: outlined, with a heavier bottom edge.
pub fn keycap(label: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .min_w(px(22.))
        .h(px(22.))
        .px(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_b_2()
        .border_color(p.line2)
        .text_size(px(11.))
        .text_color(p.fg2)
        .child(label.into())
}

/// A small inline key hint in a window (search field, footers).
pub fn key_hint(label: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .h(px(18.))
        .px(px(5.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(p.line2)
        .text_size(px(10.5))
        .text_color(p.fg3)
        .child(label.into())
}

/// A keycap on a HUD.
pub fn hud_keycap(label: impl Into<SharedString>) -> Div {
    div()
        .min_w(px(20.))
        .h(px(20.))
        .px(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .bg(hud::key())
        .border_1()
        .border_color(hud::key_border())
        .text_size(px(11.))
        .text_color(hud::fg())
        .child(label.into())
}

/// Keycaps followed by what they do, for a HUD hint bar.
pub fn hud_hint(keys: &[&str], label: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .children(keys.iter().map(|k| hud_keycap(key_label(k).to_owned())))
        .child(label)
}

/// Section label in a sidebar or inspector ("Tags", "Note").
pub fn section_label(text: &'static str, p: &Palette) -> Div {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.fg3)
        .child(text)
}
