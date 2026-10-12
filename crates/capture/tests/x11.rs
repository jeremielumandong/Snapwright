//! Captures a real X server. Run under Xvfb:
//! `xvfb-run -s "-screen 0 1280x720x24" cargo test -p snapwright-capture --test x11 -- --ignored`
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used)]

use snapwright_capture::{Capturer, Rect, window_at};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Paints the root window one known colour so the capture can be checked pixel for pixel.
fn paint_root(rgb: u32) {
    let (conn, screen) = x11rb::connect(None).expect("X server (run under xvfb-run)");
    let root = conn.setup().roots[screen].root;
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().background_pixel(rgb),
    )
    .expect("set background");
    conn.clear_area(false, root, 0, 0, 0, 0).expect("clear");
    // A round trip guarantees the server has repainted before we capture.
    conn.get_input_focus().expect("sync").reply().expect("sync");
}

/// Creates a mapped top-level window with a title and a pid.
fn open_window(conn: &RustConnection, root: u32, r: Rect, title: &str, pid: u32) -> u32 {
    let w = conn.generate_id().expect("id");
    conn.create_window(
        0,
        w,
        root,
        r.x as i16,
        r.y as i16,
        r.width as u16,
        r.height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().background_pixel(0x0033_3333),
    )
    .expect("create window");
    conn.change_property8(
        PropMode::REPLACE,
        w,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .expect("title");
    let pid_atom = conn
        .intern_atom(false, b"_NET_WM_PID")
        .expect("atom")
        .reply()
        .expect("atom")
        .atom;
    conn.change_property32(PropMode::REPLACE, w, pid_atom, AtomEnum::CARDINAL, &[pid])
        .expect("pid");
    conn.map_window(w).expect("map");
    w
}

// One test: the root window is shared state, so parallel tests would race on it.
#[test]
#[ignore = "needs an X server"]
fn captures_the_root_window_pixels_and_changes() {
    // Connect first: Xvfb resets the root window when its last client disconnects, which
    // would happen when `paint_root` closes its connection.
    let mut capturer = Capturer::new().expect("connect");
    paint_root(0x00ff_8040);
    let displays = capturer.displays().expect("displays");
    assert!(!displays.is_empty());
    for d in &displays {
        let frame = capturer.capture_display(d).expect("capture");
        assert_eq!(
            (frame.width, frame.height),
            (d.bounds.width, d.bounds.height)
        );
        // BGRA: 0x40 blue, 0x80 green, 0xff red, opaque.
        assert_eq!(frame.pixel(0, 0), Some([0x40, 0x80, 0xff, 0xff]));
        assert_eq!(
            frame.pixel(frame.width / 2, frame.height - 1),
            Some([0x40, 0x80, 0xff, 0xff])
        );
    }

    // Repeat captures through the same connection see the root window change.
    let d = &displays[0];
    paint_root(0x0000_00ff);
    assert_eq!(
        capturer.capture_display(d).expect("capture").pixel(5, 5),
        Some([0xff, 0, 0, 0xff])
    );
    paint_root(0x0000_ff00);
    assert_eq!(
        capturer.capture_display(d).expect("capture").pixel(5, 5),
        Some([0, 0xff, 0, 0xff])
    );

    // Windows come back front to back with their titles, pids and bounds; unmapped ones are
    // left out.
    let (conn, screen) = x11rb::connect(None).expect("X server");
    let root = conn.setup().roots[screen].root;
    let back = open_window(&conn, root, Rect::new(100, 100, 400, 300), "back", 4242);
    let front = open_window(&conn, root, Rect::new(300, 200, 200, 150), "front", 4343);
    let hidden = conn.generate_id().expect("id");
    conn.create_window(
        0,
        hidden,
        root,
        0,
        0,
        50,
        50,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .expect("create");
    conn.get_input_focus().expect("sync").reply().expect("sync");

    let windows = capturer.windows().expect("windows");
    let ids: Vec<u64> = windows.iter().map(|w| w.id).collect();
    let pos = |id: u32| ids.iter().position(|&x| x == u64::from(id));
    assert!(pos(hidden).is_none(), "unmapped window listed: {windows:?}");
    let (Some(f), Some(b)) = (pos(front), pos(back)) else {
        panic!("windows missing: {windows:?}");
    };
    assert!(f < b, "front window should come first: {windows:?}");
    assert_eq!(windows[f].title, "front");
    assert_eq!(windows[f].pid, 4343);
    assert_eq!(windows[b].bounds, Rect::new(100, 100, 400, 300));
    // Snapping: the overlap belongs to the front window, the rest of `back` to `back`.
    assert_eq!(
        window_at(&windows, 350, 250).map(|w| w.title.as_str()),
        Some("front")
    );
    assert_eq!(
        window_at(&windows, 120, 120).map(|w| w.title.as_str()),
        Some("back")
    );
}
