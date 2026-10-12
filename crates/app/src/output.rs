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

/// The folder captures are saved to: `Pictures/Snapwright`, or `~/Snapwright` without one.
pub fn captures_dir() -> Result<PathBuf> {
    let dirs = directories::UserDirs::new().context("no home directory")?;
    let base = dirs
        .picture_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs.home_dir().to_path_buf());
    Ok(base.join("Snapwright"))
}

/// Saves PNG bytes to a new file in the captures folder and returns its path.
pub fn save(png: &[u8]) -> Result<PathBuf> {
    let dir = captures_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
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
