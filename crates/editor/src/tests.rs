use super::*;

fn canvas(w: u32, h: u32, rgb: [u8; 3]) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    Image::new(w, h, rgba).unwrap()
}

/// Left half black, right half white: something for redactions to smear.
fn halves(w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..h {
        for x in 0..w {
            let v = if x < w / 2 { 0 } else { 255 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Image::new(w, h, rgba).unwrap()
}

fn px(img: &Image, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [
        img.rgba[i],
        img.rgba[i + 1],
        img.rgba[i + 2],
        img.rgba[i + 3],
    ]
}

fn shape(kind: Kind) -> Shape {
    Shape {
        kind,
        color: Color::RED,
        width: 4.,
    }
}

fn scene(shapes: Vec<Shape>) -> Scene {
    Scene {
        shapes,
        ..Scene::default()
    }
}

#[test]
fn scenes_round_trip_through_json_and_refuse_newer_versions() {
    let s = Scene {
        crop: Some(Rect::new(1., 2., 30., 40.)),
        shapes: vec![
            shape(Kind::Arrow {
                from: Point::new(0., 0.),
                to: Point::new(10., 5.),
            }),
            shape(Kind::Text {
                at: Point::new(3., 4.),
                text: "Click here".into(),
                size: 24.,
            }),
            shape(Kind::Step {
                center: Point::new(50., 50.),
                number: 1,
            }),
        ],
        ..Scene::default()
    };
    let json = s.to_json();
    assert!(json.contains(r#""type":"arrow""#), "{json}");
    assert_eq!(Scene::from_json(&json).unwrap(), s);
    let newer = json.replace(
        &format!(r#""version":{SCENE_VERSION}"#),
        &format!(r#""version":{}"#, SCENE_VERSION + 1),
    );
    assert!(Scene::from_json(&newer).is_err());
    assert!(Scene::from_json("not json").is_err());
}

#[test]
fn hits_pick_strokes_and_the_topmost_shape() {
    let rect = shape(Kind::Rectangle {
        rect: Rect::new(10., 10., 100., 100.),
    });
    let line = shape(Kind::Line {
        from: Point::new(0., 60.),
        to: Point::new(200., 60.),
    });
    let ellipse = shape(Kind::Ellipse {
        rect: Rect::new(300., 0., 100., 50.),
    });
    let s = scene(vec![rect, line, ellipse]);
    // On the rectangle's edge, and the line runs through its middle.
    assert_eq!(s.hit(Point::new(11., 30.), 2.), Some(0));
    assert_eq!(s.hit(Point::new(60., 61.), 2.), Some(1));
    // Inside the rectangle but away from both: nothing.
    assert_eq!(s.hit(Point::new(60., 30.), 2.), None);
    // Ellipse edge (rightmost point) yes, centre no.
    assert_eq!(s.hit(Point::new(399., 25.), 2.), Some(2));
    assert_eq!(s.hit(Point::new(350., 25.), 2.), None);
    // Where the line crosses the rectangle's edge, the later shape wins.
    assert_eq!(s.hit(Point::new(10., 60.), 2.), Some(1));
}

#[test]
fn steps_count_up_and_moves_shift_shapes() {
    let mut s = scene(vec![]);
    assert_eq!(s.next_step(), 1);
    s.shapes.push(shape(Kind::Step {
        center: Point::new(5., 5.),
        number: 3,
    }));
    assert_eq!(s.next_step(), 4);
    s.shapes[0].translate(10., -2.);
    assert_eq!(
        s.shapes[0].kind,
        Kind::Step {
            center: Point::new(15., 3.),
            number: 3
        }
    );
}

#[test]
fn clicks_without_a_drag_are_degenerate() {
    let tiny = shape(Kind::Rectangle {
        rect: Rect::from_corners(Point::new(5., 5.), Point::new(6., 7.)),
    });
    assert!(tiny.is_degenerate());
    let blank = shape(Kind::Text {
        at: Point::default(),
        text: "  ".into(),
        size: 20.,
    });
    assert!(blank.is_degenerate());
    let step = shape(Kind::Step {
        center: Point::default(),
        number: 1,
    });
    assert!(!step.is_degenerate());
}

#[test]
fn history_undoes_and_redoes_and_forgets_redo_after_an_edit() {
    let mut h = History::default();
    let mut s = scene(vec![]);
    h.record(s.clone());
    s.crop = Some(Rect::new(0., 0., 5., 5.));
    let cropped = s.clone();
    assert!(h.undo(&mut s));
    assert!(s.is_empty());
    assert!(h.redo(&mut s));
    assert_eq!(s, cropped);
    assert!(h.undo(&mut s));
    h.record(s.clone());
    s.shapes.push(shape(Kind::Step {
        center: Point::default(),
        number: 1,
    }));
    assert!(!h.can_redo());
    assert!(!h.redo(&mut s));
}

#[test]
fn rectangles_draw_their_outline_only() {
    let base = canvas(100, 100, [255, 255, 255]);
    let s = scene(vec![shape(Kind::Rectangle {
        rect: Rect::new(20., 20., 60., 60.),
    })]);
    let out = render(&base, &s);
    assert_eq!(px(&out, 20, 50), [0xE5, 0x48, 0x4D, 255]);
    assert_eq!(px(&out, 50, 50), [255, 255, 255, 255]);
    assert_eq!(px(&out, 5, 5), [255, 255, 255, 255]);
}

#[test]
fn arrows_end_in_a_filled_head_at_the_target() {
    let base = canvas(200, 100, [255, 255, 255]);
    let s = scene(vec![shape(Kind::Arrow {
        from: Point::new(10., 50.),
        to: Point::new(150., 50.),
    })]);
    let out = render(&base, &s);
    // Just behind the tip, and off the shaft's line within the head's width.
    assert_eq!(px(&out, 146, 50), [0xE5, 0x48, 0x4D, 255]);
    assert_eq!(px(&out, 136, 55), [0xE5, 0x48, 0x4D, 255]);
    // Past the tip and well off the shaft: untouched.
    assert_eq!(px(&out, 155, 50), [255, 255, 255, 255]);
    assert_eq!(px(&out, 60, 60), [255, 255, 255, 255]);
}

#[test]
fn text_paints_inside_its_bounds_only() {
    let base = canvas(300, 100, [255, 255, 255]);
    let t = shape(Kind::Text {
        at: Point::new(20., 20.),
        text: "Hello".into(),
        size: 32.,
    });
    let bounds = t.bounds();
    let out = render(&base, &scene(vec![t]));
    let mut inside = 0;
    for y in 0..out.height {
        for x in 0..out.width {
            if px(&out, x, y) != [255, 255, 255, 255] {
                assert!(
                    bounds.contains(Point::new(x as f32 + 0.5, y as f32 + 0.5)),
                    "painted outside the text bounds at {x},{y}"
                );
                inside += 1;
            }
        }
    }
    assert!(inside > 200, "only {inside} pixels of text");
    let (w, h) = measure_text("Hello", 32.);
    assert!(w > 60. && w < 120. && h > 30. && h < 50., "{w}x{h}");
}

#[test]
fn highlights_multiply_so_dark_stays_dark() {
    let mut s = scene(vec![Shape {
        kind: Kind::Highlight {
            rect: Rect::new(0., 0., 100., 10.),
        },
        color: Color::YELLOW,
        width: 4.,
    }]);
    let white = render(&canvas(100, 10, [255, 255, 255]), &s);
    assert_eq!(px(&white, 50, 5), [0xFF, 0xD6, 0x0A, 255]);
    let black = render(&canvas(100, 10, [0, 0, 0]), &s);
    assert_eq!(px(&black, 50, 5), [0, 0, 0, 255]);
    s.shapes.clear();
    assert_eq!(
        render(&canvas(4, 4, [9, 9, 9]), &s),
        canvas(4, 4, [9, 9, 9])
    );
}

#[test]
fn pixelate_flattens_blocks_and_leaves_the_rest() {
    let base = halves(64, 32);
    let s = scene(vec![shape(Kind::Pixelate {
        rect: Rect::new(24., 0., 16., 16.),
    })]);
    let out = render(&base, &s);
    let cell = Shape::pixel_cell(4.);
    assert_eq!(cell, 14);
    // The first block straddles the black/white edge: one grey for the whole block.
    let a = px(&out, 24, 0);
    assert!(a[0] > 0 && a[0] < 255, "{a:?}");
    assert_eq!(px(&out, 24 + cell - 1, cell - 1), a);
    // Outside the region nothing changed.
    assert_eq!(px(&out, 20, 5), [0, 0, 0, 255]);
    assert_eq!(px(&out, 45, 5), [255, 255, 255, 255]);
    assert_eq!(px(&out, 30, 20), px(&base, 30, 20));
}

#[test]
fn blur_smears_inside_the_region_only() {
    let base = halves(200, 40);
    let s = scene(vec![shape(Kind::Blur {
        rect: Rect::new(80., 10., 40., 20.),
    })]);
    let out = render(&base, &s);
    let edge = px(&out, 99, 20);
    assert!(edge[0] > 40 && edge[0] < 215, "edge not blurred: {edge:?}");
    // Row above the region and pixels beside it are untouched.
    for x in 70..130 {
        assert_eq!(px(&out, x, 9), px(&base, x, 9));
    }
    assert_eq!(px(&out, 79, 20), px(&base, 79, 20));
    assert_eq!(px(&out, 120, 20), px(&base, 120, 20));
}

#[test]
fn crop_cuts_the_output_and_the_cache_matches_a_fresh_render() {
    let base = halves(100, 60);
    let mut s = scene(vec![
        shape(Kind::Blur {
            rect: Rect::new(30., 10., 40., 20.),
        }),
        shape(Kind::Line {
            from: Point::new(0., 50.),
            to: Point::new(100., 50.),
        }),
    ]);
    s.crop = Some(Rect::new(10.4, 5.6, 50., 40.));
    assert_eq!(s.output_size(100, 60), (51, 41));
    let mut r = Renderer::new(base.clone());
    let first = r.render(&s, true);
    assert_eq!((first.width, first.height), (51, 41));
    assert_eq!(px(&first, 0, 0), px(&base, 10, 5));
    // Moving the line reuses the cached blur; the result still equals a fresh render.
    s.shapes[1].translate(0., -5.);
    assert_eq!(r.render(&s, true), render(&base, &s));
    // Uncropped preview keeps the full size.
    let full = r.render(&s, false);
    assert_eq!((full.width, full.height), (100, 60));
}

#[test]
fn steps_draw_a_disc_with_a_number() {
    let base = canvas(100, 100, [255, 255, 255]);
    let s = scene(vec![shape(Kind::Step {
        center: Point::new(50., 50.),
        number: 7,
    })]);
    let out = render(&base, &s);
    let r = Shape::step_radius(4.) as u32;
    // Disc colour near its rim, white digit strokes somewhere in the middle.
    assert_eq!(px(&out, 50 - r + 5, 50), [0xE5, 0x48, 0x4D, 255]);
    let white_inside = (40..60)
        .flat_map(|y| (44..56).map(move |x| (x, y)))
        .any(|(x, y)| px(&out, x, y) == [255, 255, 255, 255]);
    assert!(white_inside, "no digit drawn");
    assert_eq!(px(&out, 5, 5), [255, 255, 255, 255]);
}

#[test]
fn images_must_match_their_size() {
    assert!(Image::new(2, 2, vec![0; 15]).is_err());
    assert!(Image::new(0, 0, vec![]).is_err());
    assert!(Image::new(2, 2, vec![0; 16]).is_ok());
}
