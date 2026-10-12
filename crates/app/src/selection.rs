//! The overlay's selection state machine, in frame pixels and free of UI types so it can be
//! tested directly.
//!
//! - Hovering highlights the window under the pointer (or the whole display).
//! - A click takes the highlighted window or display.
//! - A drag selects a region and delivers it on release. Holding Shift on release keeps the
//!   region open for adjustment instead.
//! - While adjusting: drag inside the region to move it, arrow keys nudge it, Shift+arrows
//!   resize it (10 pixels at a time with Ctrl/Cmd), Enter delivers.

use snapwright_capture::Rect;

/// Drags shorter than this, in frame pixels, count as a click.
pub const CLICK_SLOP: i32 = 4;
/// Pixels per arrow press with the fast modifier held.
pub const FAST_STEP: i32 = 10;

/// What the overlay should do after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed.
    Unchanged,
    /// Redraw.
    Changed,
    /// Copy this region of the frame and close.
    Deliver(Rect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Hover,
    Dragging { from: (i32, i32), to: (i32, i32) },
    Adjusting(Rect),
    Moving { rect: Rect, grab: (i32, i32) },
}

/// Arrow key directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Left,
    Right,
    Up,
    Down,
}

impl Arrow {
    /// Maps a GPUI key name.
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            _ => return None,
        })
    }

    fn delta(self) -> (i32, i32) {
        match self {
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
            Self::Up => (0, -1),
            Self::Down => (0, 1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Selector {
    frame: Rect,
    /// Windows on this display in frame pixels, front to back, already clipped to the frame.
    windows: Vec<Rect>,
    state: State,
    cursor: Option<(i32, i32)>,
}

impl Selector {
    /// A selector over a `width` x `height` frame with snap targets in frame pixels.
    pub fn new(width: u32, height: u32, windows: Vec<Rect>) -> Self {
        let frame = Rect::new(0, 0, width.max(1), height.max(1));
        let windows = windows.iter().filter_map(|w| w.intersect(&frame)).collect();
        Self {
            frame,
            windows,
            state: State::Hover,
            cursor: None,
        }
    }

    /// The pointer position in frame pixels, if it is over the overlay.
    pub fn cursor(&self) -> Option<(i32, i32)> {
        self.cursor
    }

    /// The region to draw as selected: the drag, the adjusted region, or the hover target.
    pub fn region(&self) -> Rect {
        match self.state {
            State::Hover => self.hover_target(),
            State::Dragging { from, to } => span(from, to),
            State::Adjusting(r) | State::Moving { rect: r, .. } => r,
        }
    }

    /// Whether the region is a hover highlight rather than a chosen selection.
    pub fn is_hovering(&self) -> bool {
        self.state == State::Hover
    }

    /// Whether the region can be moved and resized.
    pub fn is_adjusting(&self) -> bool {
        matches!(self.state, State::Adjusting(_) | State::Moving { .. })
    }

    fn hover_target(&self) -> Rect {
        self.cursor
            .and_then(|(x, y)| self.windows.iter().find(|w| w.contains(x, y)).copied())
            .unwrap_or(self.frame)
    }

    fn clamp_point(&self, (x, y): (i32, i32)) -> (i32, i32) {
        let f = self.frame;
        (
            x.clamp(f.x, f.x + f.width as i32),
            y.clamp(f.y, f.y + f.height as i32),
        )
    }

    pub fn pointer_down(&mut self, p: (i32, i32)) -> Outcome {
        let p = self.clamp_point(p);
        self.cursor = Some(p);
        self.state = match self.state {
            State::Adjusting(rect) if rect.contains(p.0, p.1) => State::Moving {
                rect,
                grab: (p.0 - rect.x, p.1 - rect.y),
            },
            _ => State::Dragging { from: p, to: p },
        };
        Outcome::Changed
    }

    pub fn pointer_move(&mut self, p: (i32, i32)) -> Outcome {
        let p = self.clamp_point(p);
        let before = (self.cursor, self.state);
        self.cursor = Some(p);
        self.state = match self.state {
            State::Dragging { from, .. } => State::Dragging { from, to: p },
            State::Moving { rect, grab } => State::Moving {
                rect: self.place(Rect::new(
                    p.0 - grab.0,
                    p.1 - grab.1,
                    rect.width,
                    rect.height,
                )),
                grab,
            },
            other => other,
        };
        if before == (self.cursor, self.state) {
            Outcome::Unchanged
        } else {
            Outcome::Changed
        }
    }

    pub fn pointer_up(&mut self, p: (i32, i32), keep_open: bool) -> Outcome {
        let p = self.clamp_point(p);
        self.cursor = Some(p);
        match self.state {
            State::Dragging { from, to } => {
                let to = if to == from { p } else { to };
                let dragged =
                    (to.0 - from.0).abs() >= CLICK_SLOP || (to.1 - from.1).abs() >= CLICK_SLOP;
                if !dragged {
                    // A click: take whatever was highlighted under it.
                    self.state = State::Hover;
                    return Outcome::Deliver(self.hover_target());
                }
                let rect = span(from, to);
                if keep_open {
                    self.state = State::Adjusting(rect);
                    Outcome::Changed
                } else {
                    Outcome::Deliver(rect)
                }
            }
            State::Moving { rect, .. } => {
                self.state = State::Adjusting(rect);
                Outcome::Changed
            }
            State::Hover | State::Adjusting(_) => Outcome::Unchanged,
        }
    }

    /// Arrow keys while adjusting: move, or with `resize` grow and shrink from the
    /// bottom-right corner.
    pub fn arrow(&mut self, arrow: Arrow, resize: bool, fast: bool) -> Outcome {
        let State::Adjusting(r) = self.state else {
            return Outcome::Unchanged;
        };
        let step = if fast { FAST_STEP } else { 1 };
        let (dx, dy) = arrow.delta();
        let next = if resize {
            let w = (r.width as i32 + dx * step).max(1);
            let h = (r.height as i32 + dy * step).max(1);
            let f = self.frame;
            let w = w.min(f.x + f.width as i32 - r.x);
            let h = h.min(f.y + f.height as i32 - r.y);
            Rect::new(r.x, r.y, w.max(1) as u32, h.max(1) as u32)
        } else {
            self.place(Rect::new(
                r.x + dx * step,
                r.y + dy * step,
                r.width,
                r.height,
            ))
        };
        if next == r {
            return Outcome::Unchanged;
        }
        self.state = State::Adjusting(next);
        Outcome::Changed
    }

    /// Enter: the adjusted region, or the hover target.
    pub fn confirm(&self) -> Outcome {
        Outcome::Deliver(self.region())
    }

    /// Keeps a rectangle inside the frame without changing its size.
    fn place(&self, r: Rect) -> Rect {
        let f = self.frame;
        let w = r.width.min(f.width);
        let h = r.height.min(f.height);
        let x = r.x.clamp(f.x, f.x + (f.width - w) as i32);
        let y = r.y.clamp(f.y, f.y + (f.height - h) as i32);
        Rect::new(x, y, w, h)
    }
}

fn span(a: (i32, i32), b: (i32, i32)) -> Rect {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    Rect::new(
        x,
        y,
        (a.0 - b.0).unsigned_abs().max(1),
        (a.1 - b.1).unsigned_abs().max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector() -> Selector {
        // A 1000x800 display with a window at (100,100) behind one at (200,150).
        Selector::new(
            1000,
            800,
            vec![
                Rect::new(200, 150, 300, 200),
                Rect::new(100, 100, 600, 500),
                Rect::new(900, 700, 400, 400), // runs off the display: clipped
            ],
        )
    }

    #[test]
    fn hover_highlights_the_front_window_or_the_display() {
        let mut s = selector();
        assert_eq!(s.region(), Rect::new(0, 0, 1000, 800));
        s.pointer_move((250, 200));
        assert_eq!(s.region(), Rect::new(200, 150, 300, 200));
        s.pointer_move((120, 120));
        assert_eq!(s.region(), Rect::new(100, 100, 600, 500));
        s.pointer_move((950, 750));
        assert_eq!(s.region(), Rect::new(900, 700, 100, 100));
        s.pointer_move((10, 10));
        assert_eq!(s.region(), Rect::new(0, 0, 1000, 800));
        assert!(s.is_hovering());
    }

    #[test]
    fn click_takes_the_hovered_window() {
        let mut s = selector();
        s.pointer_move((250, 200));
        s.pointer_down((250, 200));
        s.pointer_move((252, 201));
        assert_eq!(
            s.pointer_up((252, 201), false),
            Outcome::Deliver(Rect::new(200, 150, 300, 200))
        );
    }

    #[test]
    fn click_on_the_desktop_takes_the_display() {
        let mut s = selector();
        s.pointer_down((10, 10));
        assert_eq!(
            s.pointer_up((10, 10), false),
            Outcome::Deliver(Rect::new(0, 0, 1000, 800))
        );
    }

    #[test]
    fn drag_delivers_the_region_in_any_direction() {
        let mut s = selector();
        s.pointer_down((400, 300));
        s.pointer_move((100, 50));
        assert_eq!(s.region(), Rect::new(100, 50, 300, 250));
        assert_eq!(
            s.pointer_up((100, 50), false),
            Outcome::Deliver(Rect::new(100, 50, 300, 250))
        );
    }

    #[test]
    fn drag_is_clamped_to_the_display() {
        let mut s = selector();
        s.pointer_down((900, 700));
        s.pointer_move((1200, 900));
        assert_eq!(s.region(), Rect::new(900, 700, 100, 100));
    }

    #[test]
    fn shift_release_keeps_the_region_for_adjusting() {
        let mut s = selector();
        s.pointer_down((100, 100));
        s.pointer_move((200, 150));
        assert_eq!(s.pointer_up((200, 150), true), Outcome::Changed);
        assert!(s.is_adjusting());
        assert_eq!(s.arrow(Arrow::Right, false, false), Outcome::Changed);
        assert_eq!(s.region(), Rect::new(101, 100, 100, 50));
        s.arrow(Arrow::Down, false, true);
        assert_eq!(s.region(), Rect::new(101, 110, 100, 50));
        s.arrow(Arrow::Right, true, false);
        s.arrow(Arrow::Up, true, true);
        assert_eq!(s.region(), Rect::new(101, 110, 101, 40));
        assert_eq!(s.confirm(), Outcome::Deliver(Rect::new(101, 110, 101, 40)));
    }

    #[test]
    fn nudging_stops_at_the_edges_and_sizes_stay_positive() {
        let mut s = selector();
        s.pointer_down((0, 0));
        s.pointer_move((10, 10));
        s.pointer_up((10, 10), true);
        assert_eq!(s.arrow(Arrow::Left, false, false), Outcome::Unchanged);
        assert_eq!(s.arrow(Arrow::Up, false, true), Outcome::Unchanged);
        for _ in 0..5 {
            s.arrow(Arrow::Left, true, true);
        }
        assert_eq!(s.region(), Rect::new(0, 0, 1, 10));
        for _ in 0..200 {
            s.arrow(Arrow::Right, true, true);
        }
        assert_eq!(s.region().width, 1000);
    }

    #[test]
    fn dragging_inside_an_adjusting_region_moves_it() {
        let mut s = selector();
        s.pointer_down((100, 100));
        s.pointer_move((300, 200));
        s.pointer_up((300, 200), true);
        s.pointer_down((150, 150));
        s.pointer_move((950, 790));
        assert_eq!(
            s.region(),
            Rect::new(800, 700, 200, 100),
            "kept inside the display"
        );
        assert_eq!(s.pointer_up((950, 790), false), Outcome::Changed);
        assert!(s.is_adjusting());
        // Dragging outside the region starts a new one.
        s.pointer_down((10, 10));
        s.pointer_move((60, 60));
        assert_eq!(
            s.pointer_up((60, 60), false),
            Outcome::Deliver(Rect::new(10, 10, 50, 50))
        );
    }

    #[test]
    fn arrows_do_nothing_while_hovering() {
        let mut s = selector();
        assert_eq!(s.arrow(Arrow::Left, false, false), Outcome::Unchanged);
        assert_eq!(s.confirm(), Outcome::Deliver(Rect::new(0, 0, 1000, 800)));
    }

    #[test]
    fn pointer_moves_report_changes_only() {
        let mut s = selector();
        assert_eq!(s.pointer_move((5, 5)), Outcome::Changed);
        assert_eq!(s.pointer_move((5, 5)), Outcome::Unchanged);
        assert_eq!(s.cursor(), Some((5, 5)));
    }
}
