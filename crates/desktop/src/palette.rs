//! Semantic colours used by rendering, without GPUI dependencies.

pub const WINDOW: u32 = 0x0E0F12;
pub const PANEL: u32 = 0x141519;
pub const SURFACE: u32 = 0x18191E;
pub const RAISED: u32 = 0x1F2127;
pub const BORDER: u32 = 0x26282F;
pub const BORDER_STRONG: u32 = 0x33363F;
pub const HOVER: u32 = 0x292D35;
pub const PRESSED: u32 = 0x2B2E36;
pub const SELECTED: u32 = 0x0069D9;
pub const SELECTION_TEXT: u32 = 0xFFFFFF;
pub const INACTIVE_SELECTED: u32 = 0x35465C;
pub const SELECTED_BORDER: u32 = 0x3D6BD8;
/// Ancestors of the current folder in Miller columns.
pub const TRAIL: u32 = 0x24456B;
pub const ACCENT: u32 = 0x5E8BFF;
pub const ACCENT_SOFT: u32 = 0x1D2A47;

pub const TEXT: u32 = 0xE3E5EA;
pub const TEXT_MUTED: u32 = 0x989EAA;
pub const TEXT_DIM: u32 = 0x89919F;

pub const FOLDER: u32 = 0xA0AFC5;
pub const DANGER: u32 = 0xEF6F7A;
pub const WARNING: u32 = 0xE5BD62;
pub const SUCCESS: u32 = 0x74C98A;

pub const KIND_IMAGE: u32 = 0xC88BE8;
pub const KIND_DOCUMENT: u32 = 0x6FA8E8;
pub const KIND_SHEET: u32 = 0x6CC490;
pub const KIND_PDF: u32 = 0xE8787C;
pub const KIND_ARCHIVE: u32 = 0xE0B35C;
pub const KIND_MEDIA: u32 = 0xE38C5A;
pub const KIND_CODE: u32 = 0x5FC5C9;
pub const KIND_APP: u32 = 0x9AA3B5;

pub fn resolve(light: bool, color: u32) -> u32 {
    if light {
        match color {
            WINDOW => 0xECEEF3,
            PANEL => 0xF3F5F9,
            SURFACE => 0xFFFFFF,
            RAISED => 0xFFFFFF,
            BORDER => 0xDDE1E8,
            BORDER_STRONG => 0xBEC5D1,
            HOVER => 0xE9EDF5,
            PRESSED => 0xDCE3EF,
            SELECTED => 0x0069D9,
            SELECTION_TEXT => 0xFFFFFF,
            INACTIVE_SELECTED => 0x35465C,
            SELECTED_BORDER => 0x376AC9,
            TRAIL => 0xCFDFF2,
            ACCENT => 0x285DC7,
            ACCENT_SOFT => 0xE3EBFC,
            TEXT => 0x202631,
            TEXT_MUTED => 0x505B6D,
            TEXT_DIM => 0x626D80,
            FOLDER => 0x526680,
            DANGER => 0xBD3343,
            WARNING => 0x896000,
            SUCCESS => 0x24734A,
            KIND_IMAGE => 0x8643A4,
            KIND_DOCUMENT => 0x326AA8,
            KIND_SHEET => 0x277C51,
            KIND_PDF => 0xB03F45,
            KIND_ARCHIVE => 0x856000,
            KIND_MEDIA => 0xA75524,
            KIND_CODE => 0x23777B,
            KIND_APP => 0x637185,
            _ => color,
        }
    } else {
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(color: u32) -> f64 {
        let channel = |shift: u32| {
            let c = ((color >> shift) & 255_u32) as f64 / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    #[test]
    fn secondary_labels_remain_readable_in_both_themes() {
        for light in [false, true] {
            for background in [PANEL, SURFACE] {
                let fg = luminance(resolve(light, TEXT_DIM));
                let bg = luminance(resolve(light, background));
                let contrast = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
                assert!(contrast >= 4.5, "light={light}: contrast={contrast}");
            }
        }
    }
    #[test]
    fn folder_icons_are_not_another_selection_accent() {
        assert!((FOLDER & 255).abs_diff((FOLDER >> 16) & 255) <= 50);
    }
    #[test]
    fn selection_hover_and_trail_have_distinct_surfaces() {
        for light in [false, true] {
            assert_ne!(resolve(light, SELECTED), resolve(light, HOVER));
            assert_ne!(resolve(light, HOVER), resolve(light, TRAIL));
            assert_ne!(resolve(light, SELECTED), resolve(light, TRAIL));
        }
    }
    #[test]
    fn bright_selection_has_readable_text_in_both_themes() {
        for light in [false, true] {
            for background in [SELECTED, INACTIVE_SELECTED] {
                let fg = luminance(resolve(light, SELECTION_TEXT));
                let bg = luminance(resolve(light, background));
                let contrast = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
                assert!(contrast >= 4.5, "light={light}: contrast={contrast}");
            }
        }
    }
}
