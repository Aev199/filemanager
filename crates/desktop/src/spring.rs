//! Spring-loaded navigation: holding dragged files over a folder, place,
//! path crumb or tab for a moment opens it, so the drop target can be
//! reached without letting go.

use std::path::PathBuf;
use std::time::Duration;

use filemanager_core::panes::GroupId;
use gpui::{App, Context};

use crate::Explorer;

const DELAY: Duration = Duration::from_millis(700);

#[derive(Clone, PartialEq)]
pub(crate) enum SpringTarget {
    /// Open the folder in this pane.
    Folder(GroupId, PathBuf),
    /// Switch this pane to the tab.
    Tab(GroupId, usize),
}

pub(crate) struct Spring {
    target: SpringTarget,
    generation: u64,
    /// Re-claimed by the hovered element on every move; the root drops an
    /// unclaimed spring, so leaving the target cancels it.
    claimed: bool,
}

impl Explorer {
    /// Files are being dragged: ours (left or right button) or Explorer's.
    pub(crate) fn dragging_files(&self, cx: &App) -> bool {
        (cx.has_active_drag() && self.file_drag) || self.right_drag.as_ref().is_some_and(|drag| drag.active)
    }

    /// Called by a hovered element while the pointer moves over it.
    pub(crate) fn spring_hover(&mut self, target: SpringTarget, cx: &mut Context<Self>) {
        if !self.dragging_files(cx) { return; }
        if let SpringTarget::Folder(_, folder) = &target {
            let dragged = match self.right_drag.as_ref().filter(|drag| drag.active) {
                Some(drag) => &drag.paths,
                None => &self.drag_paths,
            };
            if dragged.iter().any(|path| folder.starts_with(path)) { return; }
        }
        if let Some(spring) = self.spring.as_mut().filter(|spring| spring.target == target) {
            spring.claimed = true;
            return;
        }
        self.spring_generation += 1;
        let generation = self.spring_generation;
        self.spring = Some(Spring { target, generation, claimed: true });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DELAY).await;
            let _ = this.update(cx, |this, cx| {
                let Some(spring) = this.spring.as_ref().filter(|spring| spring.generation == generation) else { return };
                if !this.dragging_files(cx) { this.spring = None; return; }
                let target = spring.target.clone();
                // Fired once; the same target does not re-arm until left.
                this.spring.as_mut().unwrap().generation = 0;
                match target {
                    SpringTarget::Folder(side, folder) => {
                        if this.pane_path(side) != folder { this.navigate_side(side, &folder, None, cx); }
                    }
                    SpringTarget::Tab(group, index) => this.switch_group_tab(group, index, cx),
                }
                cx.notify();
            });
        }).detach();
    }

    /// Root mouse move, after the hovered elements had their turn.
    pub(crate) fn spring_settle(&mut self) {
        match self.spring.as_mut() {
            Some(spring) if spring.claimed => spring.claimed = false,
            Some(_) => self.spring = None,
            None => {}
        }
    }
}
