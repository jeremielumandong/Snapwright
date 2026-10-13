//! Fills a capture library with sample captures for the site's screenshots
//! (`scripts/site-shots.sh`): regions of a dashboard screenshot, two tags, a note, and an
//! annotated capture added last, so `--edit-latest` opens it.
//!
//! `cargo run -p snapwright-editor --example seed -- <library-dir> <dashboard.png>`

use snapwright_editor::{Color, Image, Kind, Point, Rect, Scene, Shape, render};
use snapwright_library::{EditedImage, Library, Metadata, NewCapture, Source};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `(title, x, y, width, height)` regions of the 1260×660 dashboard, oldest first.
const REGIONS: [(&str, u32, u32, u32, u32); 8] = [
    ("Northwind overview", 0, 0, 1260, 660),
    ("KPI cards", 228, 99, 1004, 117),
    ("Sign-ups per day", 228, 232, 562, 241),
    ("Open incidents", 806, 232, 426, 241),
    ("Active users", 228, 99, 324, 117),
    ("Navigation", 0, 0, 200, 300),
    ("P95 latency", 908, 99, 324, 117),
    ("Weekly metrics", 228, 18, 1004, 480),
];

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(source)) = (args.next(), args.next()) else {
        return Err("usage: seed <library-dir> <dashboard.png>".into());
    };
    let (width, rgba) = decode(&std::fs::read(source)?)?;
    let mut library = Library::open(root)?;
    let mut last = None;
    for (title, x, y, w, h) in REGIONS {
        let pixels = crop(&rgba, width, x, y, w, h);
        let id = library.add(&NewCapture {
            png: &encode(w, h, &pixels)?,
            rgba: &pixels,
            width: w,
            height: h,
            meta: Metadata {
                source: Source::Region,
                app: "Chromium-browser".into(),
                window_title: format!("{title} - Northwind"),
                url: "https://northwind.example/metrics".into(),
                display_id: None,
            },
        })?;
        last = Some((id, w, h, pixels));
    }
    let (id, w, h, pixels) = last.ok_or("no regions")?;
    library.tag(id, "incident")?;
    library.tag(id, "weekly-review")?;
    library.set_note(id, "Flag in Monday's review")?;
    let scene = annotations();
    let out = render(&Image::new(w, h, pixels)?, &scene);
    library.save_edit(
        id,
        &scene.to_json(),
        &EditedImage {
            png: &encode(out.width, out.height, &out.rgba)?,
            rgba: &out.rgba,
            width: out.width,
            height: out.height,
        },
    )?;
    println!("seeded {} captures", REGIONS.len());
    Ok(())
}

/// A box and a numbered step on the latency card, an arrow to the degraded service, a label.
fn annotations() -> Scene {
    let shape = |kind, color| Shape {
        kind,
        color,
        width: 4.,
    };
    Scene {
        shapes: vec![
            shape(
                Kind::Rectangle {
                    rect: Rect::new(684., 85., 316., 113.),
                },
                Color::RED,
            ),
            shape(
                Kind::Step {
                    center: Point::new(684., 85.),
                    number: 1,
                },
                Color::RED,
            ),
            shape(
                Kind::Arrow {
                    from: Point::new(760., 430.),
                    to: Point::new(700., 305.),
                },
                Color::RED,
            ),
            shape(
                Kind::Step {
                    center: Point::new(770., 440.),
                    number: 2,
                },
                Color::RED,
            ),
            shape(
                Kind::Text {
                    at: Point::new(150., 255.),
                    text: "Record week".into(),
                    size: 24.,
                },
                Color::RED,
            ),
        ],
        ..Scene::default()
    }
}

fn decode(png: &[u8]) -> Result<(u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        other => return Err(format!("unsupported PNG colour type {other:?}").into()),
    };
    Ok((info.width, rgba))
}

fn crop(rgba: &[u8], width: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    (y..y + h)
        .flat_map(|row| {
            let start = ((row * width + x) * 4) as usize;
            rgba[start..start + (w * 4) as usize].iter().copied()
        })
        .collect()
}

fn encode(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(out)
}
