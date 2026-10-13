//! Prints the text in a PNG: `cargo run --release -p snapwright-ocr --example recognize -- shot.png`.
//! On Linux the models go to (or are read from) `SNAPWRIGHT_OCR_MODELS`, else `./ocr-models`.

use std::time::Instant;

use snapwright_ocr::{Config, Image, Order, Recognizer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: recognize <image.png>")?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf)?;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        other => return Err(format!("unsupported PNG colour type {other:?}").into()),
    };
    let started = Instant::now();
    let mut recognizer = Recognizer::new(&Config {
        models_dir: "ocr-models".into(),
    })?;
    eprintln!(
        "engine ready in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    let lines = recognizer.recognize(&Image {
        pixels: &rgba,
        width: info.width,
        height: info.height,
        order: Order::Rgba,
    })?;
    eprintln!(
        "{} lines in {:.0} ms",
        lines.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    println!("{}", snapwright_ocr::text_of(&lines));
    Ok(())
}
