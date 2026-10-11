//! Pure geometry used by the native renderer.
pub const ANCESTOR_WIDTH: f32 = 170.;
pub const CURRENT_MIN: f32 = 280.;
pub const PREVIEW_MIN: f32 = 210.;
pub fn effective_pane_width(estimate: f32, measured: Option<f32>) -> f32 {
    measured
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(estimate)
}
pub fn column_identity(width: f32, count: usize, current: usize) -> String {
    format!("{count}-{}-{current}", width as u32 / 40)
}
pub fn column_capacity(width: f32, preview: bool) -> usize {
    if preview {
        if width < CURRENT_MIN + PREVIEW_MIN {
            return 1;
        }
        return (2 + ((width - CURRENT_MIN - PREVIEW_MIN) / ANCESTOR_WIDTH) as usize).min(3);
    }
    (1 + ((width - CURRENT_MIN).max(0.) / ANCESTOR_WIDTH) as usize).min(3)
}
pub fn column_widths(width: f32, count: usize, current: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    let current = current.min(count - 1);
    let mut widths = vec![ANCESTOR_WIDTH; count];
    if current + 1 < count {
        widths[count - 1] = PREVIEW_MIN;
    }
    widths[current] = (width
        - widths
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != current)
            .map(|(_, width)| width)
            .sum::<f32>())
    .max(CURRENT_MIN);
    widths
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_left_and_right_widths_drive_capacity_independently() {
        assert_eq!(effective_pane_width(750., Some(410.)), 410.);
        assert_eq!(effective_pane_width(750., None), 750.);
        assert_eq!(
            column_capacity(effective_pane_width(750., Some(410.)), true),
            1
        );
        assert_eq!(
            column_capacity(effective_pane_width(750., Some(670.)), true),
            3
        );
    }
    #[test]
    fn preview_role_changes_reset_cached_column_sizes() {
        assert_ne!(column_identity(750., 3, 2), column_identity(750., 3, 1));
        assert_eq!(column_identity(750., 3, 1), column_identity(750., 3, 1));
    }
    #[test]
    fn narrow_pane_does_not_force_two_columns() {
        assert_eq!(column_capacity(430., false), 1);
        assert_eq!(column_capacity(280., true), 1);
    }
    #[test]
    fn current_folder_gets_space_not_its_ancestors() {
        assert_eq!(column_widths(750., 3, 2), vec![170., 170., 410.]);
        assert_eq!(column_widths(750., 3, 2).iter().sum::<f32>(), 750.);
    }
    #[test]
    fn preview_budget_preserves_current_folder() {
        assert_eq!(column_capacity(480., true), 1);
        assert_eq!(column_capacity(500., true), 2);
        assert_eq!(column_capacity(650., true), 2);
        assert_eq!(column_capacity(670., true), 3);
        assert_eq!(column_widths(670., 3, 1), vec![170., 290., 210.]);
    }
    #[test]
    fn single_column_uses_entire_pane() {
        assert_eq!(column_widths(430., 1, 0), vec![430.]);
        assert!(column_widths(430., 0, 0).is_empty());
    }
}
