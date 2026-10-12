//! Icons: GPUI Kit's component set, plus the Lucide icons the library, editor and preview use.

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

gpui_kit::assets::icon_assets!(
    AppIcons,
    [
        AppWindow,
        Circle,
        CircleDot,
        Crop,
        Droplets,
        FileImage,
        Grid3x3,
        Hash,
        Highlighter,
        Images,
        Monitor,
        MousePointer2,
        MoveUpRight,
        Pencil,
        Repeat2,
        Scan,
        Slash,
        SquareDashed,
        Timer,
        Trash,
        Type,
        X,
    ]
);

/// The app icon, for the library's sidebar.
pub const BRAND: &str = "brand/snapwright.svg";
static BRAND_SVG: &[u8] = include_bytes!("../assets/snapwright.svg");

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == BRAND {
            return Ok(Some(Cow::Borrowed(BRAND_SVG)));
        }
        match AppIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut out = gpui_kit::assets::Assets.list(path)?;
        out.extend(AppIcons.list(path)?);
        out.sort();
        out.dedup();
        Ok(out)
    }
}
