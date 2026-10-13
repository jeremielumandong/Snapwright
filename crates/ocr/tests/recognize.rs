//! Reads rendered text with the platform engine. On Linux the models are fetched once into
//! the target folder (or read from `SNAPWRIGHT_OCR_MODELS`). Where the OS has no OCR language
//! installed (a bare Windows image), the tests say so and pass.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use snapwright_editor::{Color, Kind, Point, Scene, Shape, render};
use snapwright_ocr::{Config, Error, Image, Order, Recognizer, text_of};

/// A white `width` x `height` image with each `(x, y, size, text)` drawn in black, as RGBA.
fn page(width: u32, height: u32, texts: &[(f32, f32, f32, &str)]) -> Vec<u8> {
    let base =
        snapwright_editor::Image::new(width, height, vec![255; (width * height * 4) as usize])
            .unwrap();
    let scene = Scene {
        shapes: texts
            .iter()
            .map(|&(x, y, size, text)| Shape {
                kind: Kind::Text {
                    at: Point::new(x, y),
                    text: text.into(),
                    size,
                },
                color: Color::BLACK,
                width: 4.,
            })
            .collect(),
        ..Scene::default()
    };
    render(&base, &scene).rgba
}

fn recognizer() -> Option<Recognizer> {
    let config = Config {
        models_dir: std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ocr-models"),
    };
    match Recognizer::new(&config) {
        Ok(r) => Some(r),
        Err(Error::Unavailable(why)) => {
            eprintln!("skipped: {why}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// Lower-cased words of the recognised text.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Levenshtein distance between two words.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let next = (prev + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[test]
fn reads_a_selection_of_ui_text() {
    let Some(mut ocr) = recognizer() else {
        return;
    };
    let rgba = page(
        420,
        90,
        &[
            (12., 12., 16., "Deploy failed: exit code 137"),
            (12., 48., 16., "Retry the build"),
        ],
    );
    let lines = ocr
        .recognize(&Image {
            pixels: &rgba,
            width: 420,
            height: 90,
            order: Order::Rgba,
        })
        .unwrap();
    let text = text_of(&lines);
    let found = words(&text);
    for want in ["deploy", "failed", "exit", "code", "137", "retry", "build"] {
        assert!(found.iter().any(|w| w == want), "{want:?} not in {text:?}");
    }
    // Reading order and boxes: the first line is above the second.
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].bounds.y < lines[1].bounds.y);
    assert!(
        lines[0].bounds.x < 40 && lines[0].bounds.width > 150,
        "{lines:?}"
    );
}

#[test]
fn reads_small_text_across_a_whole_screen_in_bgra() {
    let Some(mut ocr) = recognizer() else {
        return;
    };
    let (w, h) = (1920, 1080);
    // Small UI text in corners, the middle, and across the places a tiling engine cuts. This
    // checks that every line is read once; each engine misreads a letter or two of this font
    // at this size ("Quarterty" on Windows, "Batterv" from ocrs at 16 px), so a word one edit
    // away counts.
    let texts = [
        (20., 16., 14., "Inbox notifications"),
        (720., 300., 14., "Quarterly revenue report"),
        (1500., 590., 14., "Settings privacy"),
        (40., 1040., 14., "Terminal session closed"),
        (1600., 1040., 14., "Battery charging"),
    ];
    let mut bgra = page(w, h, &texts);
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let lines = ocr
        .recognize(&Image {
            pixels: &bgra,
            width: w,
            height: h,
            order: Order::Bgra,
        })
        .unwrap();
    let text = text_of(&lines);
    let found = words(&text);
    for want in [
        "inbox",
        "notifications",
        "quarterly",
        "revenue",
        "report",
        "settings",
        "privacy",
        "terminal",
        "session",
        "closed",
        "battery",
        "charging",
    ] {
        let hits = found.iter().filter(|w| edits(w, want) <= 1).count();
        assert_eq!(hits, 1, "{want:?} found {hits} times in {text:?}");
    }
}
