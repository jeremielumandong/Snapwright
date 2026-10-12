//! `snapwright` — capture from the command line and measure how fast capture is.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use snapwright_capture::{Capturer, Display, Frame, Rect};

#[derive(Parser)]
#[command(
    name = "snapwright",
    version,
    about = "Fast cross-platform screenshots"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List connected displays.
    Displays,
    /// Capture displays to PNG files.
    Capture {
        /// Display id from `snapwright displays`; every display when omitted.
        #[arg(long)]
        display: Option<u64>,
        /// Region inside the display as `x,y,width,height` in pixels.
        #[arg(long, value_parser = parse_rect)]
        region: Option<Rect>,
        /// Output file, or directory when capturing several displays.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
    /// Time repeated captures of every display and report percentiles.
    Bench {
        /// Captures per display.
        #[arg(long, default_value_t = 50)]
        runs: usize,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Displays => displays(),
        Command::Capture {
            display,
            region,
            output,
        } => capture(display, region, &output),
        Command::Bench { runs } => bench(runs),
    }
}

fn displays() -> Result<()> {
    let mut capturer = Capturer::new()?;
    for d in capturer.displays()? {
        let b = d.bounds;
        println!(
            "{:>8}  {:>5}x{:<5} at {:>5},{:<5} scale {:.2}{}  {}",
            d.id,
            b.width,
            b.height,
            b.x,
            b.y,
            d.scale,
            if d.primary { "  primary" } else { "" },
            d.name
        );
    }
    Ok(())
}

fn capture(display: Option<u64>, region: Option<Rect>, output: &Path) -> Result<()> {
    let start = Instant::now();
    let mut capturer = Capturer::new()?;
    let connected = Instant::now();
    let all = capturer.displays()?;
    let chosen: Vec<&Display> = match display {
        Some(id) => vec![
            all.iter()
                .find(|d| d.id == id)
                .with_context(|| format!("no display with id {id}; see `snapwright displays`"))?,
        ],
        None => all.iter().collect(),
    };
    for d in &chosen {
        let t = Instant::now();
        let mut frame = capturer.capture_display(d)?;
        let grabbed = t.elapsed();
        if let Some(r) = region {
            frame = frame.crop(r).context("region is outside the display")?;
        }
        let path = if chosen.len() == 1 && output.extension().is_some() {
            output.to_path_buf()
        } else {
            output.join(format!("snapwright-{}.png", d.id))
        };
        let t = Instant::now();
        write_png(&frame, &path)?;
        eprintln!(
            "{}: {}x{} grabbed in {}, encoded in {}",
            path.display(),
            frame.width,
            frame.height,
            ms(grabbed),
            ms(t.elapsed())
        );
    }
    eprintln!(
        "connect {}, total {}",
        ms(connected - start),
        ms(start.elapsed())
    );
    Ok(())
}

fn bench(runs: usize) -> Result<()> {
    if runs == 0 {
        bail!("--runs must be at least 1");
    }
    let t = Instant::now();
    let mut capturer = Capturer::new()?;
    println!("connect: {}", ms(t.elapsed()));
    for d in capturer.displays()? {
        let t = Instant::now();
        capturer.capture_display(&d)?;
        let first = t.elapsed();
        let mut times = Vec::with_capacity(runs);
        for _ in 0..runs {
            let t = Instant::now();
            let frame = capturer.capture_display(&d)?;
            times.push(t.elapsed());
            std::hint::black_box(frame);
        }
        times.sort();
        println!(
            "display {} ({}x{}): first {}, p50 {}, p95 {}, max {} over {runs} runs",
            d.id,
            d.bounds.width,
            d.bounds.height,
            ms(first),
            ms(percentile(&times, 50)),
            ms(percentile(&times, 95)),
            ms(times.last().copied().unwrap_or_default()),
        );
    }
    Ok(())
}

/// The `p`th percentile of sorted samples (nearest rank).
fn percentile(sorted: &[Duration], p: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (p * sorted.len()).div_ceil(100).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn write_png(frame: &Frame, path: &Path) -> Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // Screenshots are flat colour: fast compression with the Up filter is close to the best
    // ratio at a fraction of the time.
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Up);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&frame.to_rgba())?;
    writer.finish()?;
    Ok(())
}

fn parse_rect(s: &str) -> Result<Rect, String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let [x, y, w, h] = parts.as_slice() else {
        return Err("expected x,y,width,height".into());
    };
    let num = |v: &str| v.parse::<i64>().map_err(|e| format!("{v}: {e}"));
    let (x, y, w, h) = (num(x)?, num(y)?, num(w)?, num(h)?);
    if w <= 0 || h <= 0 {
        return Err("width and height must be positive".into());
    }
    Ok(Rect::new(x as i32, y as i32, w as u32, h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank() {
        let v: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&v, 50), Duration::from_millis(50));
        assert_eq!(percentile(&v, 95), Duration::from_millis(95));
        assert_eq!(percentile(&v[..1], 95), Duration::from_millis(1));
    }

    #[test]
    fn parses_regions() {
        assert_eq!(parse_rect("1, 2,30,40"), Ok(Rect::new(1, 2, 30, 40)));
        assert!(parse_rect("1,2,3").is_err());
        assert!(parse_rect("0,0,0,10").is_err());
    }
}
