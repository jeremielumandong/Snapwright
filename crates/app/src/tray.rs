//! The tray (menu bar) icon: Capture, Open captures folder, Quit.
//!
//! macOS and Windows use the native status item; Linux uses StatusNotifierItem over D-Bus
//! (`tray-icon`'s `ksni` backend), so no GTK is needed. Menu clicks arrive on a platform
//! thread and are forwarded to the UI thread over a channel.

use anyhow::Result;
use futures::channel::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// What the user picked from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Capture,
    OpenFolder,
    Quit,
}

/// Keeps the tray icon alive for the life of the app.
pub struct Tray {
    _icon: TrayIcon,
}

impl gpui_kit::Global for Tray {}

const ID_CAPTURE: &str = "capture";
const ID_OPEN: &str = "open-folder";
const ID_QUIT: &str = "quit";

/// Creates the tray icon. Call on the main thread once the app is running.
pub fn create(hotkey_label: Option<&str>) -> Result<(Tray, mpsc::UnboundedReceiver<Action>)> {
    let capture_label = match hotkey_label {
        Some(k) => format!("Capture region\t{k}"),
        None => "Capture region".to_owned(),
    };
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(ID_CAPTURE, capture_label, true, None),
        &MenuItem::with_id(ID_OPEN, "Open captures folder", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_QUIT, "Quit Snapwright", true, None),
    ])?;

    let (tx, rx) = mpsc::unbounded();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(action) = action_for(&event.id) {
            let _ = tx.unbounded_send(action);
        }
    }));

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Snapwright");
    // macOS tints a black template image to match the menu bar; other trays show the colour.
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(Icon::from_rgba(
        icon_rgba(ICON_SIZE, TEMPLATE),
        ICON_SIZE,
        ICON_SIZE,
    )?);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(Icon::from_rgba(
        icon_rgba(ICON_SIZE, ACCENT),
        ICON_SIZE,
        ICON_SIZE,
    )?);
    let icon = builder.build()?;
    Ok((Tray { _icon: icon }, rx))
}

fn action_for(id: &MenuId) -> Option<Action> {
    match id.as_ref() {
        ID_CAPTURE => Some(Action::Capture),
        ID_OPEN => Some(Action::OpenFolder),
        ID_QUIT => Some(Action::Quit),
        _ => None,
    }
}

const ICON_SIZE: u32 = 32;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
const TEMPLATE: [u8; 4] = [0, 0, 0, 255];
/// Readable on both light and dark taskbars.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const ACCENT: [u8; 4] = [0x2f, 0x80, 0xed, 255];

/// Draws the tray glyph in `colour`: four viewfinder corners around a dot, on transparent.
fn icon_rgba(size: u32, colour: [u8; 4]) -> Vec<u8> {
    let s = size as i32;
    let edge = s / 8; // margin
    let arm = s * 5 / 16; // length of each corner arm
    let thick = (s / 12).max(2);
    let far = s - 1 - edge;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let near_x = x - edge;
            let near_y = y - edge;
            let from_far_x = far - x;
            let from_far_y = far - y;
            let in_box = near_x >= 0 && near_y >= 0 && from_far_x >= 0 && from_far_y >= 0;
            let on_vertical = near_x < thick || from_far_x < thick;
            let on_horizontal = near_y < thick || from_far_y < thick;
            let in_corner_x = near_x < arm || from_far_x < arm;
            let in_corner_y = near_y < arm || from_far_y < arm;
            let corner = in_box && in_corner_x && in_corner_y && (on_vertical || on_horizontal);
            let c = (s - 1) as f32 / 2.0;
            let r = s as f32 / 8.0;
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let dot = dx * dx + dy * dy <= r * r;
            if corner || dot {
                let i = ((y * s + x) * 4) as usize;
                out[i..i + 4].copy_from_slice(&colour);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(action_for(&MenuId::new(ID_CAPTURE)), Some(Action::Capture));
        assert_eq!(action_for(&MenuId::new(ID_OPEN)), Some(Action::OpenFolder));
        assert_eq!(action_for(&MenuId::new(ID_QUIT)), Some(Action::Quit));
        assert_eq!(action_for(&MenuId::new("other")), None);
    }

    #[test]
    fn icon_has_corners_and_dot_but_clear_edges() {
        let px = icon_rgba(ICON_SIZE, TEMPLATE);
        assert_eq!(px.len(), 32 * 32 * 4);
        let alpha = |x: u32, y: u32| px[((y * 32 + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "outer margin is transparent");
        assert_eq!(alpha(4, 4), 255, "top-left corner is drawn");
        assert_eq!(alpha(27, 27), 255, "bottom-right corner is drawn");
        assert_eq!(alpha(16, 4), 0, "top edge has a gap between corners");
        assert_eq!(alpha(16, 16), 255, "centre dot is drawn");
    }
}
