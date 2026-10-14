//! Where a finished capture goes: PNG bytes for the clipboard and a file in the captures
//! folder. Encoding runs on a background thread, never on the UI thread.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

/// Encodes RGBA pixels as PNG, tuned for speed: screenshots are flat colour, so fast
/// compression with the Up filter is close to the best ratio at a fraction of the time.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(rgba.len() / 4);
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Up);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(out)
}

/// Decodes a PNG to 8-bit RGBA: `(width, height, pixels)`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub fn decode_png(png: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("PNG too large")?];
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
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => anyhow::bail!("indexed PNG was not expanded"),
    };
    Ok((info.width, info.height, rgba))
}

/// The folder captures are saved to: `Pictures/Snapwright`, or `~/Snapwright` without one.
pub fn captures_dir() -> Result<PathBuf> {
    let dirs = directories::UserDirs::new().context("no home directory")?;
    let base = dirs
        .picture_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs.home_dir().to_path_buf());
    Ok(base.join("Snapwright"))
}

/// The folder recordings are saved to: `Movies/Snapwright` (`Videos` on Windows and Linux),
/// or `~/Snapwright` without one.
pub fn recordings_dir() -> Result<PathBuf> {
    let dirs = directories::UserDirs::new().context("no home directory")?;
    let base = dirs
        .video_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs.home_dir().to_path_buf());
    Ok(base.join("Snapwright"))
}

/// A new file name for a recording in the recordings folder (created if needed).
pub fn recording_path(extension: &str) -> Result<PathBuf> {
    let dir = writable(recordings_dir()?, "Recordings")?;
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    Ok(dir.join(format!("Snapwright {ms}.{extension}")))
}

/// `preferred`, created if needed, when files can be written there; otherwise `fallback` in
/// the app's local data folder. Windows' Controlled Folder Access keeps unknown apps out of
/// Pictures and Videos, and refuses even to create a folder there.
fn writable(preferred: PathBuf, fallback: &str) -> Result<PathBuf> {
    match probe(&preferred) {
        Ok(()) => Ok(preferred),
        Err(e) => {
            let dirs = directories::ProjectDirs::from("", "", "Snapwright")
                .context("no home directory")?;
            let dir = dirs.data_local_dir().join(fallback);
            tracing::warn!(
                folder = %preferred.display(),
                error = %e,
                fallback = %dir.display(),
                "cannot write to the folder; saving to the fallback"
            );
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            Ok(dir)
        }
    }
}

/// Creates `dir` and writes (then removes) an empty file in it.
fn probe(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let test = dir.join(format!(".snapwright-{}", std::process::id()));
    std::fs::write(&test, [])?;
    std::fs::remove_file(&test)
}

/// Where the capture library lives: the platform's data folder for Snapwright.
pub fn library_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "Snapwright").context("no home directory")?;
    Ok(dirs.data_dir().join("library"))
}

/// Saves PNG bytes to a new file in the captures folder and returns its path.
pub fn save(png: &[u8]) -> Result<PathBuf> {
    let dir = writable(captures_dir()?, "Captures")?;
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let path = dir.join(format!("Snapwright {ms}.png"));
    std::fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// The system clipboard on Linux, kept for the life of the app: on X11 the owner of the
/// clipboard serves every paste, so dropping it would empty the clipboard.
#[cfg(target_os = "linux")]
pub struct LinuxClipboard(std::sync::Mutex<Option<arboard::Clipboard>>);

#[cfg(target_os = "linux")]
impl LinuxClipboard {
    /// An idle clipboard; connects on first use.
    pub fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }

    /// Puts an RGBA image on the clipboard. Blocks briefly; call it off the UI thread.
    pub fn set_image(&self, width: u32, height: u32, rgba: Vec<u8>) -> Result<()> {
        let mut slot = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("clipboard poisoned by an earlier panic"))?;
        if slot.is_none() {
            *slot = Some(arboard::Clipboard::new()?);
        }
        let Some(clipboard) = slot.as_mut() else {
            anyhow::bail!("no clipboard");
        };
        clipboard.set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: rgba.into(),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn png_round_trips() {
        let rgba: Vec<u8> = (0..6 * 4).map(|i| i as u8 * 10).collect();
        let png = encode_png(3, 2, &rgba).unwrap();
        assert_eq!(decode_png(&png).unwrap(), (3, 2, rgba));
    }
}
