//! Pure geometry used by the native renderer.
pub const ANCESTOR_WIDTH: f32 = 144.;
pub const CURRENT_MIN: f32 = 200.;
pub const PREVIEW_MIN: f32 = 160.;
pub const ROW_HEIGHT: f32 = 24.;
pub fn row_index(local_y: f32, scroll: f32) -> usize {
    ((local_y + scroll).max(0.) / ROW_HEIGHT).floor() as usize
}
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
        return 2 + ((width - CURRENT_MIN - PREVIEW_MIN) / ANCESTOR_WIDTH) as usize;
    }
    1 + ((width - CURRENT_MIN).max(0.) / ANCESTOR_WIDTH) as usize
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
/// Keep the complete trail; viewport width must not delete navigation context.
pub fn column_paths(
    path: &std::path::Path,
    preview: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let mut columns: Vec<_> = path.ancestors().map(std::path::Path::to_path_buf).collect();
    columns.reverse();
    columns.extend(preview.map(std::path::Path::to_path_buf));
    columns
}
/// Negative GPUI scroll offset, revealing the current column with minimal movement.
/// Positions are content coordinates obtained from actual post-layout bounds.
pub fn reveal_offset(viewport: f32, start: f32, end: f32, offset: f32) -> f32 {
    if end - start > viewport || start + offset < 0. {
        -start
    } else if end + offset > viewport {
        viewport - end
    } else {
        offset
    }
}
pub fn reveal_column_offset(
    widths: &[f32],
    current: usize,
    viewport: f32,
    start: f32,
    actual_width: f32,
    offset: f32,
) -> f32 {
    let preview_width = widths.get(current + 1).copied().filter(|preview|actual_width + preview <= viewport).unwrap_or(0.);
    reveal_offset(
        viewport,
        start,
        start + actual_width + preview_width,
        offset,
    )
}
/// Miller widths are independent: expanding a column grows scrollable content.
pub fn resize_column(widths: &[f32], index: usize, requested: f32, current: usize) -> Vec<f32> {
    let mut result = widths.to_vec();
    if index < result.len() && requested.is_finite() {
        let minimum = if index == current {
            CURRENT_MIN
        } else if index > current {
            PREVIEW_MIN
        } else {
            ANCESTOR_WIDTH
        };
        result[index] = requested.clamp(minimum, 4000.);
    }
    result
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
            2
        );
        assert_eq!(
            column_capacity(effective_pane_width(750., Some(670.)), true),
            4
        );
    }
    #[test]
    fn preview_role_changes_reset_cached_column_sizes() {
        assert_ne!(column_identity(750., 3, 2), column_identity(750., 3, 1));
        assert_eq!(column_identity(750., 3, 1), column_identity(750., 3, 1));
    }
    #[test]
    fn narrow_pane_does_not_force_two_columns() {
        assert_eq!(column_capacity(280., false), 1);
        assert_eq!(column_capacity(280., true), 1);
    }
    #[test]
    fn current_folder_gets_space_not_its_ancestors() {
        assert_eq!(column_widths(750., 3, 2), vec![144., 144., 462.]);
        assert_eq!(column_widths(750., 3, 2).iter().sum::<f32>(), 750.);
    }
    #[test]
    fn preview_budget_preserves_current_folder() {
        assert_eq!(column_capacity(350., true), 1);
        assert_eq!(column_capacity(500., true), 2);
        assert_eq!(column_capacity(650., true), 4);
        assert_eq!(column_capacity(670., true), 4);
        assert_eq!(column_widths(670., 3, 1), vec![144., 366., 160.]);
    }
    #[test]
    fn single_column_uses_entire_pane() {
        assert_eq!(column_widths(430., 1, 0), vec![430.]);
        assert!(column_widths(430., 0, 0).is_empty());
    }
    #[test]
    fn wide_pane_is_not_limited_to_three_levels() {
        assert_eq!(column_capacity(1200., false), 7);
        assert_eq!(column_capacity(1200., true), 7);
    }
    #[test]
    fn deep_chain_overflows_instead_of_crushing_columns() {
        let widths = column_widths(600., 7, 5);
        assert_eq!(widths, vec![144., 144., 144., 144., 144., 200., 160.]);
        assert!(widths.iter().sum::<f32>() > 600.);
    }
    #[test]
    fn every_ancestor_remains_reachable_in_deep_chain() {
        let path = std::path::Path::new("/one/two/three/four/five/six");
        let preview = path.join("seven");
        let columns = column_paths(path, Some(&preview));
        assert_eq!(columns.len(), 8);
        assert_eq!(columns[0], std::path::PathBuf::from("/"));
        assert_eq!(columns[6], path);
        assert_eq!(columns[7], preview);
    }
    #[test]
    fn deep_navigation_reveals_current_column() {
        assert_eq!(reveal_offset(600., 720., 920., 0.), -320.);
    }
    #[test]
    fn folder_preview_is_visible_beside_current_when_it_fits() {
        assert_eq!(
            reveal_column_offset(&[144., 200., 160.], 1, 600., 720., 200., 0.),
            -480.
        );
    }
    #[test]
    fn oversized_preview_keeps_current_visible() {
        assert_eq!(reveal_column_offset(&[144.,200.,400.],1,440.,144.,200.,0.),0.);
    }
    #[test]
    fn navigating_up_reveals_column_to_the_left() {
        assert_eq!(reveal_offset(600., 144., 344., -720.), -144.);
    }
    #[test]
    fn already_visible_keeps_manual_scroll() {
        assert_eq!(reveal_offset(600., 200., 400., -100.), -100.);
    }
    #[test]
    fn oversized_column_reveals_its_start() {
        assert_eq!(reveal_offset(100., 144., 344., 0.), -144.);
    }
    #[test]
    fn compact_row_hit_testing_includes_scroll() {
        assert_eq!(row_index(49., 0.), 2);
        assert_eq!(row_index(1., 48.), 2);
        assert_eq!(row_index(23.9, 0.), 0);
        assert_eq!(row_index(24., 0.), 1);
    }
    #[test]
    fn resizing_deep_column_grows_content_without_shrinking_siblings() {
        let widths = column_widths(600., 7, 5);
        let enlarged = resize_column(&widths, 5, 480., 5);
        assert_eq!(enlarged[5], 480.);
        assert_eq!(&enlarged[..5], &widths[..5]);
        assert_eq!(enlarged[6], widths[6]);
        assert_eq!(
            enlarged.iter().sum::<f32>(),
            widths.iter().sum::<f32>() + 280.
        );
        assert_eq!(resize_column(&widths, 5, 50., 5)[5], CURRENT_MIN);
    }
}

