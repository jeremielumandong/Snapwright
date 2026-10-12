//! Renders every annotation kind over a synthetic screenshot and writes `sample.png`
//! (or the path given): `cargo run -p snapwright-editor --example sample -- out.png`.

use snapwright_editor::{Color, Image, Kind, Point, Rect, Scene, Shape, render};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (w, h) = (640u32, 400u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            // A light "window" with dark text-like stripes on the left half.
            let stripe = x < 320 && (y / 6) % 3 == 0 && (x / 40) % 2 == 0;
            let v = if stripe { 40 } else { 236 };
            rgba.extend_from_slice(&[v, v, (v as u16 + 10).min(255) as u8, 255]);
        }
    }
    let base = Image::new(w, h, rgba)?;
    let s = |kind, color| Shape {
        kind,
        color,
        width: 4.,
    };
    let scene = Scene {
        shapes: vec![
            s(
                Kind::Highlight {
                    rect: Rect::new(20., 18., 260., 14.),
                },
                Color::YELLOW,
            ),
            s(
                Kind::Blur {
                    rect: Rect::new(20., 60., 200., 50.),
                },
                Color::RED,
            ),
            s(
                Kind::Pixelate {
                    rect: Rect::new(20., 130., 200., 50.),
                },
                Color::RED,
            ),
            s(
                Kind::Rectangle {
                    rect: Rect::new(360., 30., 220., 120.),
                },
                Color::RED,
            ),
            s(
                Kind::Ellipse {
                    rect: Rect::new(380., 200., 180., 90.),
                },
                Color::BLUE,
            ),
            s(
                Kind::Arrow {
                    from: Point::new(300., 360.),
                    to: Point::new(430., 250.),
                },
                Color::RED,
            ),
            s(
                Kind::Line {
                    from: Point::new(20., 220.),
                    to: Point::new(280., 220.),
                },
                Color::GREEN,
            ),
            s(
                Kind::Text {
                    at: Point::new(30., 250.),
                    text: "Click Save\nthen Done".into(),
                    size: 32.,
                },
                Color::RED,
            ),
            s(
                Kind::Step {
                    center: Point::new(600., 40.),
                    number: 1,
                },
                Color::RED,
            ),
            s(
                Kind::Step {
                    center: Point::new(600., 110.),
                    number: 2,
                },
                Color::YELLOW,
            ),
        ],
        ..Scene::default()
    };
    let out = render(&base, &scene);
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample.png".into());
    let mut enc = png::Encoder::new(std::fs::File::create(&path)?, out.width, out.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&out.rgba)?;
    println!("wrote {path}");
    Ok(())
}
