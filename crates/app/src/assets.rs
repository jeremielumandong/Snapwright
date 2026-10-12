//! Icons: GPUI Kit's component set, plus the few Lucide icons the editor's toolbar uses.

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

gpui_kit::assets::icon_assets!(
    EditorIcons,
    [
        CircleDot,
        Crop,
        Droplets,
        Grid3x3,
        Highlighter,
        MousePointer2,
        MoveUpRight,
        Slash,
        Type,
        Circle,
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        match EditorIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut out = gpui_kit::assets::Assets.list(path)?;
        out.extend(EditorIcons.list(path)?);
        out.sort();
        out.dedup();
        Ok(out)
    }
}