#[cfg(test)]
mod dock_tests {
    use super::*;
    #[test]
    fn dock_regions_follow_center_edges_and_nearest_corner() {
        assert_eq!(dock_zone(50.,50.,100.,100.),DockZone::Center);
        assert_eq!(dock_zone(5.,50.,100.,100.),DockZone::Left);
        assert_eq!(dock_zone(95.,50.,100.,100.),DockZone::Right);
        assert_eq!(dock_zone(50.,5.,100.,100.),DockZone::Top);
        assert_eq!(dock_zone(50.,95.,100.,100.),DockZone::Bottom);
        assert_eq!(dock_zone(20.,5.,100.,100.),DockZone::Top);
        assert_eq!(dock_zone(95.,80.,100.,100.),DockZone::Right);
        assert_eq!(dock_zone(1.,1.,0.,100.),DockZone::Center);
        assert_eq!(dock_zone(1.,1.,100.,0.),DockZone::Center);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockZone { Left, Right, Top, Bottom, Center }
pub fn dock_zone(x: f32, y: f32, w: f32, h: f32) -> DockZone {
    if ![x,y,w,h].iter().all(|n|n.is_finite()) || w<=0. || h<=0. {return DockZone::Center;}
    let x=x.clamp(0.,w); let y=y.clamp(0.,h);
    [(DockZone::Left,x,x<w*0.3),(DockZone::Right,w-x,x>w*0.7),
     (DockZone::Top,y,y<h*0.3),(DockZone::Bottom,h-y,y>h*0.7)]
        .into_iter().filter(|(_,_,hit)|*hit).min_by(|a,b|a.1.total_cmp(&b.1))
        .map(|(zone,_,_)|zone).unwrap_or(DockZone::Center)
}

#[cfg(test)]
mod dock_hit_tests {
    use super::*;
    #[test]
    fn a_drag_source_outside_its_bounds_cannot_override_the_target() {
        assert_eq!(dock_hit(-510.,200.,550.,380.),None);
        assert_eq!(dock_hit(42.,200.,550.,700.),Some(DockZone::Left));
        assert_eq!(dock_hit(551.,20.,550.,380.),None);
    }
}

pub fn dock_hit(x: f32, y: f32, w: f32, h: f32) -> Option<DockZone> {
    if ![x,y,w,h].iter().all(|n|n.is_finite()) || w<=0. || h<=0. || x<0. || y<0. || x>w || y>h {return None;}
    Some(dock_zone(x,y,w,h))
}
