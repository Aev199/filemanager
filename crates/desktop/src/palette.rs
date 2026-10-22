//! Semantic colours used by rendering, without GPUI dependencies.

pub const WINDOW: u32 = 0x101011;
pub const PANEL: u32 = 0x131314;
pub const SURFACE: u32 = 0x161618;
pub const RAISED: u32 = 0x1F1F22;
/// Every other row of the details list, as in Atlas.
pub const STRIPE: u32 = 0x1A1A1C;
pub const BORDER: u32 = 0x252528;
pub const BORDER_STRONG: u32 = 0x333337;
pub const HOVER: u32 = 0x252529;
pub const PRESSED: u32 = 0x2C2C30;
pub const SELECTED: u32 = 0x0069D9;
pub const SELECTION_TEXT: u32 = 0xFFFFFF;
pub const INACTIVE_SELECTED: u32 = 0x35465C;
pub const SELECTED_BORDER: u32 = 0x3D6BD8;
/// Ancestors of the current folder in Miller columns.
pub const TRAIL: u32 = 0x24456B;
pub const ACCENT: u32 = 0x5E8BFF;
pub const ACCENT_SOFT: u32 = 0x1D2A47;
/// Frame of the active pane: visible, but quieter than the accent.
pub const ACTIVE_PANE: u32 = 0x34497A;

pub const TEXT: u32 = 0xDEDEE2;
pub const TEXT_MUTED: u32 = 0x9C9CA3;
pub const TEXT_DIM: u32 = 0x8D8D94;

pub const FOLDER: u32 = 0xE6B54A;
pub const DANGER: u32 = 0xEF6F7A;
pub const WARNING: u32 = 0xE5BD62;
/// A lit favourite star.
pub const FAVORITE: u32 = 0xF5C04A;
pub const SUCCESS: u32 = 0x74C98A;

pub const KIND_IMAGE: u32 = 0xC88BE8;
pub const KIND_DOCUMENT: u32 = 0x6FA8E8;
pub const KIND_SHEET: u32 = 0x6CC490;
pub const KIND_PDF: u32 = 0xE8787C;
pub const KIND_ARCHIVE: u32 = 0xE0B35C;
pub const KIND_MEDIA: u32 = 0xE38C5A;
pub const KIND_CODE: u32 = 0x5FC5C9;
pub const KIND_APP: u32 = 0x9AA3B5;

/// An accent scheme: the colours that follow the accent, for each theme,
/// in the order accent, soft accent, active pane, selection, selection
/// border, Miller trail.
pub struct AccentScheme {
    /// Stable key kept in the settings database.
    pub key: &'static str,
    pub label: &'static str,
    pub dark: [u32; 6],
    pub light: [u32; 6],
}

pub const ACCENT_SCHEMES: &[AccentScheme] = &[
    AccentScheme { key: "blue", label: "Синий",
        dark: [0x5E8BFF, 0x1D2A47, 0x34497A, 0x0069D9, 0x3D6BD8, 0x24456B],
        light: [0x285DC7, 0xE3EBFC, 0x9DB5E4, 0x0069D9, 0x376AC9, 0xCFDFF2] },
    AccentScheme { key: "teal", label: "Бирюзовый",
        dark: [0x3FC1C9, 0x163638, 0x2C5E62, 0x00727A, 0x2A9AA1, 0x1F4A4D],
        light: [0x00838C, 0xDDF3F4, 0x8FCFD3, 0x00727A, 0x1F8F96, 0xC9E9EB] },
    AccentScheme { key: "green", label: "Зелёный",
        dark: [0x5CC98A, 0x183522, 0x2F6444, 0x1E7A45, 0x3A9A62, 0x22482F],
        light: [0x23864C, 0xDFF2E6, 0x9CD3B1, 0x1E7A45, 0x2E8B55, 0xCDE8D6] },
    AccentScheme { key: "violet", label: "Фиолетовый",
        dark: [0x9A7CFF, 0x2A2147, 0x4B3E7F, 0x6A4BD6, 0x7E62E0, 0x3A2F66],
        light: [0x6A4BD6, 0xEBE5FC, 0xBFB0F0, 0x6A4BD6, 0x5B3FC4, 0xDCD3F7] },
    AccentScheme { key: "orange", label: "Оранжевый",
        dark: [0xF0A04B, 0x3A2A16, 0x6B4A22, 0xA8510C, 0xC86A1C, 0x4A3418],
        light: [0xB4590F, 0xFBEBDC, 0xE8BE93, 0xA8510C, 0x9C4A0B, 0xF3DCC4] },
    AccentScheme { key: "red", label: "Красный",
        dark: [0xF07178, 0x3A1C1F, 0x6B3237, 0xB8303A, 0xC9454E, 0x4A2427],
        light: [0xB8303A, 0xFBE3E5, 0xE7A3A8, 0xB8303A, 0xA02831, 0xF2D0D3] },
    AccentScheme { key: "graphite", label: "Графит",
        dark: [0xA9B1BD, 0x2A2D33, 0x4A505A, 0x4A5361, 0x6B7585, 0x353A42],
        light: [0x4A5361, 0xE6E8EC, 0xB5BBC5, 0x4A5361, 0x3E4652, 0xD8DCE2] },
];

