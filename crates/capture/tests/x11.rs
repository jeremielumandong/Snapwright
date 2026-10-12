//! Captures a real X server. Run under Xvfb:
//! `xvfb-run -s "-screen 0 1280x720x24" cargo test -p snapwright-capture --test x11 -- --ignored`
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used)]

use snapwright_capture::Capturer;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _};

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
}
