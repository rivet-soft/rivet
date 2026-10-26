//! Asset source: Kit's default icon set plus the extra Lucide icons Relay uses.
//! Add an icon here when `ui::icon(IconName::…)` renders blank.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    RelayIcons,
    [
        X,
        LoaderCircle,
        Unplug,
        GitBranch,
        SendHorizontal,
        Blocks,
        Layers,
        RotateCcwClock,
        RotateCcw,
        Bookmark,
        CircleArrowDown,
        CodeXml
    ]
);

pub struct RelayAssets;

impl AssetSource for RelayAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = RelayIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(RelayIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