/// The scheme with `key`, or the first (blue).
pub fn accent_scheme(key: &str) -> usize {
    ACCENT_SCHEMES.iter().position(|scheme| scheme.key == key).unwrap_or(0)
}

/// `resolve` with the user's accent scheme applied.
pub fn resolve_with(light: bool, scheme: usize, color: u32) -> u32 {
    let scheme = &ACCENT_SCHEMES[scheme.min(ACCENT_SCHEMES.len() - 1)];
    let values = if light { scheme.light } else { scheme.dark };
    match color {
        ACCENT => values[0],
        ACCENT_SOFT => values[1],
        ACTIVE_PANE => values[2],
        SELECTED => values[3],
        SELECTED_BORDER => values[4],
        TRAIL => values[5],
        _ => resolve(light, color),
    }
}

pub fn resolve(light: bool, color: u32) -> u32 {
    if light {
        match color {
            WINDOW => 0xECEEF3,
            PANEL => 0xF3F5F9,
            SURFACE => 0xFFFFFF,
            RAISED => 0xFFFFFF,
            STRIPE => 0xF5F6F8,
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
            ACTIVE_PANE => 0x9DB5E4,
            TEXT => 0x202631,
            TEXT_MUTED => 0x505B6D,
            TEXT_DIM => 0x626D80,
            FOLDER => 0xB9831B,
            DANGER => 0xBD3343,
            WARNING => 0x896000,
            FAVORITE => 0xC98A00,
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
        // Yellow as in Explorer, never the blue of the selection.
        for light in [false, true] {
            let folder = resolve(light, FOLDER);
            assert!((folder >> 16) & 255 > (folder & 255) + 80, "light={light}");
        }
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
    #[test]
    fn every_accent_scheme_keeps_selection_readable_and_distinct() {
        for scheme in 0..ACCENT_SCHEMES.len() {
            for light in [false, true] {
                let fg = luminance(resolve_with(light, scheme, SELECTION_TEXT));
                let bg = luminance(resolve_with(light, scheme, SELECTED));
                let contrast = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
                assert!(contrast >= 4.5, "{} light={light}: contrast={contrast}", ACCENT_SCHEMES[scheme].key);
                assert_ne!(resolve_with(light, scheme, SELECTED), resolve_with(light, scheme, HOVER));
                assert_ne!(resolve_with(light, scheme, SELECTED), resolve_with(light, scheme, TRAIL));
            }
        }
        // The first scheme is the palette as it always was.
        for color in [ACCENT, ACCENT_SOFT, ACTIVE_PANE, SELECTED, SELECTED_BORDER, TRAIL] {
            for light in [false, true] { assert_eq!(resolve_with(light, 0, color), resolve(light, color)); }
        }
        assert_eq!(accent_scheme("teal"), 1);
        assert_eq!(accent_scheme("unknown"), 0);
    }
}
