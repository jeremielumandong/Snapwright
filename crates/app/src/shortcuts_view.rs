//! The Shortcuts window (tray > Keyboard shortcuts…): each action's shortcut, a Change
//! button that listens for the next key press, Clear, and Reset to defaults. Changes are
//! saved and bound at once (`shortcuts::set`).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, FocusHandle, FontWeight, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, Styled as _,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};

use crate::hotkey::Action;
use crate::shortcuts::{self, ALT, CTRL, SHIFT, SUPER, Shortcut, Shortcuts};
use crate::theme::{self, Palette};
use crate::tray;

/// The open Shortcuts window, if any.
#[derive(Default)]
struct ShortcutsWindow(Option<AnyWindowHandle>);
impl Global for ShortcutsWindow {}

/// Opens the Shortcuts window, or brings it to the front.
pub fn open(cx: &mut App) {
    if let Some(h) = cx.default_global::<ShortcutsWindow>().0
        && h.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(560.), px(380.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(480.), px(340.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Snapwright Shortcuts".into()),
            ..Default::default()
        }),
        app_id: Some("snapwright".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| ShortcutsView::new(window, cx))
    }) {
        Ok((handle, _)) => cx.set_global(ShortcutsWindow(Some(handle))),
        Err(e) => tracing::error!(error = %e, "could not open the Shortcuts window"),
    }
}

struct ShortcutsView {
    focus: FocusHandle,
    /// The action waiting for its new shortcut.
    listening: Option<Action>,
    /// Why the last press was not taken.
    hint: Option<&'static str>,
    _subs: Vec<Subscription>,
}

impl ShortcutsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subs = vec![
            // Closing while listening puts the binds back.
            cx.on_release(|_, cx| shortcuts::resume(cx)),
            cx.observe_window_appearance(window, |_, window, cx| theme::sync(window, cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                theme::activated(window, cx);
                if !window.is_window_active() {
                    this.stop_listening(cx);
                }
            }),
        ];
        Self {
            focus,
            listening: None,
            hint: None,
            _subs: subs,
        }
    }

    fn listen(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.listening = Some(action);
        self.hint = None;
        window.focus(&self.focus, cx);
        // Lift the binds, so pressing the current shortcut records it rather than firing.
        shortcuts::suspend(cx);
        cx.notify();
    }

    fn stop_listening(&mut self, cx: &mut Context<Self>) {
        if self.listening.take().is_some() {
            self.hint = None;
            shortcuts::resume(cx);
            cx.notify();
        }
    }

    fn change(&mut self, action: Action, shortcut: Option<Shortcut>, cx: &mut Context<Self>) {
        let mut all = shortcuts::current(cx);
        all.set(action, shortcut);
        self.listening = None;
        self.hint = None;
        shortcuts::set(all, cx);
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.listening else {
            return;
        };
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" => return self.stop_listening(cx),
            "backspace" | "delete" => return self.change(action, None, cx),
            _ => {}
        }
        let m = k.modifiers;
        let mods = [
            (m.control, CTRL),
            (m.alt, ALT),
            (m.shift, SHIFT),
            (m.platform, SUPER),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |acc, (_, bit)| acc | bit);
        match Shortcut::from_keystroke(mods, &k.key) {
            Some(shortcut) => self.change(action, Some(shortcut), cx),
            // A modifier on its own: wait for the key.
            None if is_modifier(&k.key) => {}
            None => {
                self.hint = Some(if k.key.chars().count() == 1 {
                    "Add Ctrl, Alt or Super to a letter or digit"
                } else {
                    "Use a letter, a digit, an F key or Print"
                });
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn render_row(
        &self,
        action: Action,
        shortcut: Option<Shortcut>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let listening = self.listening == Some(action);
        let bound = cx
            .try_global::<tray::Labels>()
            .is_some_and(|l| l.get(action).is_some());
        let keys = if listening {
            div()
                .text_color(p.accent)
                .child("Press a shortcut")
                .into_any_element()
        } else if let Some(s) = shortcut {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keycaps(&s).into_iter().map(|k| theme::keycap(k, p)))
                .into_any_element()
        } else {
            div().text_color(p.fg3).child("Off").into_any_element()
        };
        // Bound elsewhere: another app holds the hotkey, or the user's Hyprland bind.
        let taken = shortcut.is_some() && !bound && !listening && shortcuts::supported(cx);
        let id = shortcuts::setting_name(action);
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(shortcuts::title(action))
                    .when(taken, |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(p.danger)
                                .child("In use by another app or a compositor bind"),
                        )
                    }),
            )
            .child(div().w(px(160.)).flex().justify_end().child(keys))
            .child(
                Button::new(format!("change-{id}"))
                    .outline()
                    .small()
                    .label(if listening { "Cancel" } else { "Change" })
                    .disabled(!shortcuts::supported(cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.listening == Some(action) {
                            this.stop_listening(cx);
                        } else {
                            this.listen(action, window, cx);
                        }
                    })),
            )
            .child(
                Button::new(format!("clear-{id}"))
                    .ghost()
                    .small()
                    .label("Clear")
                    .disabled(shortcut.is_none() || !shortcuts::supported(cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.change(action, None, cx))),
            )
    }
}

impl Render for ShortcutsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let current = shortcuts::current(cx);
        let note = if !shortcuts::supported(cx) {
            "This compositor gives apps no global shortcuts. Bind snapwright-app --capture-now \
             (or --repeat-last, --record, --record-gif) to keys in its settings."
        } else if shortcuts::in_hyprland(cx) {
            "Bound in Hyprland, so they work in every app."
        } else {
            "They work in every app while Snapwright runs."
        };
        let footer = match (self.listening, self.hint) {
            (Some(_), Some(hint)) => hint,
            (Some(_), None) => "Esc cancels. Backspace turns the shortcut off.",
            (None, _) => "",
        };
        let rows: Vec<_> = current
            .iter()
            .map(|(action, s)| self.render_row(action, s, &p, cx).into_any_element())
            .collect();
        div()
            .id("shortcuts")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.fg)
            .text_size(px(13.))
            .p(px(20.))
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Keyboard shortcuts"),
            )
            .child(div().text_color(p.fg2).pb(px(6.)).child(note))
            .children(rows)
            .child(
                div()
                    .pt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(p.fg3)
                            .child(footer),
                    )
                    .child(
                        Button::new("reset")
                            .outline()
                            .small()
                            .label("Reset to defaults")
                            .disabled(current == Shortcuts::default() || !shortcuts::supported(cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.listening = None;
                                this.hint = None;
                                shortcuts::set(Shortcuts::default(), cx);
                                cx.notify();
                            })),
                    ),
            )
    }
}

/// The keys of a modifier on its own, as GPUI names them on each platform.
fn is_modifier(key: &str) -> bool {
    [
        "shift", "control", "ctrl", "alt", "super", "meta", "cmd", "platform", "fn",
    ]
    .iter()
    .any(|m| key.contains(m))
}

/// A shortcut's keycaps, with this platform's symbols.
fn keycaps(s: &Shortcut) -> Vec<String> {
    crate::library_view::hotkey_keys(&s.label())
}
