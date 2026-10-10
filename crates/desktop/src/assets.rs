//! Embedded icons. `fm/*` are this app's own Lucide-style SVGs; anything
//! else is resolved by gpui-component's bundled assets (input widgets).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    ("fm/alert.svg", include_bytes!("../assets/fm/alert.svg")),
    ("fm/app.svg", include_bytes!("../assets/fm/app.svg")),
    ("fm/archive.svg", include_bytes!("../assets/fm/archive.svg")),
    ("fm/arrow-left.svg", include_bytes!("../assets/fm/arrow-left.svg")),
    ("fm/arrow-right.svg", include_bytes!("../assets/fm/arrow-right.svg")),
    ("fm/arrow-up.svg", include_bytes!("../assets/fm/arrow-up.svg")),
    ("fm/chevron-right.svg", include_bytes!("../assets/fm/chevron-right.svg")),
    ("fm/clipboard.svg", include_bytes!("../assets/fm/clipboard.svg")),
    ("fm/code.svg", include_bytes!("../assets/fm/code.svg")),
    ("fm/columns.svg", include_bytes!("../assets/fm/columns.svg")),
    ("fm/copy.svg", include_bytes!("../assets/fm/copy.svg")),
    ("fm/download.svg", include_bytes!("../assets/fm/download.svg")),
    ("fm/drive.svg", include_bytes!("../assets/fm/drive.svg")),
    ("fm/external.svg", include_bytes!("../assets/fm/external.svg")),
    ("fm/eye.svg", include_bytes!("../assets/fm/eye.svg")),
    ("fm/eye-off.svg", include_bytes!("../assets/fm/eye-off.svg")),
    ("fm/eye-watch.svg", include_bytes!("../assets/fm/eye-watch.svg")),
    ("fm/file.svg", include_bytes!("../assets/fm/file.svg")),
    ("fm/file-text.svg", include_bytes!("../assets/fm/file-text.svg")),
    ("fm/film.svg", include_bytes!("../assets/fm/film.svg")),
    ("fm/folder-fill.svg", include_bytes!("../assets/fm/folder-fill.svg")),
    ("fm/folder-plus.svg", include_bytes!("../assets/fm/folder-plus.svg")),
    ("fm/history.svg", include_bytes!("../assets/fm/history.svg")),
    ("fm/home.svg", include_bytes!("../assets/fm/home.svg")),
    ("fm/image.svg", include_bytes!("../assets/fm/image.svg")),
    ("fm/inbox.svg", include_bytes!("../assets/fm/inbox.svg")),
    ("fm/layers.svg", include_bytes!("../assets/fm/layers.svg")),
    ("fm/list.svg", include_bytes!("../assets/fm/list.svg")),
    ("fm/loader.svg", include_bytes!("../assets/fm/loader.svg")),
    ("fm/monitor.svg", include_bytes!("../assets/fm/monitor.svg")),
    ("fm/move.svg", include_bytes!("../assets/fm/move.svg")),
    ("fm/music.svg", include_bytes!("../assets/fm/music.svg")),
    ("fm/panel-left.svg", include_bytes!("../assets/fm/panel-left.svg")),
    ("fm/panel-right.svg", include_bytes!("../assets/fm/panel-right.svg")),
    ("fm/pencil.svg", include_bytes!("../assets/fm/pencil.svg")),
    ("fm/plus.svg", include_bytes!("../assets/fm/plus.svg")),
    ("fm/refresh.svg", include_bytes!("../assets/fm/refresh.svg")),
    ("fm/save.svg", include_bytes!("../assets/fm/save.svg")),
    ("fm/search.svg", include_bytes!("../assets/fm/search.svg")),
    ("fm/server.svg", include_bytes!("../assets/fm/server.svg")),
    ("fm/split.svg", include_bytes!("../assets/fm/split.svg")),
    ("fm/trash.svg", include_bytes!("../assets/fm/trash.svg")),
    ("fm/undo.svg", include_bytes!("../assets/fm/undo.svg")),
    ("fm/usb.svg", include_bytes!("../assets/fm/usb.svg")),
    ("fm/x.svg", include_bytes!("../assets/fm/x.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = ICONS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut found: Vec<SharedString> = ICONS.iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect();
        found.extend(gpui_component_assets::Assets.list(path)?);
        Ok(found)
    }
}
