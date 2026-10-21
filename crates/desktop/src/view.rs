//! Window layout and rendering. All state lives on `Explorer`; nothing
//! here reads the disk — listings arrive from background snapshots.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use filemanager_core::browser::{self, Entry};
use filemanager_core::format::{format_size, format_time, items_label, kind_label};
use filemanager_core::places::{containing_place, Place, PlaceKind};
use filemanager_core::sort::SortKey;
use filemanager_core::versions::Stacked;
use gpui::{
    anchored, deferred, div, img, rgb_to_hsla, ObjectFit, ease_out_quint, prelude::*, px, rgba, svg, uniform_list,
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, Div, ElementId, Hsla,
    ExternalPaths, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Stateful, Window,
};
use gpui_component::input::Input;
use gpui_component::Sizable;
use gpui_component::resizable::{h_resizable, v_resizable, resizable_panel};
use filemanager_core::panes::{Node, Axis, Edge, tab_titles};
use gpui_component::scroll::ScrollableElement;
use gpui_component::theme::Theme;
use gpui_component::tooltip::Tooltip;
use crate::projects::{FavoriteDrag, PlaceDrag};
use gpui_component::{ElementExt, InteractiveElementExt};

use crate::keys;
use crate::theme::*;
use crate::{ContextMenu, Explorer, FileDragInfo, FileDragPreview, MenuTarget, GroupId, TabDrag};

use crate::layout::ROW_HEIGHT;

/// Fixed, as in Atlas: the sidebar does not grow with the window.
const SIDEBAR_WIDTH: f32 = 220.;

fn column_scroll_area<I: Into<gpui::ElementId>>(id: I, scroll: &gpui::ScrollHandle) -> impl IntoElement + Styled + ParentElement + use<I> {
    div().id(id).relative().track_scroll(scroll).flex_1().min_w_0().min_h_0()
        .overflow_x_scroll().lock_scroll_axis()
}

#[cfg(test)]
mod atlas_scroll_tests {
    use super::*;
    #[test]
    fn explicit_column_handle_tracks_the_overflow_viewport() {
        let mut area = column_scroll_area("test-columns", &gpui::ScrollHandle::default());
        assert_eq!(area.style().overflow.x, Some(gpui::Overflow::Scroll));
    }
}

/// Aligns gpui-component widgets (inputs, scrollbars, tooltips) with our palette.
pub fn apply_component_theme(cx: &mut App) {
    let colors = &mut cx.global_mut::<Theme>().colors;
    colors.background = hsla(SURFACE);
    colors.foreground = hsla(TEXT);
    colors.muted_foreground = hsla(TEXT_MUTED);
    colors.border = hsla(BORDER_STRONG);
    colors.input = hsla(BORDER_STRONG);
    colors.ring = hsla(ACCENT);
    colors.caret = hsla(ACCENT);
    colors.selection = rgb_to_hsla(crate::theme::accent_alpha(0x55));
    colors.popover = hsla(RAISED);
    colors.popover_foreground = hsla(TEXT);
    colors.primary = hsla(ACCENT);
    colors.accent = hsla(HOVER);
    colors.accent_foreground = hsla(TEXT);
    colors.scrollbar = rgb_to_hsla(rgba(0x00000000));
    colors.scrollbar_thumb = rgb_to_hsla(rgba(if crate::theme::is_light() { 0x00000033 } else { 0xFFFFFF22 }));
    colors.scrollbar_thumb_hover = rgb_to_hsla(rgba(if crate::theme::is_light() { 0x00000055 } else { 0xFFFFFF40 }));
    // No glow outside a focused field (it came out reddish); the blue
    // border is enough, as in Atlas.
    cx.global_mut::<Theme>().focus_ring = false;
    // Re-project onto the base layer, which draws focus rings and scrollbars.
    Theme::sync_base(cx);
}

fn hsla(color: u32) -> Hsla {
    rgb_to_hsla(rgb(color))
}

fn icon(path: &'static str, color: u32) -> gpui::Svg {
    svg().path(path).flex_none().size(px(16.)).text_color(rgb(color))
}

fn small_icon(path: &'static str, color: u32) -> gpui::Svg {
    svg().path(path).flex_none().size(px(14.)).text_color(rgb(color))
}

/// Square ghost button with an icon and a tooltip.
fn icon_button(
    id: impl Into<ElementId>,
    icon_path: &'static str,
    tooltip: &'static str,
    active: bool,
    enabled: bool,
) -> Stateful<Div> {
    let color = if !enabled { TEXT_DIM } else if active { ACCENT } else { TEXT_MUTED };
    div().id(id).flex_none().size(px(30.)).rounded_md().flex().items_center().justify_center()
        .when(active, |this| this.bg(rgb(ACCENT_SOFT)))
        .when(enabled, |this| this.cursor_pointer()
            .hover(|style| style.bg(rgb(HOVER)))
            .active(|style| style.bg(rgb(PRESSED))))
        .child(icon(icon_path, color))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
}

/// Text button for dialogs and panels.
/// Small icon button of the bottom bar, like the corner icons in Atlas.
fn bar_button(id: &'static str, path: &'static str, tip: &'static str, active: bool) -> Stateful<Div> {
    div().id(id).flex_none().size(px(24.)).rounded_sm().flex().items_center().justify_center()
        .cursor_pointer()
        .when(active, |this| this.bg(rgb(ACCENT_SOFT)))
        .hover(|style| style.bg(rgb(HOVER)))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(svg().path(path).size(px(14.)).text_color(rgb(if active { ACCENT } else { TEXT_MUTED })))
}

/// «Версия 1.6», or «Исходный» for the file without a suffix.

fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind) -> Stateful<Div> {
    let (bg, hover, fg) = match kind {
        ButtonKind::Primary => (ACCENT, 0x7AA0FF, 0xFFFFFF),
        ButtonKind::Danger => (0x9E3A43, 0xB8454F, 0xFFFFFF),
        ButtonKind::Ghost => (RAISED, HOVER, TEXT),
    };
    div().id(id).flex_none().h(px(28.)).px_3().rounded_md().flex().items_center()
        .justify_center().bg(rgb(bg)).text_color(rgb(fg)).cursor_pointer()
        .hover(move |style| style.bg(rgb(hover)))
        .child(label.into())
}

#[derive(Clone, Copy)]
enum ButtonKind { Primary, Danger, Ghost }

fn place_icon(kind: PlaceKind) -> &'static str {
    match kind {
        PlaceKind::Home => "fm/home.svg",
        PlaceKind::Desktop => "fm/monitor.svg",
        PlaceKind::Documents => "fm/file-text.svg",
        PlaceKind::Downloads => "fm/download.svg",
        PlaceKind::Pictures => "fm/image.svg",
        PlaceKind::Music => "fm/music.svg",
        PlaceKind::Videos => "fm/film.svg",
        PlaceKind::Drive => "fm/drive.svg",
        PlaceKind::RemovableDrive => "fm/usb.svg",
        PlaceKind::NetworkDrive => "fm/server.svg",
    }
}

fn entry_icon(entry: &Entry) -> (&'static str, u32) {
    if entry.is_directory {
        ("fm/folder-fill.svg", FOLDER)
    } else {
        file_icon(entry.extension().as_deref())
    }
}

impl Explorer {
    // ---------------------------------------------------------------- tabs

    fn tab_strip(&mut self, group: GroupId, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.browser.group(group).unwrap().tabs.clone();
        let active_index = self.browser.group(group).unwrap().active_tab;
        let titles = tab_titles(&tabs.iter().map(|t|t.pane.path.clone()).collect::<Vec<_>>());
        let scroll = self.tab_scroll.entry(group).or_default().clone();
        let reveal = (active_index,tabs.len());
        if self.tab_reveal.get(&group) != Some(&reveal) {
            scroll.scroll_to_item(active_index); self.tab_reveal.insert(group,reveal);
        }
        let wheel = scroll.clone();
        let mut strip = div().id(SharedString::from(format!("tabs-{}",group.0)))
            .flex_1().min_w_0().h_full().flex().items_end().gap_1().px_1()
            .overflow_x_scroll().track_scroll(&scroll)
            .on_scroll_wheel(move |event,window,cx| {
                let delta=event.delta.pixel_delta(px(32.));
                let x=(wheel.offset().x+delta.x+delta.y).min(px(0.)).max(-wheel.max_offset().x);
                wheel.set_offset(gpui::point(x,px(0.))); window.refresh(); cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Left,cx.listener(move |this,event: &MouseDownEvent,_,cx| {
                if event.click_count==2 { this.focus_group(group,cx); this.add_tab(cx); }
            }))
            .on_drop(cx.listener(move |this,drag: &TabDrag,_,cx| {
                let end=this.browser.group(group).map(|g|g.tabs.len()).unwrap_or(0);
                this.move_tab_to(drag,group,end,cx);
            }));
        for (index,tab) in tabs.iter().enumerate() {
            let active=index==active_index;
            let closable=(tabs.len()>1 || self.browser.groups.len()>1) && !tab.pinned;
            let tip=tab.pane.path.display().to_string();
            strip=strip.child(div().id(("tab",index)).group("tab").flex_none().h(px(32.))
                .min_w(px(96.)).max_w(px(200.)).pl_2().pr_1().flex().items_center().gap_2()
                .rounded_t_md().cursor_pointer()
                .when(active,|row|row.bg(rgb(SURFACE)).text_color(rgb(TEXT)).border_t_2().border_color(rgb(ACCENT)))
                .when(!active,|row|row.text_color(rgb(TEXT_MUTED)).hover(|style|style.bg(rgb(HOVER)).text_color(rgb(TEXT))))
                .tooltip(move |window,cx|Tooltip::new(tip.clone()).build(window,cx))
                .child(small_icon("fm/folder-fill.svg",if active {FOLDER} else {TEXT_DIM}))
                .when(tab.pinned,|row|row.child(div().text_color(rgb(ACCENT)).child("●")))
                .child(div().flex_1().min_w_0().truncate().child(titles[index].clone()))
                .when(closable,|row|row.child(div().id(("tab-close",index)).flex_none().size(px(20.)).rounded_sm()
                    .flex().items_center().justify_center().invisible().group_hover("tab",|style|style.visible())
                    .when(active,|row|row.visible()).hover(|style|style.bg(rgb(PRESSED)))
                    .child(small_icon("fm/x.svg",TEXT_MUTED))
                    .on_click(cx.listener(move |this,_,_,cx| {cx.stop_propagation();this.focus_group(group,cx);this.close_tab_at(index,cx);}))))
                .on_click(cx.listener(move |this,_,_,cx| {cx.stop_propagation();this.switch_group_tab(group,index,cx);} ))
                .when(index!=active_index,|tab|tab.on_mouse_move(cx.listener(move |this,_,_,cx| this.spring_hover(crate::spring::SpringTarget::Tab(group,index),cx))))
                .on_drag(TabDrag {group,index,title:titles[index].clone()},|drag: &TabDrag,position,_,cx|
                    cx.new(|_|FileDragPreview {name:drag.title.clone(),position}))
                .drag_over::<TabDrag>(|style,_,_,_|style.border_l_2().border_color(rgb(ACCENT)))
                .on_drop(cx.listener(move |this,drag: &TabDrag,_,cx| {cx.stop_propagation();this.move_tab_to(drag,group,index,cx);} ))
                .on_mouse_down(MouseButton::Middle,cx.listener(move |this,_,_,cx| {cx.stop_propagation();this.focus_group(group,cx);this.close_tab_at(index,cx);} ))
                .on_mouse_down(MouseButton::Left,cx.listener(|_,_,_,cx|cx.stop_propagation()))
                .on_mouse_down(MouseButton::Right,cx.listener(move |this,event: &MouseDownEvent,_,cx| {
                    this.focus_group(group,cx);
                    this.open_menu(event.position,MenuTarget::Tab(group,index),cx);
                })));
        }
        // A project lives in this pane: its name leads the tab strip.
        let project = self.browser.group(group).and_then(|g|g.project.as_ref()).map(|p|p.name.clone());
        // Never wider than its pane: overflowing tabs scroll inside the strip.
        div().h(px(34.)).w_full().flex_none().min_w_0().overflow_hidden().flex().items_center().bg(rgb(WINDOW))
            .when_some(project, |bar,name| bar.child(div().id(SharedString::from(format!("project-{}",group.0)))
                .flex_none().max_w(px(180.)).h(px(24.)).ml_1().pl_2().pr_0p5().flex().items_center().gap_1p5()
                .rounded_md().bg(rgb(ACCENT_SOFT)).text_size(px(12.)).text_color(rgb(TEXT))
                .tooltip({let name=name.clone(); move |window,cx|Tooltip::new(format!("Проект «{name}» открыт в этой панели")).build(window,cx)})
                .child(small_icon("fm/layers.svg",ACCENT))
                .child(div().min_w_0().truncate().child(name))
                .child(div().id(SharedString::from(format!("project-close-{}",group.0))).flex_none().size(px(20.)).rounded_sm()
                    .flex().items_center().justify_center().hover(|style|style.bg(rgb(PRESSED)))
                    .child(small_icon("fm/x.svg",TEXT_MUTED))
                    .tooltip(|window,cx|Tooltip::new("Закрыть проект — вернуть прежние вкладки").build(window,cx))
                    .on_click(cx.listener(move |this,_,_,cx| {cx.stop_propagation();this.close_project(group,cx);})))))
            .child(strip)
            .child(icon_button("new-tab","fm/plus.svg","Новая вкладка (Ctrl+T)",false,true)
                .on_click(cx.listener(move |this,_,_,cx| {this.focus_group(group,cx);this.add_tab(cx);})))
            .child(icon_button("tab-list","fm/chevron-down.svg","Все вкладки",false,true)
                .on_mouse_down(MouseButton::Left,cx.listener(move |this,event: &MouseDownEvent,window,cx| {
                    cx.stop_propagation(); this.focus_group(group,cx); window.focus(&this.focus_handle,cx);
                    this.open_menu(event.position,MenuTarget::TabList(group),cx);
                })))
            .into_any_element()
    }

    fn move_tab_to(&mut self, drag: &TabDrag, group: GroupId, index: usize, cx: &mut Context<Self>) {
        if let Err(error)=self.browser.move_tab(drag.group,drag.index,group,index) {self.status=error.to_string();}
        self.prune_group_state(); self.selected=None; self.marked.clear(); self.anchor=None;
        self.marquee=None; self.context_menu=None; self.clear_filter=!self.filter.is_empty(); self.close_search(); cx.notify();
    }

    fn group_view(&mut self, group: GroupId, cx: &mut Context<Self>) -> AnyElement {
        let tabs=self.tab_strip(group,cx);
        let navigation=self.nav_row(group,cx).into_any_element();
        let body=self.pane(group,cx);
        div().id(SharedString::from(format!("group-{}",group.0))).size_full().min_w_0().min_h_0()
            .flex().flex_col().overflow_hidden().border_1()
            .border_color(rgb(if group==self.browser.active {ACTIVE_PANE} else {BORDER}))
            .on_mouse_down(MouseButton::Left,cx.listener(move |this,_,_,cx|this.focus_group(group,cx)))
            .child(tabs).child(navigation).child(div().flex_1().min_h_0().child(body)).into_any_element()
    }

    fn layout_view(&mut self, node: &Node, path: Vec<bool>, width: f32, height: f32, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Leaf(id)=>self.group_view(*id,cx),
            Node::Split {axis,ratio,first,second}=>{
                let horizontal=*axis==Axis::Horizontal;
                let mut a=path.clone();a.push(false);let mut b=path.clone();b.push(true);
                let one=self.layout_view(first,a,if horizontal {width*ratio} else {width},if horizontal {height} else {height*ratio},cx);
                let two=self.layout_view(second,b,if horizontal {width*(1.-ratio)} else {width},if horizontal {height} else {height*(1.-ratio)},cx);
                let key=SharedString::from(format!("split-{}-{:?}",self.browser.signature(),path));
                // Each pane draws its own frame. The divider's built-in line
                // would cover the left or top edge of the active pane's frame,
                // so it paints nothing; dragging still works.
                let group=if horizontal {h_resizable(key)} else {v_resizable(key)}
                    .with_handle_appearance(std::rc::Rc::new(|_,_,_| Some(div().into_any_element())));
                let available=if horizontal {width} else {height};
                group.child(resizable_panel().size(px(available*ratio)).size_range(px(240.)..px(10000.)).child(one))
                    .child(resizable_panel().size(px(available*(1.-ratio))).size_range(px(240.)..px(10000.)).child(two))
                    .on_resize(cx.listener(move |this,state: &gpui::Entity<gpui_component::resizable::ResizableState>,_,cx| {
                        let sizes=state.read(cx).sizes();
                        if sizes.len()==2 && sizes[0]+sizes[1]>px(0.) {
                            this.browser.set_ratio(&path,sizes[0]/(sizes[0]+sizes[1])); cx.notify();
                        }
                    })).into_any_element()
            }
        }
    }

    /// Ends a list column drag and keeps the widths for the next start.
    pub(crate) fn finish_list_resize(&mut self) {
        let Some((side, ..)) = self.list_resize.take() else { return };
        // Panes still on the old shared widths keep them: only this one changed.
        for id in self.browser.leaves() {
            if id != side && !self.pane_list_widths.contains_key(&id) {
                self.pane_list_widths.insert(id, self.list_widths);
            }
        }
        // The last widths set start new panes, now and after a restart.
        self.list_widths = self.list_widths_for(side);
        if let Some(store)=&self.workspaces {
            let [a,b,c]=self.list_widths;
            let _=store.set_value(crate::LIST_WIDTHS, &format!("{a:.0},{b:.0},{c:.0}"));
        }
    }

    pub(crate) fn finish_column_resize(&mut self) {
        let Some(resize) = self.column_resize.take() else { return };
        // Panes still on the old shared width keep it: only this one changed.
        for id in self.browser.leaves() {
            if id != resize.group && !self.pane_column_widths.contains_key(&id) {
                self.pane_column_widths.insert(id, self.column_width);
            }
        }
        self.column_width = self.column_width_for(resize.group);
        if let Some(store)=&self.workspaces {
            let _=store.set_value(crate::COLUMN_WIDTH, &format!("{:.0}", self.column_width));
        }
    }

    pub(crate) fn open_tour(&mut self, cx: &mut Context<Self>) {
        self.palette=None; self.settings=None; self.context_menu=None; self.about_open=false;
        self.tour=Some(0); cx.notify();
    }
    /// Closing the tour, finished or skipped, means it was seen.
    pub(crate) fn close_tour(&mut self, cx: &mut Context<Self>) {
        self.tour=None;
        if let Some(store)=&self.workspaces { let _=store.set_flag(crate::TOUR_SEEN); }
        cx.notify();
    }
    fn tour_view(&self, step: usize, cx: &mut Context<Self>) -> AnyElement {
        let (icon, title, text) = TOUR[step.min(TOUR.len()-1)];
        let last = step + 1 >= TOUR.len();
        let dots = div().flex().gap_1().children((0..TOUR.len()).map(|i| div().size(px(6.)).rounded_full()
            .bg(rgb(if i == step { ACCENT } else { BORDER_STRONG }))));
        let card = div().id("tour").w(px(460.)).p_5().flex().flex_col().gap_3()
            .rounded_md().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_,_,cx| cx.stop_propagation())
            .child(div().flex().items_center().gap_3()
                .child(if step == 0 { img("app/icon.png").size(px(32.)).into_any_element() }
                    else { div().size(px(32.)).rounded_md().bg(rgb(ACCENT_SOFT)).flex().items_center().justify_center()
                        .child(svg().path(icon).size(px(18.)).text_color(rgb(ACCENT))).into_any_element() })
                .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child(title)))
            // One height for every step: the buttons stay under the cursor.
            .child(div().min_h(px(120.)).text_size(px(13.)).line_height(px(20.)).text_color(rgb(TEXT_MUTED)).child(text))
            .child(div().pt_2().flex().items_center().gap_2()
                .child(dots)
                .child(div().flex_1())
                .when(!last, |row| row.child(text_button("tour-skip", "Пропустить", ButtonKind::Ghost)
                    .on_click(cx.listener(|this,_,_,cx| this.close_tour(cx)))))
                .when(step > 0, |row| row.child(text_button("tour-back", "Назад", ButtonKind::Ghost)
                    .on_click(cx.listener(|this,_,_,cx| {this.tour=this.tour.map(|s| s.saturating_sub(1));cx.notify();}))))
                .child(text_button("tour-next", if last { "Начать работу" } else { "Далее" }, ButtonKind::Primary)
                    .on_click(cx.listener(move |this,_,_,cx| {
                        if last { this.close_tour(cx); } else { this.tour=Some(step+1); cx.notify(); }
                    }))));
        div().id("tour-backdrop").absolute().inset_0().flex().items_center().justify_center()
            .bg(rgba(0x0000008C))
            .child(card)
            .with_animation("tour-appear", Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta))
            .into_any_element()
    }

    fn comparison_view(&self, cx: &mut Context<Self>) -> AnyElement {
        use filemanager_core::folder_compare::Difference::*;
        let comparison = self.comparison.as_ref().unwrap();
        let (to_right, to_left) = (comparison.count(true), comparison.count(false));
        let side = |side: &Option<filemanager_core::folder_compare::Side>, is_dir: bool| match side {
            Some(s) if is_dir => "папка".to_string(),
            Some(s) => format!("{} · {}", format_size(s.size), s.modified.map(format_time).unwrap_or_default()),
            None => "—".into(),
        };
        let body: AnyElement = match &comparison.rows {
            None => div().p_4().text_color(rgb(TEXT_DIM)).child("Сравниваем…").into_any_element(),
            Some(Err(error)) => div().p_4().text_color(rgb(DANGER)).child(format!("Сравнить не удалось: {error}")).into_any_element(),
            Some(Ok(rows)) if rows.is_empty() => div().p_4().text_color(rgb(SUCCESS)).child("Папки совпадают: имена, размеры и даты одинаковы.").into_any_element(),
            Some(Ok(rows)) => {
                let shown = rows.iter().take(2000).map(|row| {
                    let (mark, color, tip) = match row.difference {
                        OnlyLeft => ("→", ACCENT, "Только слева"),
                        OnlyRight => ("←", ACCENT, "Только справа"),
                        LeftNewer => ("→", SUCCESS, "Слева новее"),
                        RightNewer => ("←", SUCCESS, "Справа новее"),
                        SizeDiffers => ("≠", WARNING, "Дата одинаковая, размер разный — решите сами"),
                    };
                    div().h(px(24.)).flex().items_center().gap_2().px_2().text_size(px(12.))
                        .child(div().w(px(18.)).flex_none().text_color(rgb(color)).child(mark))
                        .child(div().w(px(110.)).flex_none().text_color(rgb(TEXT_DIM)).truncate().child(tip))
                        .child(div().flex_1().min_w_0().truncate().text_color(rgb(TEXT)).child(row.relative.display().to_string()))
                        .child(div().w(px(170.)).flex_none().truncate().text_color(rgb(TEXT_MUTED)).child(side(&row.left, row.is_dir)))
                        .child(div().w(px(170.)).flex_none().truncate().text_color(rgb(TEXT_MUTED)).child(side(&row.right, row.is_dir)))
                });
                div().id("comparison-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col()
                    .children(shown)
                    .when(rows.len() > 2000, |list| list.child(div().p_2().text_color(rgb(TEXT_DIM)).child(format!("Показаны первые 2000 из {}", rows.len()))))
                    .into_any_element()
            }
        };
        let card = div().id("comparison").w(gpui::relative(0.85)).h(gpui::relative(0.85)).flex().flex_col()
            .rounded_md().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_,_,cx| cx.stop_propagation())
            .child(div().p_3().flex().flex_col().gap_1().border_b_1().border_color(rgb(BORDER))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child("Сравнение папок"))
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).truncate().child(format!("Слева: {}", comparison.left.display())))
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).truncate().child(format!("Справа: {}", comparison.right.display()))))
            .child(div().h(px(24.)).flex().items_center().gap_2().px_2().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                .border_b_1().border_color(rgb(BORDER))
                .child(div().w(px(136.)).flex_none()).child(div().flex_1().child("Путь"))
                .child(div().w(px(170.)).flex_none().child("Слева")).child(div().w(px(170.)).flex_none().child("Справа")))
            .child(body)
            .child(div().p_3().flex().items_center().gap_2().border_t_1().border_color(rgb(BORDER))
                .child(div().flex_1().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                    .child("Сравниваются имена, размеры и даты. Новый файл заменяет старый, старый уходит в Корзину."))
                .child(text_button("comparison-again", "Сравнить снова", ButtonKind::Ghost)
                    .on_click(cx.listener(|this,_,_,cx| this.rerun_comparison(cx))))
                .child(text_button("comparison-left", format!("← Скопировать влево ({to_left})"), if to_left > 0 { ButtonKind::Primary } else { ButtonKind::Ghost })
                    .when(to_left > 0, |b| b.on_click(cx.listener(|this,_,_,cx| this.sync_comparison(false, cx)))))
                .child(text_button("comparison-right", format!("Скопировать вправо ({to_right}) →"), if to_right > 0 { ButtonKind::Primary } else { ButtonKind::Ghost })
                    .when(to_right > 0, |b| b.on_click(cx.listener(|this,_,_,cx| this.sync_comparison(true, cx)))))
                .child(text_button("comparison-close", "Закрыть", ButtonKind::Ghost)
                    .on_click(cx.listener(|this,_,_,cx| {this.comparison=None;cx.notify();}))));
        div().id("comparison-backdrop").absolute().inset_0().flex().items_center().justify_center()
            .bg(rgba(0x0000008C))
            .child(card)
            .into_any_element()
    }

    fn quick_look_view(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::quick_look::QuickContent;
        let look = self.quick_look.as_ref().unwrap();
        let path = look.path.clone();
        let name = browser::display_name(&path);
        let body: AnyElement = match &look.content {
            QuickContent::Loading => div().text_color(rgb(TEXT_DIM)).child("Загрузка…").into_any_element(),
            QuickContent::Image(file) => img(file.clone()).size_full().object_fit(ObjectFit::Contain).into_any_element(),
            QuickContent::Text(text) => div().id("quick-look-text").size_full().overflow_y_scroll().p_3()
                .font_family("Consolas").text_size(px(12.)).line_height(px(18.)).text_color(rgb(TEXT))
                .child(text.clone()).into_any_element(),
            QuickContent::Message(message) => div().max_w(px(420.)).text_color(rgb(TEXT_MUTED)).child(message.clone()).into_any_element(),
        };
        let (open, preview) = (path.clone(), path.clone());
        let document = crate::quick_look::is_document(&path);
        let card = div().id("quick-look").w(gpui::relative(0.8)).h(gpui::relative(0.85)).flex().flex_col()
            .rounded_md().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_,_,cx| cx.stop_propagation())
            .child(div().h(px(40.)).flex_none().px_3().flex().items_center().gap_2().border_b_1().border_color(rgb(BORDER))
                .child(div().flex_1().min_w_0().truncate().font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child(name))
                .when(document, |bar| bar.child(text_button("quick-look-preview", "Подробный предпросмотр", ButtonKind::Ghost)
                    .on_click(cx.listener(move |this,_,_,cx| {this.quick_look=None;this.open_document_preview(preview.clone(),cx);}))))
                .child(text_button("quick-look-open", "Открыть", ButtonKind::Ghost)
                    .on_click(cx.listener(move |this,_,_,cx| {
                        this.quick_look=None;
                        let side=this.active_side();
                        this.open_entry(open.clone(), open.is_dir(), side, cx);
                    })))
                .child(icon_button("quick-look-close","fm/x.svg","Закрыть (Пробел, Esc)",false,true)
                    .on_click(cx.listener(|this,_,_,cx| {this.quick_look=None;cx.notify();}))))
            .child(div().flex_1().min_h_0().p_2().flex().items_center().justify_center().child(body))
            .child(div().h(px(26.)).flex_none().px_3().flex().items_center().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                .child("Пробел или Esc — закрыть · ↑ ↓ — соседние файлы"));
        div().id("quick-look-backdrop").absolute().inset_0().flex().items_center().justify_center()
            .bg(rgba(0x00000080))
            .on_mouse_down(MouseButton::Left, cx.listener(|this,_,_,cx| {this.quick_look=None;cx.notify();}))
            .child(card)
            .into_any_element()
    }

    /// «О программе» under its titlebar button, as in SoilStiffness:
    /// a click elsewhere or Esc closes it.
    fn about_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let data = filemanager_core::path_utils::app_data_dir().display().to_string();
        div().id("about-backdrop").absolute().inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|this,_,_,cx| {this.about_open=false;cx.notify();}))
            .child(div().id("about-panel").absolute().top(px(36.)).right(px(12.)).w(px(340.)).p_4()
                .flex().flex_col().gap_2().rounded_md().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG))
                .shadow_lg().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                .on_mouse_down(MouseButton::Left, |_,_,cx| cx.stop_propagation())
                .child(div().flex().items_center().gap_2()
                    .child(img("app/icon.png").size(px(28.)))
                    .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(rgb(TEXT))
                        .child(format!("Papka {}", crate::updates::VERSION)))
                    .when(!crate::updates::COMMIT.is_empty(), |row| row.child(div().text_color(rgb(TEXT_DIM)).child(crate::updates::COMMIT))))
                .child(div().child("Файловый менеджер для проектных папок: колонки и списки, \
                    панели с вкладками, проекты, журнал изменений файлов без хранения их содержимого."))
                .child(div().flex().flex_wrap().gap_1()
                    .child("Автор:")
                    .child(div().id("about-author").text_color(rgb(ACCENT)).cursor_pointer()
                        .hover(|style| style.underline())
                        .child("Евдокимов Александр")
                        .on_click(|_,_,cx| cx.open_url("https://t.me/aevd19"))))
                .child(div().text_color(rgb(TEXT_DIM)).child(format!("Настройки и журналы: {data}")))
                .child(div().pt_1().flex().flex_wrap().gap_1()
                    .child(text_button("about-tour", "Экскурс по возможностям", ButtonKind::Ghost)
                        .on_click(cx.listener(|this,_,_,cx| {this.about_open=false;this.open_tour(cx);})))
                    .child(text_button("about-update", if matches!(self.updates.phase, crate::updates::Phase::Checking) {"Проверяем…"} else {"Проверить обновления"}, ButtonKind::Ghost)
                        .on_click(cx.listener(|this,_,_,cx| this.check_updates(true,cx))))
                    .when(crate::updates::Updates::previous_exists(), |row| row.child(text_button("about-rollback", "Вернуть предыдущую версию", ButtonKind::Ghost)
                        .on_click(cx.listener(|this,_,_,cx| {this.about_open=false;this.rollback_update(cx);})))))
                .when_some(self.updates.note.clone(), |panel, note| panel.child(div().text_color(rgb(TEXT)).child(note))))
            .into_any_element()
    }

    /// Window strip above the panes: it drags the window and holds the
    /// system buttons; each pane has its own tab strip.
    fn title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = !cfg!(target_os="macos") && (!cfg!(target_os="linux") || matches!(window.window_decorations(),gpui::Decorations::Client {..}));
        div().h(px(32.)).flex_none().flex().items_center().bg(rgb(WINDOW))
            .when(cfg!(target_os="macos") && !self.show_sidebar, |bar|bar.pl(px(80.)))
            .when(!self.show_sidebar, |bar|bar.child(icon_button("toggle-sidebar","fm/panel-left.svg","Боковая панель (Ctrl+B)",false,true)
                .on_click(cx.listener(|this,_,_,cx| {this.show_sidebar=true;cx.notify();}))))
            .child(div().flex_1().min_w_0().h_full().px_3().flex().items_center()
                .window_control_area(gpui::WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left,|_,window,_| {if cfg!(any(target_os="linux",target_os="macos")) {window.start_window_move();}})
            )
            .children(self.update_badge(cx))
            .child(icon_button("open-settings","fm/settings.svg","Настройки (Ctrl+,)",self.settings.is_some(),true)
                .on_click(cx.listener(|this,_,window,cx| {this.about_open=false;this.open_settings(window,cx);})))
            .child(icon_button("about","fm/help.svg","О программе",self.about_open,true)
                .on_click(cx.listener(|this,_,_,cx| {this.about_open=!this.about_open;cx.notify();})))
            .child(icon_button("toggle-inspector","fm/panel-right.svg","Панель сведений (Ctrl+I)",self.show_inspector,true)
                .on_click(cx.listener(|this,_,_,cx| {this.show_inspector=!this.show_inspector;cx.notify();})))
            .when(controls, |bar|bar.child(icon_button("window-minimize","fm/window-minimize.svg","Свернуть",false,true)
                .on_click(|_,window,_|window.minimize_window()))
            .child(icon_button("window-maximize","fm/window-maximize.svg","Развернуть / восстановить",false,true)
                .when(cfg!(target_os="windows"), |button|button.window_control_area(gpui::WindowControlArea::Max))
                .on_click(|_,window,_| {if !cfg!(target_os="windows") {window.zoom_window();}}))
            .child(icon_button("window-close","fm/x.svg","Закрыть окно",false,true)
                .on_click(cx.listener(|this,_,window,cx| {
                    if this.prepare_window_close(cx) {window.remove_window();}
                }))))
    }

    // ------------------------------------------------------------- toolbar

    fn breadcrumbs(&self, side: GroupId, cx: &mut Context<Self>) -> AnyElement {
        if self.address_editing == Some(side) {
            return div().flex_1().min_w_0().h(px(30.))
                .child(Input::new(&self.address_input).h(px(30.)))
                .into_any_element();
        }
        let path = self.pane_path(side);
        let chain = browser::ancestors_from_root(&path);
        // Long paths keep the root and the last folders, like Explorer.
        let skip = if self.pane_width_for(side)<400. && chain.len()>2 {chain.len()-1}
            else if chain.len()>6 {chain.len()-4} else {0};
        let mut crumbs = div().id("breadcrumbs").flex_1().min_w_0().h(px(30.)).px_1()
            .flex().items_center().gap_0p5().overflow_hidden().rounded_md()
            .hover(|style| style.bg(rgb(HOVER))).cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus_group(side,cx);
                this.address_editing = Some(side);
                this.focus_address(window, cx);
            }));
        for (index, folder) in chain.iter().enumerate() {
            if index > 0 && index < skip {
                if index == 1 {
                    crumbs = crumbs.child(small_icon("fm/chevron-right.svg", TEXT_DIM))
                        .child(div().px_1().text_color(rgb(TEXT_DIM)).child("…"));
                }
                continue;
            }
            if index > 0 {
                crumbs = crumbs.child(small_icon("fm/chevron-right.svg", TEXT_DIM));
            }
            let last = index + 1 == chain.len();
            let target = folder.clone();
            crumbs = crumbs.child(
                // The current folder is only as wide as its name: the empty
                // rest of the bar belongs to the bar and opens the path input.
                div().id(("crumb", index)).when(last,|crumb|crumb.min_w_0())
                    .when(!last,|crumb|crumb.flex_none()).max_w(px(220.)).truncate().px_1p5().py_0p5()
                    .rounded_sm().cursor_pointer()
                    .text_color(rgb(if last { TEXT } else { TEXT_MUTED }))
                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .child(browser::display_name(folder))
                    // A click on the current folder edits the path, as in Explorer.
                    .when(!last, |crumb| crumb.on_click(cx.listener({ let target = target.clone(); move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.navigate_side(side, &target, None, cx);
                    }}))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        this.spring_hover(crate::spring::SpringTarget::Folder(side, target.clone()), cx);
                    })))
            );
        }
        crumbs.into_any_element()
    }

    fn nav_row(&self, side: GroupId, cx: &mut Context<Self>) -> impl IntoElement {
        let group = self.browser.group(side).unwrap();
        let pane = &group.tabs[group.active_tab].pane;
        let can_back = pane.can_go_back();
        let can_forward = pane.can_go_forward();
        let can_up = pane.path.parent().is_some();
        div().w_full().h(px(36.)).flex_none().flex().items_center().gap_1().px_2()
            .bg(rgb(SURFACE)).border_b_1().border_color(rgb(BORDER))
            .child(icon_button("back", "fm/arrow-left.svg", "Назад (Alt+←)", false, can_back)
                .on_click(cx.listener(move |this, _, _, cx| {this.focus_group(side,cx);this.go_back(false,cx);})))
            .child(icon_button("forward", "fm/arrow-right.svg", "Вперёд (Alt+→)", false, can_forward)
                .on_click(cx.listener(move |this, _, _, cx| {this.focus_group(side,cx);this.go_back(true,cx);})))
            .child(icon_button("up", "fm/arrow-up.svg", "Вверх (Backspace)", false, can_up)
                .on_click(cx.listener(move |this, _, _, cx| {this.focus_group(side,cx);this.go_up(cx);})))
            .child(div().w_2())
            .child(self.breadcrumbs(side,cx))
            .child(icon_button("favorite-current","fm/star.svg",
                if self.favorites.contains(&pane.path) {"Убрать из избранного"} else {"Добавить в избранное"},
                self.favorites.contains(&pane.path),true)
                .on_click(cx.listener(move |this,_,_,cx| {let path=this.pane_path(side);this.toggle_favorite(&path,cx);})))
    }

    // ------------------------------------------------------------- sidebar

    fn place_row(&self, id: (&'static str, usize), place: &Place, active: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let path = place.path.clone();
        let middle=path.clone();
        let spring=path.clone();
        div().id(id).mx_2().h(px(28.)).px_2().flex().items_center().gap_2().rounded_md()
            .cursor_pointer()
            .when(active, |this| this.bg(rgb(ACCENT_SOFT)).text_color(rgb(TEXT)))
            .when(!active, |this| this.text_color(rgb(TEXT_MUTED))
                .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT))))
            .child(icon(place_icon(place.kind), if active { ACCENT } else { TEXT_MUTED }))
            .child(div().flex_1().min_w_0().truncate().child(place.label.clone()))
            .on_click(cx.listener(move |this, _, _, cx| {
                let side = this.active_side();
                this.navigate_side(side, &path, None, cx);
            }))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                let side = this.active_side();
                this.spring_hover(crate::spring::SpringTarget::Folder(side, spring.clone()), cx);
            }))
            .on_mouse_down(MouseButton::Middle,cx.listener(move |this,_,window,cx| {
                this.open_in_new_tab(&middle,cx);window.focus(&this.focus_handle,cx);
            }))
            .on_drop(cx.listener(|this, data: &FileDragInfo, _, cx| this.stage_paths(&data.paths, cx)))
    }

    /// A sidebar section title; a click folds the section away or back.
    fn section(&self, key: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        let folded = self.collapsed.contains(key);
        div().id(SharedString::from(format!("section-{key}"))).mx_1().px_2().pt_3().pb_1().flex().items_center().gap_1()
            .rounded_sm().cursor_pointer().group("section")
            .text_size(px(12.)).text_color(rgb(TEXT_MUTED)).font_weight(gpui::FontWeight::SEMIBOLD)
            .hover(|style| style.text_color(rgb(TEXT)))
            .child(div().flex_1().child(key))
            .child(svg().path(if folded { "fm/chevron-right.svg" } else { "fm/chevron-down.svg" }).size(px(12.))
                .text_color(rgb(TEXT_DIM)).when(!folded, |chevron| chevron.invisible().group_hover("section", |style| style.visible())))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(key, cx)))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.pane_path(self.active_side());
        let all: Vec<Place> = self.places.iter().chain(self.drives.iter()).cloned().collect();
        let active_place = containing_place(&all, &current).map(|place| place.path.clone());
        let mut side = div().id("sidebar").flex_1().min_h_0().flex().flex_col().pb_3()
            .overflow_y_scrollbar().text_size(px(13.))
            .child(self.section("Места", cx)
                .on_mouse_down(MouseButton::Right,cx.listener(|this,event: &MouseDownEvent,_,cx|this.open_menu(event.position,MenuTarget::Place(None),cx))));
        let open = |key: &str| !self.collapsed.contains(key);
        // Places are edited like favourites: right click hides, dragging reorders.
        for (index, place) in self.places.iter().enumerate().filter(|_| open("Места")) {
            let active = active_place.as_ref() == Some(&place.path);
            let drag_name = place.label.clone();
            side = side.child(self.place_row(("place", index), place, active, cx)
                .on_mouse_down(MouseButton::Right,cx.listener(move |this,event: &MouseDownEvent,_,cx|this.open_menu(event.position,MenuTarget::Place(Some(index)),cx)))
                .on_drag(PlaceDrag {index},move |_: &PlaceDrag,position,_,cx|cx.new(|_|FileDragPreview {name:drag_name.clone(),position}))
                .drag_over::<PlaceDrag>(|style,_,_,_|style.border_t_1().border_color(rgb(ACCENT)))
                .on_drop(cx.listener(move |this,data: &PlaceDrag,_,cx|this.move_place(data.index,index,cx))));
        }
        if self.places.is_empty() && open("Места") {
            side = side.child(self.sidebar_action("places-restore","fm/plus.svg","Вернуть стандартные места",
                cx.listener(|this,_,_,cx|this.show_all_places(cx))));
        }
        side = side.child(self.section("Избранное", cx));
        for (index,path) in self.favorites.iter().enumerate().filter(|_| open("Избранное")) {
            let target=path.clone();let tip=path.display().to_string();let name=browser::display_name(path);let drag_name=name.clone();
            side=side.child(div().id(("favorite",index)).mx_2().h(px(28.)).px_2().flex().items_center().gap_2()
                .rounded_sm().cursor_pointer().text_color(rgb(TEXT_MUTED)).hover(|style|style.bg(rgb(HOVER)))
                .child(small_icon("fm/folder-fill.svg",FOLDER)).child(div().flex_1().min_w_0().truncate().child(name))
                .tooltip(move |window,cx|Tooltip::new(tip.clone()).build(window,cx))
                .on_click(cx.listener(move |this,_,_,cx|this.navigate_side(this.active_side(),&target,None,cx)))
                .on_mouse_down(MouseButton::Right,cx.listener(move |this,event: &MouseDownEvent,_,cx|this.open_menu(event.position,MenuTarget::Favorite(index),cx)))
                .on_drag(FavoriteDrag {index},move |_: &FavoriteDrag,position,_,cx|cx.new(|_|FileDragPreview {name:drag_name.clone(),position}))
                .drag_over::<FavoriteDrag>(|style,_,_,_|style.border_t_1().border_color(rgb(ACCENT)))
                .on_drop(cx.listener(move |this,data: &FavoriteDrag,_,cx|this.move_favorite(data.index,index,cx))));
        }
        if !self.favorites.contains(&current) && open("Избранное") {
            side=side.child(self.sidebar_action("favorite-add","fm/plus.svg","Добавить папку",
                cx.listener(|this,_,_,cx| {let path=this.pane_path(this.active_side());this.toggle_favorite(&path,cx);})));
        }
        let recent = self.recent_rows();
        if !recent.is_empty() {
            side = side.child(self.section("Недавние", cx));
            for (index, path) in recent.into_iter().enumerate().filter(|_| open("Недавние")) {
                let target = path.clone(); let tip = path.display().to_string(); let menu_path = path.clone();
                side = side.child(div().id(("recent", index)).mx_2().h(px(28.)).px_2().flex().items_center().gap_2()
                    .rounded_sm().cursor_pointer().text_color(rgb(TEXT_MUTED)).hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .child(small_icon("fm/history.svg", TEXT_DIM))
                    .child(div().flex_1().min_w_0().truncate().child(browser::display_name(&path)))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate_side(this.active_side(), &target, None, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx|
                        this.open_menu(event.position, MenuTarget::Recent(menu_path.clone()), cx))));
            }
        }
        side = side.child(self.section("Диски", cx));
        for (index, place) in self.drives.iter().enumerate().filter(|_| open("Диски")) {
            let active = active_place.as_ref() == Some(&place.path);
            side = side.child(self.place_row(("drive", index), place, active, cx));
            if let Some(&(free, total)) = self.drive_space.get(&place.path) {
                let used = 1. - free as f32 / total as f32;
                let color = if used > 0.9 { DANGER } else { TEXT_DIM };
                side = side.child(
                    div().mx_2().pl(px(32.)).pr_2().pb_1().flex().flex_col().gap_1()
                        .child(div().h(px(4.)).w_full().rounded_full().bg(rgb(BORDER))
                            .child(div().h_full().rounded_full().bg(rgb(color)).w(gpui::relative(used))))
                        .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                            .child(format!("свободно {} из {}", format_size(free), format_size(total))))
                );
            }
        }
        side = side.child(div().mt_3().child(self.drop_zone(cx)));
        if !self.workspace_names.is_empty() {
            side = side.child(self.section("Проекты", cx));
        }
        for (index, name) in self.workspace_names.iter().enumerate().filter(|_| open("Проекты")) {
            let open_name = name.clone();
            let delete_name = name.clone();
            side = side.child(
                div().id(("workspace", index)).group("workspace").mx_2().h(px(28.)).px_2().flex().items_center()
                    .gap_2().rounded_md().cursor_pointer().text_color(rgb(TEXT_MUTED))
                    .when(self.browser.project_group(name).is_some(),|row|row.bg(rgb(ACCENT_SOFT)).text_color(rgb(TEXT)))
                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .child(icon("fm/layers.svg", TEXT_MUTED))
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .child(div().id(("workspace-delete", index)).size(px(20.)).rounded_sm().flex()
                        .items_center().justify_center().invisible().group_hover("workspace", |style| style.visible())
                        .hover(|style| style.bg(rgb(PRESSED)))
                        .child(small_icon("fm/x.svg", TEXT_DIM))
                        .tooltip(|window, cx| Tooltip::new("Удалить проект").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_workspace(&delete_name, cx);
                        })))
                    .on_click(cx.listener(move |this, _, _, cx| this.restore_workspace(&open_name, cx)))
            );
        }
        side = side
            .child(self.sidebar_action("save-workspace", "fm/plus.svg", "Создать проект…",
                cx.listener(|this, _, window, cx| this.begin_save_workspace(window, cx))))
            .child(self.sidebar_action("operation-review", "fm/history.svg", "Журнал операций",
                cx.listener(|this, _, _, cx| this.toggle_operation_review(cx))));
        // Full window height like Atlas; its header also drags the window.
        div().w(px(SIDEBAR_WIDTH)).flex_none().h_full().flex().flex_col().bg(rgb(PANEL))
            .border_r_1().border_color(rgb(BORDER))
            .child(div().h(px(32.)).flex_none().pl_3().pr_2().flex().items_center().gap_2()
                .when(cfg!(target_os="macos"),|bar|bar.pl(px(80.)))
                .child(div().flex_1().min_w_0().h_full().flex().items_center().truncate().text_size(px(13.)).text_color(rgb(TEXT_MUTED))
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left,|_,window,_| {if cfg!(any(target_os="linux",target_os="macos")) {window.start_window_move();}})
                    .child("Papka"))
                .child(icon_button("toggle-sidebar", "fm/panel-left.svg", "Скрыть боковую панель (Ctrl+B)", false, true)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_sidebar = false;
                        cx.notify();
                    }))))
            .child(side)
            .into_any_element()
    }

    fn sidebar_action(
        &self,
        id: &'static str,
        icon_path: &'static str,
        label: &'static str,
        click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div().id(id).mx_2().h(px(28.)).px_2().flex().items_center().gap_2().rounded_md()
            .cursor_pointer().text_color(rgb(TEXT_MUTED))
            .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
            .child(icon(icon_path, TEXT_MUTED))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
            .on_click(click)
    }

    fn drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.zone.items();
        let mut zone = div().id("drop-zone").mx_2().p_2().rounded_md().flex().flex_col().gap_1()
            .when(!items.is_empty(), |zone| zone.border_1().border_color(rgb(BORDER)).bg(rgb(SURFACE)))
            .drag_over::<FileDragInfo>(|style, _, _, _| style.border_color(rgb(ACCENT)).bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener(|this, data: &FileDragInfo, _, cx| this.stage_paths(&data.paths, cx)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.border_color(rgb(ACCENT)).bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener(|this, data: &ExternalPaths, _, cx| this.stage_paths(data.paths(), cx)));
        if items.is_empty() {
            zone = zone.child(
                div().flex().items_center().gap_2().text_color(rgb(TEXT_MUTED))
                    .text_size(px(12.))
                    .child(icon("fm/inbox.svg", TEXT_MUTED))
                    .child(div().flex_1().min_w_0().truncate().child("Файлы для переноса"))
            );
            return zone.tooltip(|window, cx| Tooltip::new("Перетащите сюда файлы, затем откройте папку назначения. Файлы не изменятся до команды копирования или переноса.").build(window, cx));
        }
        // Staged items are dragged out like files: the header carries all,
        // a row carries one, into any folder, pane or program.
        let preview = |info: &FileDragInfo, position, _: &mut Window, cx: &mut App| {
            let name = match info.paths.as_slice() { [single] => browser::display_name(single), many => format!("{} элементов", many.len()) };
            cx.new(|_| FileDragPreview { name, position })
        };
        zone = zone.child(div().id("zone-all").flex().items_center().gap_1().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
            .cursor_grab().rounded_sm().hover(|style| style.bg(rgb(HOVER)))
            .child(div().flex_1().child(format!("Для переноса: {}", items.len())))
            .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child("тяните"))
            .tooltip(|window, cx| Tooltip::new("Перетащите в папку или панель: тот же диск — перенос, другой — копия; Ctrl — копия, Shift — перенос").build(window, cx))
            .on_drag(FileDragInfo { paths: items.to_vec() }, preview));
        for (index, path) in items.iter().take(5).enumerate() {
            zone = zone.child(
                div().id(("zone-item", index)).flex().items_center().gap_2().text_size(px(12.)).text_color(rgb(TEXT))
                    .cursor_grab().rounded_sm().hover(|style| style.bg(rgb(HOVER)))
                    .child(small_icon(if path.is_dir() { "fm/folder-fill.svg" } else { "fm/file.svg" },
                        if path.is_dir() { FOLDER } else { TEXT_MUTED }))
                    .child(div().flex_1().min_w_0().truncate().child(browser::display_name(path)))
                    .on_drag(FileDragInfo { paths: vec![path.clone()] }, preview)
            );
        }
        if items.len() > 5 {
            zone = zone.child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                .child(format!("и ещё {}", items.len() - 5)));
        }
        zone.child(
            div().pt_1().flex().flex_col().gap_1()
                .child(text_button("zone-copy", "Копировать сюда", ButtonKind::Primary).w_full().text_size(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| this.paste(cx))))
                .child(text_button("zone-move", "Переместить сюда", ButtonKind::Ghost).w_full().text_size(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| this.move_staged(cx))))
        ).child(
            div().id("zone-clear").text_size(px(12.)).text_color(rgb(TEXT_DIM)).cursor_pointer()
                .hover(|style| style.text_color(rgb(TEXT)))
                .child("Очистить")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.zone.clear();
                    this.status = "Отложенные файлы убраны; сами файлы не изменены".into();
                    cx.notify();
                }))
        )
    }

    // --------------------------------------------------------------- panes

    fn pane_width_for(&self, side: GroupId) -> f32 {
        crate::layout::effective_pane_width(self.pane_width, self.pane_measured_widths.get(&side).copied())
    }

    /// Folders shown in a pane: the trail of the current folder and, in
    /// columns mode, a preview of the selected subfolder.
    pub(crate) fn pane_columns(&self, side: GroupId) -> Vec<PathBuf> {
        let path = self.pane_path(side);
        if !self.miller(side) {
            return vec![path];
        }
        // A marquee reaching a folder must not open its preview: the columns
        // would shift under the gesture and cancel it.
        let marquee_here = self.marquee.as_ref().is_some_and(|m| m.side == side);
        let preview = (side == self.active_side() && !marquee_here).then(|| self.selected.clone()).flatten()
            .filter(|selected| {
                selected.parent() == Some(path.as_path())
                    && self.directory_cache.get(&path).is_some_and(|listing| {
                        listing.entries.iter().any(|entry| &entry.path == selected && entry.is_directory)
                    })
            });
        // A narrow viewport may omit the optional preview, never the trail.
        let fit = crate::layout::column_capacity(self.pane_width_for(side));
        let preview = preview.filter(|_| fit > 1);
        crate::layout::column_paths(&path, preview.as_deref())
    }

    fn pane(&mut self, side: GroupId, cx: &mut Context<Self>) -> AnyElement {

        let path = self.pane_path(side);
        let pane_width = self.pane_width_for(side);
        let body = if self.miller(side) {
            let folders = self.pane_columns(side);
            let current_index = folders.iter().position(|folder| folder == &path).unwrap_or(folders.len() - 1);
            let group_id = format!(
                "miller-{}-{}-{}",
                // This pane's own tab: another pane's tab switch must not reset its columns.
                self.browser.group(side).map_or(0, |group| group.active_tab),
                side.0,
                crate::layout::column_identity(pane_width, folders.len(), current_index),
            );
            let scroll = self.column_scroll_handles.entry(side).or_default().clone();
            let reveal_key = format!("{group_id}-{:?}-{:?}-{}", path, folders.last(), pane_width as u32);
            let widths = crate::layout::column_widths(pane_width, folders.len(), self.column_width_for(side));
            let content_width = px(widths.iter().sum::<f32>().max(pane_width));
            let mut group = div().id(group_id).w(content_width).h_full().flex().flex_row().flex_none();
            for (index, folder) in folders.iter().enumerate() {
                // The folder of the next column is highlighted as the trail.
                let trail = folders.get(index + 1).cloned();
                let column = self.column(folder.clone(), side, trail, cx);
                let weak = cx.weak_entity();
                let reveal_scroll = scroll.clone();
                let reveal_key = reveal_key.clone();
                let reveal_widths = widths.clone();
                let column = div().relative().w(px(widths[index])).h_full().flex_none().child(column)
                    .child(div().id(("column-resize", index)).absolute().right_0().top_0().bottom_0().w(px(6.))
                        .cursor_col_resize().hover(|style| style.bg(rgb(BORDER_STRONG)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if this.interface_modal() { return; }
                            this.column_resize = Some(crate::ColumnResize { group: side, start_x: event.position.x, start_width: this.column_width_for(side) });
                            this.marquee = None;
                        })))
                    .when(index == current_index, |column| column.on_prepaint(move |bounds, _, cx| {
                        let reveal_scroll = reveal_scroll.clone();
                        let reveal_key = reveal_key.clone();
                        let weak = weak.clone();
                        let reveal_widths = reveal_widths.clone();
                        cx.defer(move |cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if this.column_reveal_keys.get(&side) == Some(&reveal_key) {
                                    return;
                                }
                                let viewport = reveal_scroll.bounds();
                                if viewport.size.width <= px(0.) { return; }
                                let offset = reveal_scroll.offset().x / px(1.);
                                let start = (bounds.origin.x - viewport.origin.x) / px(1.) - offset;
                                let next = crate::layout::reveal_column_offset(&reveal_widths, current_index,
                                    viewport.size.width / px(1.), start, bounds.size.width / px(1.), offset)
                                    .min(0.).max(-reveal_scroll.max_offset().x / px(1.));
                                this.column_reveal_keys.insert(side,reveal_key);
                                reveal_scroll.set_offset(gpui::point(px(next), px(0.)));
                                cx.notify();
                            });
                        });
                    }));
                group = group.child(column);
            }
            // Keep the scrollbar outside the content that it scrolls.
            div().relative().flex_1().min_w_0().min_h_0().overflow_hidden().flex()
                .child(column_scroll_area(gpui::SharedString::from(format!("columns-{}",side.0)), &scroll).child(group))
                .child(div().absolute().inset_0().child(gpui_component::scroll::Scrollbar::horizontal(&scroll)))
                .into_any_element()
        } else {
            div().flex_1().min_h_0().flex().flex_col()
                .child(self.list_header(side, cx))
                .child(self.column(path.clone(), side, None, cx))
                .into_any_element()
        };
        let weak = cx.weak_entity();
        div().relative().size_full().flex().flex_col().overflow_hidden().bg(rgb(SURFACE))
            .on_prepaint(move |bounds, _, cx| {
                let width = bounds.size.width / px(1.);
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.pane_bounds.insert(side,bounds);
                        if width > 0. && this.pane_measured_widths.get(&side).is_none_or(|old| (*old - width).abs() >= 1.) {
                            this.pane_measured_widths.insert(side,width);
                            cx.notify();
                        }
                    });
                });
            })
            .child(body)
            .drag_over::<TabDrag>(|style,_,_,_|style)
            .on_drag_move::<TabDrag>(cx.listener(move |this,event: &gpui::DragMoveEvent<TabDrag>,_,cx| {
                let Some(zone)=crate::layout::dock_hit((event.event.position.x-event.bounds.origin.x)/px(1.),
                    (event.event.position.y-event.bounds.origin.y)/px(1.),event.bounds.size.width/px(1.),event.bounds.size.height/px(1.)) else {
                    if this.dock_hover.is_some_and(|(id,_)|id==side) {this.dock_hover=None;cx.notify();}
                    return;
                };
                if this.dock_hover!=Some((side,zone)) {this.dock_hover=Some((side,zone));cx.notify();}
            }))
            .on_drop(cx.listener(move |this,drag: &TabDrag,window,cx| {
                use crate::layout::DockZone::*;
                let zone=this.dock_hover.take().filter(|(id,_)|*id==side).map(|(_,zone)|zone).unwrap_or(Center);
                // Its own pane's centre: the tab is already there.
                if zone==Center && drag.group==side {cx.notify();return;}
                if zone==Center || this.browser.groups.len()>=filemanager_core::panes::MAX_GROUPS {
                    this.move_tab_to(drag,side,usize::MAX,cx);
                    if zone!=Center {this.status="Не больше четырёх панелей".into();}
                } else {
                    let edge=match zone {Left=>Edge::Left,Right=>Edge::Right,Top=>Edge::Top,Bottom=>Edge::Bottom,Center=>unreachable!()};
                    this.split_group(side,edge,Some((drag.group,drag.index)),window,cx);
                }
                cx.notify();
            }))
            .when_some(self.dock_hover.filter(|(id,_)|*id==side).map(|(_,zone)|zone),|pane,zone| {
                use crate::layout::DockZone::*;
                let overlay=div().absolute().bg(crate::theme::accent_alpha(0x22)).border_2().border_color(rgb(ACCENT)).rounded(px(6.));
                let overlay=match zone {
                    Left=>overlay.left_0().top_0().bottom_0().w(gpui::relative(0.5)),
                    Right=>overlay.right_0().top_0().bottom_0().w(gpui::relative(0.5)),
                    Top=>overlay.top_0().left_0().right_0().h(gpui::relative(0.5)),
                    Bottom=>overlay.bottom_0().left_0().right_0().h(gpui::relative(0.5)),
                    Center=>overlay.inset_0(),
                };
                pane.child(overlay)
            })
            // Files from Explorer are copied into the pane's folder.
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener({
                let folder = path.clone();
                move |this, data: &ExternalPaths, _, cx| {
                    this.drop_into(data.paths(), folder.clone(), true, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                this.focus_group(side,cx);
                let handle = this.focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// Narrow panes drop the type and date columns, keeping names readable.
    fn list_columns(&self, side: GroupId) -> (bool, bool) {
        let width = self.pane_width_for(side);
        (width >= 520., width >= 680.)
    }

    fn list_header(&self, side: GroupId, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = self.sort_for(side);
        let (show_modified, show_kind) = self.list_columns(side);
        let header = |id: &'static str, label: &'static str, key: SortKey, cx: &mut Context<Self>| {
            let active = spec.key == key;
            div().id(id).h_full().px_2().flex().items_center().gap_1().cursor_pointer()
                .text_color(rgb(if active { TEXT } else { TEXT_DIM }))
                .hover(|style| style.text_color(rgb(TEXT)))
                .child(label)
                .when(active, |this| this.child(if spec.descending { "↓" } else { "↑" }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // Each pane sorts on its own.
                    let next = this.sort_for(side).toggled(key);
                    this.sorts.insert(side, next);
                    cx.notify();
                }))
        };
        let widths = self.list_widths_for(side);
        // A fixed column with a grip on its left border: dragging it left
        // widens the column, as in Explorer; the name column takes the rest.
        let sized = |index: usize, cell: Stateful<Div>, cx: &mut Context<Self>| {
            div().relative().h_full().w(px(widths[index])).flex_none()
                .child(cell.size_full())
                .child(div().id(("list-resize", index)).absolute().top_0().bottom_0().left(px(-5.)).w(px(9.))
                    .cursor_col_resize().flex().justify_center()
                    .child(div().w(px(1.)).h_full().bg(rgb(BORDER)))
                    .hover(|style| style.bg(rgb(HOVER)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.list_resize = Some((side, index, event.position.x, this.list_widths_for(side)[index]));
                    })))
        };
        div().h(px(28.)).flex_none().flex().items_center().gap_2().pl(px(36.)).pr(px(12.))
            .border_b_1().border_color(rgb(BORDER)).text_size(px(12.)).bg(rgb(SURFACE))
            .child(header("sort-name", "Имя", SortKey::Name, cx).flex_1().min_w_0())
            .when(show_modified, |this| { let cell = header("sort-modified", "Изменён", SortKey::Modified, cx); this.child(sized(0, cell, cx)) })
            .when(show_kind, |this| { let cell = header("sort-kind", "Тип", SortKey::Kind, cx); this.child(sized(1, cell, cx)) })
            .child({ let cell = header("sort-size", "Размер", SortKey::Size, cx).justify_end(); sized(2, cell, cx) })
    }

    fn column(&mut self, folder: PathBuf, side: GroupId, trail: Option<PathBuf>, cx: &mut Context<Self>) -> AnyElement {
        let list_mode = !self.miller(side);
        if list_mode { self.auto_folder_sizes(&folder, cx); }
        let mut column = div().size_full().min_h_0().flex().flex_col()
            .when(!list_mode, |this| this.border_r_1().border_color(rgb(BORDER)));
        if !list_mode {
            let current = folder == self.pane_path(side);
            column = column.child(div().h(px(ROW_HEIGHT)).flex_none().px_2().flex().items_center()
                .border_b_1().border_color(rgb(BORDER)).text_size(px(12.))
                .text_color(rgb(if current { TEXT } else { TEXT_MUTED }))
                .child(div().min_w_0().truncate().child(browser::display_name(&folder))));
        }
        let background_folder = folder.clone();
        let drop_folder = folder.clone();
        let hover_folder = folder.clone();
        let right_target = self.right_drag.as_ref().and_then(|drag| drag.target.as_ref()) == Some(&folder);
        column = column.on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
            // Shift+right click on empty space: Explorer's own folder menu.
            if event.modifiers.shift {
                this.explorer_folder_menu(background_folder.clone(), window, cx);
            } else {
                this.open_menu(event.position, MenuTarget::Folder(background_folder.clone()), cx);
            }
        }))
            // Files dropped on the empty space or a file row land in this folder.
            .drag_over::<FileDragInfo>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener(move |this, data: &FileDragInfo, window, cx| {
                this.drop_files(&data.paths, drop_folder.clone(), window.modifiers(), cx);
            }))
            .on_mouse_move(cx.listener(move |this, _, _, _| {
                if let Some(drag) = this.right_drag.as_mut().filter(|drag| drag.active && !drag.claimed) {
                    drag.target = Some(hover_folder.clone());
                    drag.claimed = true;
                }
            }))
            .when(right_target, |column| column.bg(rgb(ACCENT_SOFT)));

        if let Some((listing, indices)) = self.folder_view(side, &folder) {
            if indices.is_empty() {
                return column.child(
                    div().flex_1().flex().flex_col().items_center().justify_center().gap_2()
                        .text_color(rgb(TEXT_DIM)).text_size(px(12.))
                        .child(icon("fm/folder-fill.svg", BORDER_STRONG))
                        .child(if listing.entries.is_empty() {
                            "Папка пуста".to_string()
                        } else if !self.filter.is_empty() && folder == self.pane_path(self.active_side()) {
                            format!("Нет имён, содержащих «{}». Enter — искать во вложенных папках", self.filter)
                        } else {
                            "Только скрытые файлы".to_string()
                        })
                ).into_any_element();
            }
            let handle = self.scroll_handle(side, &folder);
            let count = indices.len();
            let list_id = SharedString::from(format!("list-{}-{}", side.0, folder.display()));
            let background_marquee_folder=folder.clone();
            column = column.child(
                div().flex_1().min_h_0().child(
                    uniform_list(list_id, count, cx.processor({ let listing = listing.clone(); move |this, range: Range<usize>, _, cx| {
                        range.map(|position| {
                            let entry = &listing.entries[indices[position]];
                            let on_trail = trail.as_ref() == Some(&entry.path);
                            this.row(entry, &folder, side, on_trail, list_mode, list_mode && position % 2 == 1, cx)
                        }).collect::<Vec<_>>()
                    }}))
                    .track_scroll(&handle)
                    .h_full()
                    .pl_1().pr(px(12.))
                    .on_mouse_down(MouseButton::Left,cx.listener({ let folder=background_marquee_folder.clone(); move |this,event,window,cx|this.begin_marquee(&folder,side,event,window,cx) }))
                )
            );
            if self.unwatched_folders.contains(&background_marquee_folder) {
                column = column.child(
                    div().px_3().py_1().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                        .child("Без автообновления — F5 обновит")
                );
            }
            if listing.truncated {
                column = column.child(
                    div().px_3().py_1().text_size(px(11.)).text_color(rgb(WARNING))
                        .child("Показаны не все элементы: нет доступа или превышен лимит")
                );
            }
        } else if let Some(error) = self.directory_errors.get(&folder) {
            column = column.child(
                div().p_4().flex().flex_col().gap_2().text_size(px(12.))
                    .child(div().flex().items_center().gap_2().text_color(rgb(DANGER))
                        .child(icon("fm/alert.svg", DANGER)).child("Не удалось открыть папку"))
                    .child(div().text_color(rgb(TEXT_MUTED)).child(error.clone()))
                    .child(div().text_color(rgb(TEXT_DIM)).child("F5 — повторить"))
            );
        } else {
            column = column.child(
                div().p_4().flex().items_center().gap_2().text_color(rgb(TEXT_DIM)).text_size(px(12.))
                    .child(icon("fm/loader.svg", TEXT_DIM).with_animation(
                        "loading-spinner",
                        Animation::new(Duration::from_millis(900)).repeat(),
                        |svg, delta| svg.with_transformation(gpui::Transformation::rotate(gpui::percentage(delta))),
                    ))
                    .child("Загрузка…")
            );
        }
        column.into_any_element()
    }

    fn row(&self, entry: &Entry, folder: &Path, side: GroupId, on_trail: bool, list_mode: bool, striped: bool, cx: &mut Context<Self>) -> AnyElement {
        let path = entry.path.clone();
        let is_dir = entry.is_directory;
        let selected = self.is_selected(&path);
        let focused_pane = side == self.active_side();
        let (icon_path, tint) = entry_icon(entry);
        let stack = self.stack_role(side, folder, &path);
        let bg = if selected && focused_pane {
            Some(SELECTED)
        } else if selected {
            Some(INACTIVE_SELECTED)
        } else if on_trail {
            Some(TRAIL)
        } else {
            None
        };
        let base = if striped { STRIPE } else { SURFACE };
        let right_target = is_dir && self.right_drag.as_ref().and_then(|drag| drag.target.as_ref()) == Some(&path);
        let bg = if right_target && !selected { Some(ACCENT_SOFT) } else { bg };
        let text = if selected { SELECTION_TEXT } else if entry.hidden { TEXT_MUTED } else { TEXT };
        let metadata_text = if selected { SELECTION_TEXT } else { TEXT_MUTED };
        let tint = if selected { SELECTION_TEXT } else { tint };
        let mut row = div().id(SharedString::from(format!("row-{}", path.display())))
            .w_full().h(px(ROW_HEIGHT)).pl(px(6.)).pr_2().flex().items_center().gap_2()
            .border_l_2().border_color(rgb(if selected && focused_pane { ACCENT } else { bg.unwrap_or(base) }))
            .text_size(px(13.)).text_color(rgb(text)).cursor_pointer()
            .when_some(bg, |this, bg| this.bg(rgb(bg)))
            .when(bg.is_none() && striped, |this| this.bg(rgb(STRIPE)))
            .when(bg.is_none(), |this| this.hover(|style| style.bg(rgb(HOVER))))
            // An older version sits indented under the newest one.
            .when(matches!(stack, Some(Stacked::Member { .. })), |row| row.child(div().w(px(16.)).flex_none()))
            .child(match self.system_icon(&path).filter(|_| !is_dir) {
                // The program's own icon, as in Explorer.
                Some(image) => img(image).size(px(16.)).flex_none().when(entry.hidden && !selected, |image| image.opacity(0.55)).into_any_element(),
                None => icon(icon_path, tint).when(entry.hidden && !selected, |svg| svg.opacity(0.55)).into_any_element(),
            });
        // F2 edits the name in place; Enter applies, Esc or a click elsewhere cancels.
        row = if self.renaming && side == self.active_side() && self.selected.as_ref() == Some(&path) {
            // A compact field that fits the 24 px row and starts where the name does.
            row.child(div().flex_1().min_w_0().h(px(ROW_HEIGHT)).flex().items_center().ml(px(-5.))
                .child(Input::new(&self.rename_input).xsmall().h(px(20.)).w_full().text_size(px(13.))))
        } else {
            let badge = |text: String| div().flex_none().px_1p5().rounded_sm().text_size(px(11.))
                .bg(rgb(if selected { SELECTED_BORDER } else { ACCENT_SOFT })).text_color(rgb(if selected { SELECTION_TEXT } else { TEXT_MUTED }))
                .child(text);
            let name = div().flex_1().min_w_0().flex().items_center().gap_1p5()
                .child(div().min_w_0().truncate().child(entry.name.clone()));
            let name = match &stack {
                Some(Stacked::Head { key, label, others, expanded }) => {
                    let (folder_key, key) = (folder.to_path_buf(), key.clone());
                    let expanded = *expanded;
                    name.child(div().id(SharedString::from(format!("stack-{}", path.display())))
                        .flex_none().flex().items_center().gap_0p5().cursor_pointer()
                        .tooltip(move |window, cx| Tooltip::new(if expanded { "Скрыть прежние версии" } else { "Показать прежние версии" }).build(window, cx))
                        .child(badge(format!("{label} +{others}")))
                        .child(small_icon(if expanded { "fm/chevron-down.svg" } else { "fm/chevron-right.svg" }, if selected { SELECTION_TEXT } else { TEXT_DIM }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_stack(&folder_key, &key, cx);
                        })))
                }
                Some(Stacked::Member { label, .. }) => name.child(badge(label.clone())),
                None => name,
            };
            row.child(name)
        };
        if list_mode {
            let (show_modified, show_kind) = self.list_columns(side);
            let widths = self.list_widths_for(side);
            // Cells never wrap: a narrow column cuts its text with "…".
            row = row
                .when(show_modified, |row| row.child(div().w(px(widths[0])).flex_none().px_2().truncate().text_size(px(12.)).text_color(rgb(metadata_text))
                    .child(entry.modified.map(format_time).unwrap_or_default())))
                .when(show_kind, |row| row.child(div().w(px(widths[1])).flex_none().px_2().truncate().text_size(px(12.)).text_color(rgb(metadata_text))
                    .child(kind_label(entry))))
                .child(div().w(px(widths[2])).flex_none().px_2().flex().justify_end().text_size(px(12.))
                    .text_color(rgb(metadata_text))
                    .child(div().min_w_0().truncate()
                        .child(if is_dir { self.folder_size_label(&entry.path) } else { format_size(entry.size) })));
        } else if is_dir {
            row = row.child(small_icon("fm/chevron-right.svg", if selected { SELECTION_TEXT } else { TEXT_DIM }));
        }
        if is_dir {
            let internal_target = path.clone();
            let external_target = path.clone();
            row = row
                .drag_over::<FileDragInfo>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)).border_1().border_color(rgb(ACCENT)))
                .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)).border_1().border_color(rgb(ACCENT)))
                // Inside the app: Explorer's rule (see drop_files); from Explorer it copies.
                .on_drop(cx.listener(move |this, data: &FileDragInfo, window, cx| {
                    cx.stop_propagation();
                    this.drop_files(&data.paths, internal_target.clone(), window.modifiers(), cx);
                }))
                .on_mouse_move(cx.listener({ let target = path.clone(); move |this, _, _, cx| {
                    if let Some(drag) = this.right_drag.as_mut().filter(|drag| drag.active && !drag.claimed) {
                        drag.target = Some(target.clone());
                        drag.claimed = true;
                    }
                    this.spring_hover(crate::spring::SpringTarget::Folder(side, target.clone()), cx);
                }}))
                .on_drop(cx.listener(move |this, data: &ExternalPaths, _, cx| {
                    cx.stop_propagation();
                    this.drop_into(data.paths(), external_target.clone(), true, cx);
                }));
        }
        let column_folder = folder.to_path_buf();
        let menu_path = path.clone();
        let menu_folder = folder.to_path_buf();
        // While the name is edited the mouse belongs to the text field:
        // pressing and moving selects text, it must not drag the file or
        // start a frame selection in the column.
        if self.renaming && side == self.active_side() && self.selected.as_ref() == Some(&path) {
            return row.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).into_any_element();
        }
        // Dragging a selected row carries the whole selection.
        let drag_paths = if selected { self.selection() } else { vec![path.clone()] };
        row.on_drag(FileDragInfo { paths: drag_paths }, |info: &FileDragInfo, position, _, cx| {
                let name = match info.paths.as_slice() {
                    [single] => browser::display_name(single),
                    many => format!("{} элементов", many.len()),
                };
                cx.new(|_| FileDragPreview { name, position })
            })
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                // Right-click inside the selection keeps it for batch actions.
                if !this.is_selected(&menu_path) {
                    this.navigate_side(side, &menu_folder, Some(menu_path.clone()), cx);
                }
                // As in Explorer the menu opens on release; dragging instead
                // ends in the copy/move menu (see the root mouse handlers).
                let paths = if this.is_selected(&menu_path) { this.selection() } else { vec![menu_path.clone()] };
                this.context_menu = None;
                this.right_drag = Some(crate::RightDrag {
                    paths, start: event.position, position: event.position, active: false,
                    target: None, claimed: false, click_menu: MenuTarget::Entry(menu_path.clone()),
                });
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if this.marquee.is_some() {cx.stop_propagation();return;}
                if event.click_count() >= 2 {
                    this.open_entry(path.clone(), is_dir, side, cx);
                } else {
                    this.click_entry(side, &column_folder, path.clone(), event.modifiers(), cx);
                }
            }))
            .into_any_element()
    }

    fn search_results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use filemanager_core::find::Kind;
        use crate::search_panel::{DEPTHS, PERIODS, SYNTAX_HINT};
        let root = self.search_root.clone().unwrap_or_default();
        let chip = |id: &'static str, label: &'static str, active: bool| {
            div().id(id).h(px(24.)).px_2().flex().items_center().gap_1().rounded_md().cursor_pointer()
                .text_size(px(12.)).border_1()
                .when(active, |chip| chip.bg(rgb(ACCENT_SOFT)).border_color(rgb(ACCENT)).text_color(rgb(TEXT)))
                .when(!active, |chip| chip.border_color(rgb(BORDER)).text_color(rgb(TEXT_MUTED))
                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT))))
                .child(label)
        };
        let kind = self.search.kind;
        let busy = self.search_busy;
        let title = div().flex().items_center().gap_2()
            .child(icon("fm/search.svg", ACCENT))
            .child(div().flex_1().min_w_0().truncate().font_weight(gpui::FontWeight::SEMIBOLD)
                .child(format!("«{}» в папке «{}»", self.search_query.trim(), browser::display_name(&root))))
            .child(if busy {
                text_button("search-stop", "Остановить", ButtonKind::Ghost)
                    .on_click(cx.listener(|this, _, _, cx| { this.stop_search(); cx.notify(); }))
            } else {
                text_button("search-again", "Искать снова", ButtonKind::Ghost)
                    .on_click(cx.listener(|this, _, _, cx| this.run_search(cx)))
            })
            .child(icon_button("search-close", "fm/x.svg", "Закрыть поиск (Esc)", false, true)
                .on_click(cx.listener(|this, _, window, cx| this.key_dismiss(&keys::DismissOverlay, window, cx))));
        let filters = div().flex().flex_wrap().items_center().gap_1()
            .child(chip("search-any", "Всё", kind == Kind::Any)
                .on_click(cx.listener(|this, _, _, cx| this.set_search_kind(Kind::Any, cx))))
            .child(chip("search-files", "Файлы", kind == Kind::Files)
                .on_click(cx.listener(|this, _, _, cx| this.set_search_kind(Kind::Files, cx))))
            .child(chip("search-folders", "Папки", kind == Kind::Folders)
                .on_click(cx.listener(|this, _, _, cx| this.set_search_kind(Kind::Folders, cx))))
            .child(div().w_2())
            .child(chip("search-period", PERIODS[self.search.period].0, self.search.period > 0)
                .tooltip(|window, cx| Tooltip::new("Дата изменения: щёлкните, чтобы сменить").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.cycle_search_period(cx))))
            .child(chip("search-depth", DEPTHS[self.search.depth].0, self.search.depth > 0)
                .tooltip(|window, cx| Tooltip::new("Сколько уровней вложенных папок просматривать: щёлкните, чтобы сменить").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.cycle_search_depth(cx))))
            .child(chip("search-content", "И в тексте файлов", self.search.content)
                .tooltip(|window, cx| Tooltip::new("Искать слова ещё и внутри текстовых файлов (txt, csv, xml, lsp, dxf…)").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_search_content(cx))))
            .child(div().id("search-help").ml_1().child(small_icon("fm/help.svg", TEXT_DIM))
                .tooltip(|window, cx| Tooltip::new(SYNTAX_HINT).build(window, cx)));
        let summary = div().flex().items_center().gap_2().text_size(px(12.)).text_color(rgb(TEXT_DIM))
            .when(busy, |row| row.child(small_icon("fm/loader.svg", ACCENT)
                .with_animation("search-spin", Animation::new(Duration::from_secs(1)).repeat(),
                    |icon, delta| icon.with_transformation(gpui::Transformation::rotate(gpui::percentage(delta))))))
            .child(self.search_summary());
        let header = div().flex_none().px_3().py_2().flex().flex_col().gap_2().border_b_1().border_color(rgb(BORDER))
            .child(title).child(filters).child(summary);
        let columns = div().flex_none().h(px(26.)).px_3().flex().items_center().gap_2().text_size(px(12.))
            .text_color(rgb(TEXT_MUTED)).border_b_1().border_color(rgb(BORDER))
            .child(div().w(px(16.)).flex_none())
            .child(div().w(gpui::relative(0.38)).flex_none().child("Имя"))
            .child(div().flex_1().min_w_0().child("Папка"))
            .child(div().w(px(84.)).flex_none().flex().justify_end().child("Размер"))
            .child(div().w(px(128.)).flex_none().child("Изменён"))
            .child(div().w(px(20.)).flex_none());
        let count = self.search.hits.len();
        let list = uniform_list("search-hits", count, cx.processor(move |this, range: Range<usize>, _, cx| {
            let root = this.search_root.clone().unwrap_or_default();
            range.map(|index| {
                let hit = &this.search.hits[index];
                let selected = this.search.selected == Some(index);
                let folder = hit.path.parent().map(|parent| parent.strip_prefix(&root).unwrap_or(parent).display().to_string())
                    .filter(|relative| !relative.is_empty()).unwrap_or_else(|| "·".into());
                let (icon_path, tint) = if hit.is_dir { ("fm/folder-fill.svg", FOLDER) }
                    else { file_icon(hit.path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref()) };
                let text = if selected { SELECTION_TEXT } else { TEXT };
                let dim = if selected { SELECTION_TEXT } else { TEXT_DIM };
                let reveal = hit.path.clone();
                div().id(("search-hit", index)).group("hit").w_full().h(px(ROW_HEIGHT)).px_2().flex().items_center().gap_2()
                    .rounded_md().cursor_pointer().text_color(rgb(text))
                    .when(selected, |row| row.bg(rgb(SELECTED)))
                    .when(!selected, |row| row.hover(|style| style.bg(rgb(HOVER))))
                    .child(match this.system_icon(&hit.path).filter(|_| !hit.is_dir) {
                        Some(image) => img(image).size(px(16.)).flex_none().into_any_element(),
                        None => icon(icon_path, if selected { SELECTION_TEXT } else { tint }).into_any_element(),
                    })
                    .child(div().w(gpui::relative(0.38)).flex_none().min_w_0().flex().items_center().gap_1()
                        .child(div().min_w_0().truncate().child(browser::display_name(&hit.path)))
                        .when(hit.by_content, |name| name.child(div().flex_none().px_1().rounded_sm().text_size(px(10.))
                            .bg(rgb(if selected { SELECTED_BORDER } else { ACCENT_SOFT })).child("в тексте"))))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(rgb(dim)).child(folder))
                    .child(div().w(px(84.)).flex_none().flex().justify_end().text_size(px(12.)).text_color(rgb(dim))
                        .child(if hit.is_dir { String::new() } else { format_size(hit.size) }))
                    .child(div().w(px(128.)).flex_none().text_size(px(12.)).text_color(rgb(dim))
                        .child(hit.modified.map(format_time).unwrap_or_default()))
                    .child(div().id(("search-reveal", index)).flex_none().size(px(20.)).rounded_sm().flex().items_center()
                        .justify_center().invisible().group_hover("hit", |style| style.visible())
                        .hover(|style| style.bg(rgb(HOVER)))
                        .child(small_icon("fm/external.svg", dim))
                        .tooltip(|window, cx| Tooltip::new("Показать в папке").build(window, cx))
                        .on_click(cx.listener({ let reveal = reveal.clone(); move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.reveal_search_hit(reveal.clone(), false, cx);
                        }})))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        if event.click_count() >= 2 {
                            this.open_search_hit(index, cx);
                        } else {
                            this.search.selected = Some(index);
                            this.status = format!("Выбрано: {}", reveal.display());
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                        this.search.selected = Some(index);
                        cx.notify();
                    }))
            }).collect::<Vec<_>>()
        })).track_scroll(&self.search.scroll).flex_1().min_h_0().px_1();
        div().id("search-results").size_full().min_h_0().flex().flex_col().bg(rgb(SURFACE))
            .child(header)
            .when(count > 0, |panel| panel.child(columns).child(list))
            .when(count == 0 && !busy, |panel| panel.child(
                div().flex_1().flex().flex_col().items_center().justify_center().gap_2().px_6()
                    .text_color(rgb(TEXT_DIM)).text_size(px(12.))
                    .child(icon("fm/search.svg", BORDER_STRONG))
                    .child("Ничего не найдено")
                    .child(div().max_w(px(520.)).text_center().child(SYNTAX_HINT))))
            .into_any_element()
    }

    // ----------------------------------------------------------- inspector

    fn property(label: &'static str, value: impl Into<SharedString>) -> impl IntoElement {
        div().flex().gap_2().text_size(px(12.))
            .child(div().w(px(76.)).flex_none().text_color(rgb(TEXT_MUTED)).child(label))
            .child(div().flex_1().min_w_0().text_color(rgb(TEXT)).child(value.into()))
    }

    fn inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div().id("inspector").size_full().min_w_0().min_h_0().flex().flex_col().gap_3().p_3()
            .overflow_y_scrollbar().bg(rgb(PANEL)).text_size(px(13.));
        if self.document_preview.is_some() || self.preview_launch_busy {
            panel = panel.child(text_button("document-preview-close", "Закрыть предпросмотр", ButtonKind::Ghost)
                .on_click(cx.listener(|this, _, _, cx| this.close_document_preview(cx))));
        }
        if self.operation_review {
            panel = panel.child(div().flex().items_center().justify_between()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Последние операции"))
                    .child(icon_button("close-review", "fm/x.svg", "Закрыть", false, true)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_operation_review(cx)))))
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child("Последние 30 операций"));
            if let Some(control) = &self.copy_control {
                let progress = control.progress();
                panel = panel.child(div().child(format!("Обработано: {}/{}", progress.processed, progress.total)))
                    .child(text_button("queue-pause", if control.is_paused() { "Продолжить" } else { "Пауза" }, ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_copy_pause(cx))))
                    .child(text_button("queue-cancel", "Отменить передачу", ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_copy(cx))));
            }
            if self.operation_alerts.is_empty() {
                panel = panel.child(div().text_color(rgb(TEXT_DIM)).child("Записей пока нет."));
            }
            for entry in &self.operation_alerts {
                panel = panel.child(
                    div().p_2().rounded_md().bg(rgb(RAISED)).flex().flex_col().gap_1().text_size(px(12.))
                        .child(format!("#{} · {} · {}", entry.id, entry.action, entry.status))
                        .child(match entry.phase.as_deref() {
                            Some("move_copying") => "Этап: создание копии",
                            Some("move_verifying_copy") => "Этап: проверка копии",
                            Some("move_recycling_source") => "Этап: перенос исходника в Корзину",
                            Some("undo_restoring_source") => "Этап: восстановление исходника",
                            Some("undo_recycling_copy") => "Этап: перенос копии в Корзину",
                            Some("replace_recycling_old") => "Этап: прежний файл в Корзину",
                            Some("replace_incoming") => "Этап: создание нового файла",
                            Some("replace_verifying_copy") => "Этап: проверка новой копии",
                            Some("undo_replacement_incoming") => "Этап: отмена нового действия",
                            Some("undo_replacement_restore_old") => "Этап: восстановление прежнего файла",
                            _ => "Этап не указан",
                        })
                        .child(entry.error.clone().unwrap_or_default())
                        .child(entry.recovery.clone().unwrap_or_default())
                        .child(div().text_color(rgb(TEXT_MUTED)).child(format!("Откуда: {}", entry.source.display())))
                        .child(div().text_color(rgb(TEXT_MUTED)).child(format!("Куда: {}",
                            entry.destination.as_ref().map(|p| p.display().to_string())
                                .unwrap_or_else(|| "Корзина".into()))))
                );
            }
            return panel.into_any_element();
        }

        if self.marked.len() > 1 {
            let marked = self.marked.clone();
            let mut files = 0usize;
            let mut folders = 0usize;
            let mut bytes = 0u64;
            if let Some(folder) = marked[0].parent().map(Path::to_path_buf) {
                if let Some((listing, _)) = self.folder_view(self.browser.active, &folder) {
                    for entry in listing.entries.iter().filter(|e| marked.contains(&e.path)) {
                        if entry.is_directory { folders += 1 } else { files += 1; bytes += entry.size; }
                    }
                }
            }
            return panel
                .child(div().flex().items_center().gap_2()
                    .child(icon("fm/layers.svg", TEXT_MUTED))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("Выбрано: {}", marked.len()))))
                .child(div().flex().flex_col().gap_1p5()
                    .child(Self::property("Файлов", files.to_string()))
                    .child(Self::property("Папок", folders.to_string()))
                    .child(Self::property("Размер", format!("{} (без папок)", format_size(bytes)))))
                .child(div().flex().gap_2()
                    .child(text_button("group-stage", "Отложить", ButtonKind::Ghost).flex_1()
                        .on_click(cx.listener(|this, _, _, cx| this.stage(cx))))
                    .child(text_button("group-recycle", "В Корзину", ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _, _, cx| this.recycle(cx)))))
                .child(text_button("group-rename", "Переименовать…", ButtonKind::Ghost).w_full()
                    .on_click(cx.listener(|this, _, window, cx| this.begin_bulk_rename(window, cx))))
                .into_any_element();
        }
        let selected_entry = self.selected.clone().and_then(|selected| {
            let folder = selected.parent()?.to_path_buf();
            let (listing, _) = self.folder_view(self.browser.active, &folder)?;
            listing.entries.iter().find(|entry| entry.path == selected).cloned()
        });
        let current = self.pane_path(self.active_side());
        let (title, icon_path, tint) = match &selected_entry {
            Some(entry) => {
                let (icon_path, tint) = entry_icon(entry);
                (entry.name.clone(), icon_path, tint)
            }
            None if self.selected.is_some() => (browser::display_name(self.selected.as_ref().unwrap()), "fm/file.svg", TEXT_DIM),
            None => (browser::display_name(&current), "fm/folder-fill.svg", FOLDER),
        };
        panel = panel.child(
            div().flex().items_center().gap_2()
                .child(match selected_entry.as_ref().filter(|entry| !entry.is_directory).and_then(|entry| self.system_icon(&entry.path)) {
                    Some(image) => img(image).size(px(16.)).flex_none().into_any_element(),
                    None => icon(icon_path, tint).into_any_element(),
                })
                .child(div().flex_1().min_w_0().font_weight(gpui::FontWeight::SEMIBOLD)
                    .overflow_hidden().child(title))
        );
        match &selected_entry {
            Some(entry) => {
                let mut props = div().flex().flex_col().gap_1p5();
                if !entry.is_directory {
                    props = props.child(Self::property("Размер", format_size(entry.size)));
                } else if let Some(Some(size)) = self.sizes.known.get(&entry.path) {
                    props = props.child(Self::property("Размер", format!("{}{}", if size.complete { "" } else { "не меньше " }, format_size(size.bytes))))
                        .child(Self::property("Содержит", format!("файлов: {}, папок: {}", size.files, size.folders)));
                } else {
                    let target = vec![entry.path.clone()];
                    props = props.child(div().child(text_button("inspector-measure", "Посчитать размер", ButtonKind::Ghost)
                        .on_click(cx.listener(move |this,_,_,cx| this.measure_folders(target.clone(), cx)))));
                }
                if let Some(modified) = entry.modified {
                    props = props.child(Self::property("Изменён", format_time(modified)));
                }
                panel = panel.child(props);
                let copy_target=entry.path.clone();let tip=entry.path.display().to_string();
                panel=panel.child(div().flex().items_center().gap_1().min_w_0()
                    .child(div().id("inspector-path").flex_1().min_w_0().truncate().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                        .child(tip.clone()).tooltip(move |window,cx|Tooltip::new(tip.clone()).build(window,cx)))
                    .child(icon_button("inspector-copy-path","fm/copy.svg","Копировать путь",false,true)
                        .on_click(cx.listener(move |this,_,_,cx|this.copy_path(&copy_target,cx)))));
                if let Some(undo) = self.undo_stack.last() {
                    panel = panel.child(
                        text_button("undo-last", format!("Отменить: {}", undo.label), ButtonKind::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| this.undo_last(cx)))
                    );
                }
                panel = panel.child(self.preview_section(entry));
                if filemanager_core::preview_host::supports_document(&entry.path) {
                    let path = entry.path.clone();
                    panel = panel.child(text_button("document-preview-open", "Просмотреть документ", ButtonKind::Ghost)
                        .on_click(cx.listener(move |this, _, _, cx| this.open_document_preview(path.clone(), cx))));
                }
            }
            None if self.selected.is_some() => {
                let selected = self.selected.as_ref().unwrap();
                panel = panel.child(Self::property("Выбранный путь", selected.display().to_string()));
                let message = if self.inspector_loading || selected.parent().is_some_and(|parent| self.directory_loading.contains_key(parent)) {
                    "Проверяем выбранный путь…".to_owned()
                } else {
                    self.inspector_preview.as_ref().filter(|(kind, _)| kind == "Файл недоступен")
                        .map(|(_, reason)| reason.clone())
                        .unwrap_or_else(|| "Выбранного пути нет в списке. Файл мог быть удалён или перемещён.".into())
                };
                panel = panel.child(div().text_size(px(12.)).text_color(rgb(DANGER)).child(message));
            }
            None => {
                let count = self.folder_view(self.browser.active, &current).map(|(_, indices)| indices.len());
                panel = panel.child(div().flex().flex_col().gap_1p5()
                    .child(Self::property("Папка", current.display().to_string()))
                    .when_some(count, |this, count| this.child(Self::property("Содержит", items_label(count)))));
            }
        }
        panel = panel.child(self.versions_section(cx));
        panel = panel.child(self.feed_section(cx));
        panel.into_any_element()
    }

    /// Other files saved as versions of the selected document
    /// (`Отчёт_v_1_6.docx`), newest first; a click selects one.
    fn versions_section(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.inspector_versions.is_empty() {
            return div().into_any_element();
        }
        let mut section = div().flex().flex_col().gap_0p5().pt_2().border_t_1().border_color(rgb(BORDER))
            .child(div().id("versions-toggle").h(px(28.)).flex().items_center().gap_1().cursor_pointer().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                .child(small_icon(if self.versions_expanded {"fm/chevron-down.svg"} else {"fm/chevron-right.svg"},TEXT_MUTED))
                .child(format!("Версии · {}", self.inspector_versions.len()))
                .on_click(cx.listener(|this,_,_,cx| {this.versions_expanded=!this.versions_expanded;cx.notify();})));
        if !self.versions_expanded {return section.into_any_element();}
        for (index, member) in self.inspector_versions.iter().enumerate() {
            let current = self.selected.as_ref() == Some(&member.path);
            let path = member.path.clone();
            section = section.child(
                div().id(("version", index)).h(px(24.)).px_2().rounded_sm().flex().items_center().gap_2()
                    .text_size(px(12.)).cursor_pointer()
                    .when(current, |row| row.bg(rgb(TRAIL)))
                    .when(!current, |row| row.hover(|style| style.bg(rgb(HOVER))))
                    .child(div().w(px(64.)).flex_none().text_color(rgb(TEXT)).child(if member.version.number.is_empty() { "Исходный".to_owned() } else { member.version.label() }))
                    .child(div().flex_1().min_w_0().truncate().text_color(rgb(TEXT_MUTED))
                        .child(member.modified.map(format_time).unwrap_or_default()))
                    .child(div().flex_none().text_color(rgb(TEXT_DIM)).child(format_size(member.size)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(path.clone());
                        this.marked.clear();
                        cx.notify();
                    }))
            );
        }
        section.into_any_element()
    }

    fn preview_section(&self, entry: &Entry) -> AnyElement {
        // GPUI only decodes dimension-validated small PNGs from the helper.
        if let Some(thumbnail) = self.inspector_thumbnail.clone().filter(|_| self.inspector_path.as_ref() == Some(&entry.path)) {
            return div().flex().flex_col().gap_1()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child("Предпросмотр"))
                .child(div().w_full().h(px(240.)).p_1().flex().items_center().justify_center()
                    .child(img(thumbnail).size_full().object_fit(ObjectFit::Contain)))
                .into_any_element();
        }
        if self.inspector_loading {
            return div().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child("Загрузка предпросмотра…").into_any_element();
        }
        match &self.inspector_preview {
            // Other kinds only restate the type and size shown above.
            Some((kind, description)) if self.inspector_path == self.selected
                && matches!(kind.as_str(), "text" | "Файл недоступен") => div().flex().flex_col().gap_1()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child(match kind.as_str() {
                    "Файл недоступен" => "Файл недоступен",
                    _ => "Предпросмотр",
                }))
                .child(div().id("preview-text").h(px(260.)).min_w_0().overflow_scrollbar()
                    .text_size(px(12.)).font_family("monospace").text_color(rgb(TEXT_MUTED))
                    .child(div().flex().flex_col().children(description.lines().enumerate().map(|(index,line)|
                        div().id(("preview-line",index)).whitespace_nowrap().flex_none().child(if line.is_empty() {" ".to_owned()} else {line.to_owned()})))))
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    /// What changed in the current folder and its subfolders: newest first,
    /// by day; a click shows the file. Only names and times are recorded.
    fn feed_section(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::feed::PERIODS;
        let current = self.pane_path(self.active_side());
        let mut section = div().flex().flex_col().gap_1().pt_2().border_t_1().border_color(rgb(BORDER))
            .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child(format!("Изменения в «{}»", browser::display_name(&current))))
            .child(div().flex().flex_wrap().gap_1().children(PERIODS.iter().enumerate().map(|(index, (label, _))| {
                let chosen = index == self.feed.period;
                div().id(("feed-period", index)).h(px(22.)).px_2().flex().items_center().rounded_md().cursor_pointer()
                    .text_size(px(11.)).border_1()
                    .when(chosen, |chip| chip.bg(rgb(ACCENT_SOFT)).border_color(rgb(ACCENT)).text_color(rgb(TEXT)))
                    .when(!chosen, |chip| chip.border_color(rgb(BORDER)).text_color(rgb(TEXT_MUTED)).hover(|style| style.bg(rgb(HOVER))))
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_feed_period(index, cx)))
            })));
        if let Some(reason) = &self.live_watch_error {
            section = section.child(div().text_size(px(12.)).text_color(rgb(DANGER)).child(reason.clone()));
        }
        let events: Vec<&filemanager_core::history::Event> = if self.feed.folder.as_ref() == Some(&current) { self.feed.events.iter().collect() } else { Vec::new() };
        if events.is_empty() {
            return section.child(div().pt_1().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                .child("Изменений нет. Папка замечает изменения в папках, пока они открыты в ней; содержимое файлов не сохраняется."))
                .into_any_element();
        }
        let mut day = String::new();
        let mut list = div().id("feed-list").max_h(px(420.)).overflow_y_scrollbar().flex().flex_col();
        for event in events {
            let heading = event.day_label();
            if heading != day {
                list = list.child(div().pt_2().pb_0p5().text_size(px(11.)).font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT_MUTED)).child(heading.clone()));
                day = heading;
            }
            let name = browser::display_name(&event.path);
            // Where in the folder, when not directly in it.
            let place = event.path.parent().and_then(|parent| parent.strip_prefix(&current).ok())
                .map(|relative| relative.display().to_string()).filter(|relative| !relative.is_empty());
            let tint = match event.kind.as_str() { "missing" => DANGER, "observed" | "version" => ACCENT, _ => TEXT_MUTED };
            let path = event.path.clone();
            let gone = event.kind == "missing";
            let event_id = event.id;
            let editing = self.feed.editing == Some(event_id);
            let note = match (event.author.as_deref().filter(|author| !author.is_empty()), event.comment.trim()) {
                (Some(author), "") => Some(author.to_owned()),
                (Some(author), comment) => Some(format!("{author}: {comment}")),
                (None, "") => None,
                (None, comment) => Some(comment.to_owned()),
            };
            list = list.child(div().id(("feed-event", event_id as usize)).group("feed-row").py_1().px_1().rounded_sm().cursor_pointer()
                .flex().flex_col().gap_0p5().text_size(px(12.))
                .when(editing, |row| row.bg(rgb(ACCENT_SOFT)))
                .when(!editing, |row| row.hover(|style| style.bg(rgb(HOVER))))
                .child(div().flex().items_center().gap_1()
                    .child(div().flex_1().min_w_0().truncate().text_color(rgb(if gone { TEXT_MUTED } else { TEXT })).child(name))
                    // Who changed it and why: written by hand, kept with the entry.
                    .child(div().id(("feed-comment", event_id as usize)).flex_none().size(px(20.)).rounded_sm().flex().items_center().justify_center()
                        .when(!editing && note.is_none(), |button| button.invisible().group_hover("feed-row", |style| style.visible()))
                        .hover(|style| style.bg(rgb(HOVER)))
                        .child(small_icon("fm/pencil.svg", if note.is_some() || editing { ACCENT } else { TEXT_DIM }))
                        .tooltip(|window, cx| Tooltip::new("Комментарий: кто и что изменил").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.edit_feed_comment(event_id, window, cx);
                        }))))
                .child(div().flex().gap_1().min_w_0().text_size(px(11.))
                    .child(div().flex_none().text_color(rgb(TEXT_DIM)).child(event.clock_label()))
                    .child(div().flex_none().text_color(rgb(tint)).child(event.feed_label().to_owned()))
                    .when_some(place, |row, place| row.child(div().min_w_0().truncate().text_color(rgb(TEXT_DIM)).child(format!("· {place}")))))
                .when_some(note.filter(|_| !editing), |row, note| row.child(div().text_size(px(11.)).text_color(rgb(TEXT)).child(note)))
                .on_click(cx.listener(move |this, _, _, cx| this.open_feed_event(path.clone(), cx))));
            if editing {
                list = list.child(div().p_1().flex().flex_col().gap_1()
                    .child(Input::new(&self.author_input).xsmall())
                    .child(Input::new(&self.comment_input).xsmall())
                    .child(div().flex().gap_1().justify_end()
                        .child(text_button("feed-comment-cancel", "Отмена", ButtonKind::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| { this.feed.editing = None; cx.notify(); })))
                        .child(text_button("feed-comment-save", "Сохранить", ButtonKind::Primary)
                            .on_click(cx.listener(|this, _, _, cx| this.save_feed_comment(cx))))));
            }
        }
        section.child(list).into_any_element()
    }

    // -------------------------------------------------------- status bar

    fn status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.pane_path(self.active_side());
        let count = self.folder_view(self.browser.active, &current).map(|(_, indices)| indices.len());
        let marked = self.marked.len();
        let selection = if marked > 1 { Some(format!("Выбрано: {marked}")) } else { self.selected.clone().and_then(|selected| {
            let folder = selected.parent()?.to_path_buf();
            let (listing, _) = self.folder_view(self.browser.active, &folder)?;
            listing.entries.iter().find(|entry| entry.path == selected).map(|entry| {
                if entry.is_directory { format!("Выбрано: {}", entry.name) }
                else { format!("Выбрано: {} · {}", entry.name, format_size(entry.size)) }
            })
        }) };
        let filter_shown = self.filter_open || !self.filter.is_empty();
        let split = self.browser.groups.len() > 1;
        let mut bar = div().w_full().h(px(32.)).flex_none().pl_1().pr_1().flex().items_center().gap_3()
            .bg(rgb(WINDOW)).border_t_1().border_color(rgb(BORDER))
            .text_size(px(12.)).text_color(rgb(TEXT_MUTED))
            // Atlas: a magnifier in the corner; the filter field opens in place.
            .when(!filter_shown, |this| this.child(
                bar_button("filter-open", "fm/search.svg", "Фильтр по имени (Ctrl+F)", false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.key_find(&crate::keys::Find, window, cx);
                    }))))
            .when(filter_shown, |this| this.child(
                div().id("filter-field").flex_none().w(px(200.))
                    .tooltip(|window, cx| Tooltip::new("Фильтр по имени в этой папке. Enter — во вложенных папках, Esc — закрыть.").build(window, cx))
                    .child(Input::new(&self.search_input).h(px(24.)).text_size(px(12.))
                        .prefix(small_icon("fm/search.svg", TEXT_DIM)).cleanable(true))))
            .when_some(count, |this, count| this.child(items_label(count)))
            .when_some(selection, |this, text| this.child(div().max_w(px(360.)).truncate().child(text)))
            .child(div().flex_1().min_w_0().truncate().text_color(rgb(TEXT_DIM))
                .child(crate::messages::localize(&self.status)));
        if self.copy_in_progress {
            let paused = self.copy_control.as_ref().is_some_and(|control| control.is_paused());
            bar = bar.child(
                div().id("pause-copy").px_2().rounded_sm().cursor_pointer().text_color(rgb(TEXT))
                    .hover(|style| style.bg(rgb(HOVER)))
                    .child(if paused { "Продолжить" } else { "Пауза" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_copy_pause(cx)))
            ).child(
                div().id("cancel-copy").px_2().rounded_sm().cursor_pointer().text_color(rgb(DANGER))
                    .hover(|style| style.bg(rgb(HOVER)))
                    .child("Отменить передачу")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_copy(cx)))
            );
        } else if let Some((items, _, _)) = &self.retry {
            bar = bar.child(
                div().id("retry-copy").px_2().rounded_sm().cursor_pointer().text_color(rgb(ACCENT))
                    .hover(|style| style.bg(rgb(HOVER)))
                    .child(format!("Повторить сбойные ({})", items.len()))
                    .tooltip(|window, cx| Tooltip::new("Снова скопировать или перенести файлы, которые не прошли, например при обрыве сети").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| this.retry_failed(cx)))
            );
        }
        if !self.zone.items().is_empty() {
            bar = bar.child(div().flex().items_center().gap_1()
                .child(small_icon("fm/inbox.svg", ACCENT))
                .child(format!("Для переноса: {}", self.zone.items().len())));
        }
        if self.watched_root.is_some() {
            bar = bar.child(div().flex().items_center().gap_1().text_color(rgb(SUCCESS))
                .child(small_icon("fm/eye-watch.svg", SUCCESS)).child("Наблюдение"));
        }
        bar.child(div().flex_none().flex().items_center().gap_0p5()
            .child(bar_button("bar-new-folder", "fm/folder-plus.svg", "Новая папка (Ctrl+Shift+N)", false)
                .on_click(cx.listener(|this, _, window, cx| this.begin_new_folder(window, cx))))
            .child(bar_button("bar-split", "fm/split.svg", "Разделить окно (Ctrl+\\)", split)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_split(cx))))
            .child(bar_button("bar-hidden", if self.show_hidden { "fm/eye.svg" } else { "fm/eye-off.svg" },
                "Скрытые файлы (Ctrl+.)", self.show_hidden)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_hidden = !this.show_hidden;
                    cx.notify();
                })))
            .child(bar_button("bar-list", "fm/list.svg", "Список (Ctrl+1)", !self.miller(self.active_side()))
                .on_click(cx.listener(|this, _, _, cx| this.set_miller(false, cx))))
            .child(bar_button("bar-columns", "fm/columns.svg", "Колонки (Ctrl+2)", self.miller(self.active_side()))
                .on_click(cx.listener(|this, _, _, cx| this.set_miller(true, cx)))))
    }

    // ------------------------------------------------------- context menu

    pub(crate) fn open_menu(&mut self, position: gpui::Point<gpui::Pixels>, target: MenuTarget, cx: &mut Context<Self>) {
        self.menu_serial = self.menu_serial.wrapping_add(1);
        self.context_menu = Some(ContextMenu { position, target, serial: self.menu_serial });
        cx.notify();
    }

    fn menu_item(
        id: &'static str,
        icon_path: &'static str,
        label: impl Into<SharedString>,
        shortcut: &'static str,
        danger: bool,
        enabled: bool,
    ) -> Stateful<Div> {
        let color = if !enabled { TEXT_DIM } else if danger { DANGER } else { TEXT };
        let label: SharedString = label.into();
        let tip = label.clone();
        // Long labels end in "…" with the full text in a tooltip; the
        // shortcut keeps its place inside the menu.
        div().id(id).h(px(28.)).flex_none().px_2().flex().items_center().gap_2().rounded_md()
            .text_size(px(13.)).text_color(rgb(color))
            .when(enabled, |this| this.cursor_pointer().hover(|style| style.bg(rgb(if danger { 0x3A2128 } else { HOVER }))))
            .when(tip.chars().count() > 34, |this| this.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
            .child(small_icon(icon_path, if !enabled { TEXT_DIM } else if danger { DANGER } else { TEXT_MUTED }))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(div().flex_none().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child(shortcut))
    }

    fn separator() -> Div {
        div().my_1().h(px(1.)).bg(rgb(BORDER))
    }

    fn context_menu_view(&self, menu: &ContextMenu, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        // Never taller than the window: a long menu scrolls instead of
        // running past the bottom edge.
        let max_height = window.viewport_size().height - px(16.);
        let mut list = div().id(("context-menu", menu.serial as usize)).occlude().w(px(300.)).max_h(max_height)
            .overflow_y_scroll().p_1().flex().flex_col()
            .rounded_lg().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }));
        let staged = self.zone.items().len();
        match &menu.target {
            MenuTarget::Recent(path) => {
                let (open, tab, forget, star) = (path.clone(), path.clone(), path.clone(), path.clone());
                list = list.child(Self::menu_item("recent-open","fm/folder-fill.svg","Открыть","",false,true)
                        .on_click(cx.listener(move |this,_,_,cx| {this.context_menu=None;this.navigate_side(this.active_side(),&open,None,cx);})))
                    .child(Self::menu_item("recent-tab","fm/plus.svg","Открыть в новой вкладке","",false,true)
                        .on_click(cx.listener(move |this,_,_,cx| {this.context_menu=None;this.open_in_new_tab(&tab,cx);})))
                    .child(Self::menu_item("recent-star","fm/star.svg","Добавить в избранное","",false,true)
                        .on_click(cx.listener(move |this,_,_,cx|this.toggle_favorite(&star,cx))))
                    .child(Self::separator())
                    .child(Self::menu_item("recent-forget","fm/x.svg","Убрать из недавних","",false,true)
                        .on_click(cx.listener(move |this,_,_,cx|this.forget_recent(Some(&forget),cx))))
                    .child(Self::menu_item("recent-clear","fm/trash.svg","Очистить недавние","",false,true)
                        .on_click(cx.listener(|this,_,_,cx|this.forget_recent(None,cx))));
            }
            MenuTarget::Place(index) => {
                let hidden = self.hidden_places();
                if let Some(index) = *index {
                    list = list.child(Self::menu_item("place-hide","fm/eye-off.svg","Скрыть из «Мест»","",false,true)
                            .on_click(cx.listener(move |this,_,_,cx|this.hide_place(index,cx))))
                        .child(Self::menu_item("place-up","fm/arrow-up.svg","Выше","",false,index>0)
                            .on_click(cx.listener(move |this,_,_,cx|this.move_place(index,index.saturating_sub(1),cx))))
                        .child(Self::menu_item("place-down","fm/chevron-down.svg","Ниже","",false,index+1<self.places.len())
                            .on_click(cx.listener(move |this,_,_,cx|this.move_place(index,index+1,cx))))
                        .child(Self::separator());
                }
                list = list.child(Self::menu_item("places-show",  "fm/eye.svg",
                        if hidden > 0 { format!("Вернуть скрытые места ({hidden})") } else { "Скрытых мест нет".into() }, "", false, hidden > 0)
                    .when(hidden > 0, |item| item.on_click(cx.listener(|this,_,_,cx|this.show_all_places(cx)))));
            }
            MenuTarget::Favorite(index) => {
                if let Some(path)=self.favorites.get(*index) {
                    let path=path.clone();let index=*index;
                    list=list.child(Self::menu_item("favorite-remove","fm/x.svg","Убрать из избранного","",false,true)
                        .on_click(cx.listener(move |this,_,_,cx|this.toggle_favorite(&path,cx))))
                        .child(Self::menu_item("favorite-up","fm/arrow-up.svg","Выше","",false,index>0)
                            .on_click(cx.listener(move |this,_,_,cx|this.move_favorite(index,index.saturating_sub(1),cx))))
                        .child(Self::menu_item("favorite-down","fm/chevron-down.svg","Ниже","",false,index+1<self.favorites.len())
                            .on_click(cx.listener(move |this,_,_,cx|this.move_favorite(index,index+1,cx))));
                }
            }
            MenuTarget::Entry(path) => {
                let is_dir = self.directory_cache.get(path.parent().unwrap_or(path))
                    .and_then(|listing| listing.entries.iter().find(|e| &e.path == path).map(|e| e.is_directory))
                    .unwrap_or(false);
                let open_path = path.clone();
                list = list.child(Self::menu_item("m-open", if is_dir { "fm/folder-fill.svg" } else { "fm/external.svg" },
                        "Открыть", "Enter", false, true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.context_menu = None;
                        let side = this.active_side();
                        this.open_entry(open_path.clone(), is_dir, side, cx);
                    })));
                if is_dir {
                    let favorite_path=path.clone();
                    list=list.child(Self::menu_item("m-favorite","fm/star.svg",
                        if self.favorites.contains(path) {"Убрать из избранного"} else {"Добавить в избранное"},"",false,true)
                        .on_click(cx.listener(move |this,_,_,cx|this.toggle_favorite(&favorite_path,cx))));
                    let tab_path = path.clone();
                    let other_path = path.clone();
                    list = list
                        .child(Self::menu_item("m-open-tab", "fm/plus.svg", "Открыть в новой вкладке", "", false, true)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.context_menu = None;
                                this.open_in_new_tab(&tab_path, cx);
                            })))
                        .child(Self::menu_item("m-open-other", "fm/split.svg", "Открыть в другой панели", "", false, true)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.context_menu = None;
                                this.open_in_other_pane(&other_path, cx);
                            })));
                }
                let stage_target = self.selection();
                let explorer_target = stage_target.clone();
                let copy_paths = if stage_target.contains(path) { stage_target.clone() } else { vec![path.clone()] };
                let network = filemanager_core::path_utils::is_network_path(path) && !path.to_string_lossy().starts_with(r"\\");
                let count = stage_target.len();
                list = list.child(Self::separator())
                    .child(Self::menu_item("m-copy", "fm/copy.svg", "Копировать", "Ctrl+C", false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.clipboard_put(false, cx);
                        })))
                    .child(Self::menu_item("m-cut", "fm/move.svg", "Вырезать", "Ctrl+X", false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.clipboard_put(true, cx);
                        })))
                    .child(Self::menu_item("m-rename", "fm/pencil.svg", "Переименовать", "F2", false, true)
                        .on_click(cx.listener(|this, _, window, cx| this.begin_rename(window, cx))))
                    .child(Self::menu_item("m-copy-path", "fm/clipboard.svg",
                        if count > 1 { format!("Копировать пути ({count})") } else { "Копировать путь".into() }, "Ctrl+Shift+C", false, true)
                        .on_click(cx.listener({ let paths = copy_paths.clone(); move |this, _, _, cx| this.copy_paths(&paths, crate::CopyAs::Path, cx) })))
                    .when(network, |list| list.child(Self::menu_item("m-copy-unc", "fm/server.svg", "Копировать сетевой путь (\\\\сервер\\…)", "", false, true)
                        .on_click(cx.listener({ let paths = copy_paths.clone(); move |this, _, _, cx| this.copy_paths(&paths, crate::CopyAs::Network, cx) }))))
                    .child(Self::menu_item("m-copy-name", "fm/file-text.svg",
                        if count > 1 { format!("Копировать имена ({count})") } else { "Копировать имя".into() }, "", false, true)
                        .on_click(cx.listener({ let paths = copy_paths.clone(); move |this, _, _, cx| this.copy_paths(&paths, crate::CopyAs::Name, cx) })))
                    .when(is_dir || copy_paths.len() > 1, |list| {
                        // From the listing, not the disk: menus redraw every frame.
                        let folders: Vec<PathBuf> = copy_paths.iter().filter(|p| self.directory_cache.get(p.parent().unwrap_or(p))
                            .is_some_and(|listing| listing.entries.iter().any(|e| &e.path == *p && e.is_directory))).cloned().collect();
                        let n = folders.len();
                        list.child(Self::menu_item("m-measure", "fm/drive.svg",
                            if n > 1 { format!("Посчитать размер ({n})") } else { "Посчитать размер".into() }, "", false, n > 0)
                            .when(n > 0, |item| item.on_click(cx.listener(move |this, _, _, cx| this.measure_folders(folders.clone(), cx)))))
                    })
                    .child(Self::menu_item("m-stage", "fm/inbox.svg",
                        if count > 1 { format!("Отложить для переноса ({count})") } else { "Отложить для переноса".into() }, "Ctrl+Shift+D", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.stage_paths(&stage_target, cx);
                        })))
                    .child(Self::menu_item("m-explorer", "fm/command.svg",
                        if count > 1 { format!("Меню Проводника ({count})") } else { "Меню Проводника".into() }, "Shift+ПКМ", false, true)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let paths = if explorer_target.len() > 0 { explorer_target.clone() } else { Vec::new() };
                            this.explorer_menu(paths, true, window, cx);
                        })))
                    .child(Self::separator())
                    .child(Self::menu_item("m-recycle", "fm/trash.svg",
                        match (filemanager_core::path_utils::has_recycle_bin(path), count > 1) {
                            (true, true) => format!("Удалить в Корзину ({count})"),
                            (true, false) => "Удалить в Корзину".into(),
                            (false, true) => format!("Удалить безвозвратно ({count})"),
                            (false, false) => "Удалить безвозвратно".into(),
                        }, "Del", true, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.recycle(cx);
                        })));
            }
            MenuTarget::Folder(folder) => {
                let copy_target = folder.clone();
                let tab_target = folder.clone();
                let (f1, f2, f3, f4) = (folder.clone(), folder.clone(), folder.clone(), folder.clone());
                let explorer_folder = folder.clone();
                if let Some(undo) = self.undo_stack.last() {
                    list = list
                        .child(Self::menu_item("m-undo", "fm/undo.svg", format!("Отменить: {}", undo.label), "Ctrl+Z", false, true)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_menu = None;
                                this.undo_last(cx);
                            })))
                        .child(Self::separator());
                }
                list = list
                    .child(Self::menu_item("m-explorer-folder", "fm/command.svg", "Меню Проводника для папки", "Shift+ПКМ", false, true)
                        .on_click(cx.listener(move |this, _, window, cx| this.explorer_folder_menu(explorer_folder.clone(), window, cx))))
                    .child(Self::separator())
                    .child(Self::menu_item("m-new-folder", "fm/folder-plus.svg", "Новая папка", "Ctrl+Shift+N", false, true)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.enter_folder(&f1, cx);
                            this.begin_new_folder(window, cx);
                        })))
                    .child(Self::menu_item("m-clipboard-paste", "fm/clipboard.svg", "Вставить", "Ctrl+V", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.enter_folder(&f2, cx);
                            this.clipboard_paste(cx);
                        })))
                    .child(Self::menu_item("m-paste", "fm/clipboard.svg",
                        format!("Копировать отложенные сюда ({staged})"), "", false, staged > 0)
                        .when(staged > 0, |this| this.on_click(cx.listener(move |this, _, _, cx| {
                            this.enter_folder(&f3, cx);
                            this.paste(cx);
                        }))))
                    .child(Self::menu_item("m-move-here", "fm/move.svg", "Переместить отложенные сюда", "", false, staged > 0)
                        .when(staged > 0, |this| this.on_click(cx.listener(move |this, _, _, cx| {
                            this.enter_folder(&f4, cx);
                            this.move_staged(cx);
                        }))))
                    .child(Self::separator())
                    .child(Self::menu_item("m-refresh", "fm/refresh.svg", "Обновить", "F5", false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.refresh_visible_directories(cx);
                        })))
                    .child(Self::menu_item("m-hidden", if self.show_hidden { "fm/eye-off.svg" } else { "fm/eye.svg" },
                        if self.show_hidden { "Не показывать скрытые" } else { "Показать скрытые" }, "Ctrl+H", false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.show_hidden = !this.show_hidden;
                            cx.notify();
                        })))
                    .child(Self::menu_item("m-folder-tab", "fm/plus.svg", "Открыть в новой вкладке", "", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.open_in_new_tab(&tab_target, cx);
                        })))
                    .child(Self::menu_item("m-folder-path", "fm/copy.svg", "Копировать путь папки", "", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.copy_path(&copy_target, cx);
                        })))
                    .when(self.browser.groups.len() > 1, |list| list.child(Self::menu_item("m-folder-compare", "fm/split.svg", "Сравнить с соседней панелью", "", false, true)
                        .on_click(cx.listener(|this, _, _, cx| this.open_comparison(cx)))))
                    .child(Self::menu_item("m-folder-measure", "fm/drive.svg", "Посчитать размер папки", "", false, true)
                        .on_click(cx.listener({ let target = vec![folder.clone()]; move |this, _, _, cx| this.measure_folders(target.clone(), cx) })))
                    .when(filemanager_core::path_utils::is_network_path(folder) && !folder.to_string_lossy().starts_with(r"\\"), |list| {
                        let target = vec![folder.clone()];
                        list.child(Self::menu_item("m-folder-unc", "fm/server.svg", "Копировать сетевой путь папки", "", false, true)
                            .on_click(cx.listener(move |this, _, _, cx| this.copy_paths(&target, crate::CopyAs::Network, cx))))
                    });
            }
            MenuTarget::Drop(paths, folder) => {
                let what = match paths.as_slice() {
                    [single] => format!("«{}»", browser::display_name(single)),
                    many => format!("{} элементов", many.len()),
                };
                list = list.child(div().px_2().py_1().text_size(px(12.)).text_color(rgb(TEXT_DIM)).truncate()
                    .child(format!("{what} → {}", browser::display_name(folder))));
                for (id, icon_path, label, copy) in [("m-drop-copy", "fm/copy.svg", "Копировать сюда", true),
                                                      ("m-drop-move", "fm/move.svg", "Переместить сюда", false)] {
                    let (paths, folder) = (paths.clone(), folder.clone());
                    list = list.child(Self::menu_item(id, icon_path, label, "", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.drop_into(&paths, folder.clone(), copy, cx);
                        })));
                }
                list = list.child(Self::separator())
                    .child(Self::menu_item("m-drop-cancel", "fm/x.svg", "Отмена", "Esc", false, true)
                        .on_click(cx.listener(|this, _, _, cx| { this.context_menu = None; cx.notify(); })));
            }
            MenuTarget::TabList(group) => {
                let group=*group;
                list=list.w(px(440.)).max_h(px(420.)).overflow_y_scroll();
                if let Some(g)=self.browser.group(group) {
                    let titles=tab_titles(&g.tabs.iter().map(|t|t.pane.path.clone()).collect::<Vec<_>>());
                    for (index,tab) in g.tabs.iter().enumerate() {
                        list=list.child(div().id(("all-tab",index)).px_3().py_2().cursor_pointer().rounded_sm()
                            .hover(|style|style.bg(rgb(HOVER)))
                            .child(div().text_color(rgb(if index==g.active_tab {ACCENT} else {TEXT})).child(titles[index].clone()))
                            .child(div().max_w(px(420.)).truncate().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child(tab.pane.path.display().to_string()))
                            .on_click(cx.listener(move |this,_,_,cx|this.switch_group_tab(group,index,cx))));
                    }
                }
            }
            MenuTarget::Tab(group, index) => {
                let group = *group;
                let index = *index;
                let Some(tabs) = self.browser.group(group).map(|g| &g.tabs) else { return div().into_any_element(); };
                if index >= tabs.len() { return div().into_any_element(); }
                let many = tabs.len() > 1;
                let closable = (many || self.browser.groups.len() > 1) && !tabs[index].pinned;
                list = list
                    .child(Self::menu_item("m-tab-new", "fm/plus.svg", "Новая вкладка", "Ctrl+T", false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.add_tab(cx);
                        })))
                    .child(Self::menu_item("m-tab-duplicate", "fm/copy.svg", "Дублировать", "", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            if let Some(path) = this.browser.group(group).and_then(|g|g.tabs.get(index)).map(|tab| tab.active().path.clone()) {
                                this.open_in_new_tab(&path, cx);
                            }
                        })))
                    .child(Self::menu_item("m-tab-pin", "fm/folder-fill.svg", if tabs[index].pinned { "Открепить вкладку" } else { "Закрепить вкладку" }, "", false, true)
                        .on_click(cx.listener(move |this,_,_,cx| { this.browser.toggle_pin(group, index); this.context_menu=None; cx.notify(); })))
                    .child(Self::separator())
                    .child(Self::menu_item("m-tab-close", "fm/x.svg", "Закрыть", "Ctrl+W", false, closable)
                        .when(closable, |this| this.on_click(cx.listener(move |this, _, _, cx| { this.focus_group(group,cx); this.close_tab_at(index, cx); }))))
                    .child(Self::menu_item("m-tab-move","fm/split.svg","Переместить в другую панель","",false,self.browser.groups.len()>1)
                        .on_click(cx.listener(move |this,_,_,cx| {
                            if let Some(target)=this.browser.other_group(group) {
                                this.move_tab_to(&TabDrag {group,index,title:String::new()},target,usize::MAX,cx);
                            }
                        })))
                    .child(Self::menu_item("m-tab-split-right","fm/split.svg","Разделить вправо","Ctrl+Shift+S",false,true)
                        .on_click(cx.listener(move |this,_,window,cx|this.split_group(group,Edge::Right,if this.browser.group(group).is_some_and(|g|g.tabs.len()>1) {Some((group,index))} else {None},window,cx))))
                    .child(Self::menu_item("m-tab-split-down","fm/split.svg","Разделить вниз","Ctrl+Alt+S",false,true)
                        .on_click(cx.listener(move |this,_,window,cx|this.split_group(group,Edge::Bottom,if this.browser.group(group).is_some_and(|g|g.tabs.len()>1) {Some((group,index))} else {None},window,cx))))
                    .child(Self::menu_item("m-tab-close-others", "fm/x.svg", "Закрыть другие", "", false, many)
                        .when(many, |this| this.on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.focus_group(group,cx); this.close_other_tabs(index, cx);
                        }))));
            }
        }
        let animated = list.with_animation(
            ("menu-appear", menu.serial as usize),
            Animation::new(Duration::from_millis(130)).with_easing(ease_out_quint()),
            |this, delta| this.opacity(delta).mt(px(-6. * (1. - delta))),
        );
        deferred(anchored().position(menu.position).snap_to_window_with_margin(px(8.)).child(animated))
            .with_priority(1)
            .into_any_element()
    }

    // ------------------------------------------------------------ dialogs

    fn bulk_rename_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.bulk_rename_errors.is_empty() {
            let mut rows = div().id("bulk-rename-errors").max_h(px(320.)).overflow_y_scrollbar()
                .flex().flex_col().gap_2().text_size(px(12.));
            for error in &self.bulk_rename_errors { rows = rows.child(error.clone()); }
            return Some(div().id("bulk-rename-result").absolute().inset_0().flex().items_center().justify_center()
                .bg(rgba(0x0000008C))
                .child(div().w(px(620.)).max_h(px(560.)).p_4().flex().flex_col().gap_3().rounded_md().bg(rgb(RAISED))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child("Ошибки переименования группы")
                    .child(rows)
                    .child("Завершённые действия можно отменить Ctrl+Z после закрытия этого списка.")
                    .child(text_button("bulk-result-close", "Закрыть", ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _, window, cx| this.key_dismiss(&keys::DismissOverlay, window, cx)))))
                .into_any_element());
        }
        let state = self.bulk_rename.as_ref()?;
        let mut rows = div().id("bulk-rename-rows").max_h(px(320.)).overflow_y_scrollbar()
            .flex().flex_col().gap_2().p_2().rounded_md().bg(rgb(SURFACE));
        if state.loading { rows = rows.child("Проверка имён…"); }
        if let Some(error) = &state.error { rows = rows.child(error.clone()); }
        if let Some(preview) = &state.preview {
            for entry in &preview.entries {
                let mut row = div().flex().flex_col().gap_1().text_size(px(12.))
                    .child(format!("{} → {}", browser::display_name(&entry.source), browser::display_name(&entry.destination)));
                if let Some(error) = &entry.error {
                    row = row.child(div().text_color(rgb(0xe07a7a)).child(format!("Ошибка: {error}")));
                }
                rows = rows.child(row);
            }
        }
        let can_apply = state.preview.as_ref().is_some_and(|preview| preview.can_apply()) && !self.operation_busy;
        let mut buttons = div().flex().justify_end().gap_2()
            .child(text_button("bulk-cancel", "Отмена", ButtonKind::Ghost)
                .on_click(cx.listener(|this, _, window, cx| this.key_dismiss(&keys::DismissOverlay, window, cx))));
        if can_apply {
            buttons = buttons.child(text_button("bulk-apply", "Переименовать группу", ButtonKind::Primary)
                .on_click(cx.listener(|this, _, _, cx| this.apply_bulk_rename(cx))));
        } else {
            buttons = buttons.child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child("Применение недоступно"));
        }
        Some(div().id("bulk-rename-backdrop").absolute().inset_0().flex().items_center().justify_center()
            .bg(rgba(0x0000008C))
            .child(div().w(px(620.)).max_h(px(560.)).p_4().flex().flex_col().gap_3().rounded_xl()
                .bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Массовое переименование"))
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child("{stem} — имя, {ext} — расширение с точкой, {n} — номер. До применения файлы не меняются."))
                .child(Input::new(&self.bulk_template_input))
                .child(rows)
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                    .child("Занятые имена и ошибки блокируют группу. Частичный успех можно отменить Ctrl+Z."))
                .child(buttons))
            .into_any_element())
    }

    fn conflict_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (transfer, conflicts) = self.pending_transfer.as_ref()?;
        let recycle = filemanager_core::path_utils::has_recycle_bin(&transfer.target);
        let names: Vec<String> = conflicts.iter().take(4).map(|p| browser::display_name(p)).collect();
        let more = conflicts.len().saturating_sub(names.len());
        let mut list = div().flex().flex_col().gap_1().p_2().rounded_md().bg(rgb(SURFACE))
            .border_1().border_color(rgb(BORDER)).text_size(px(12.));
        for name in names {
            list = list.child(div().flex().items_center().gap_2()
                .child(small_icon("fm/file.svg", TEXT_MUTED)).child(div().truncate().child(name)));
        }
        if more > 0 {
            list = list.child(div().text_color(rgb(TEXT_DIM)).child(format!("и ещё {more}")));
        }
        let dialog = div().id("conflict-dialog").w(px(460.)).p_4().flex().flex_col().gap_3()
            .rounded_md().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD)
                .child(format!("Совпадают имена: {}", conflicts.len())))
            .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                .child(format!("В папке «{}» уже есть объекты с такими именами. {} Каталоги не заменяются.",
                    browser::display_name(&transfer.target),
                    if recycle { "При замене прежние файлы отправляются в Корзину." }
                    else { "Здесь нет Корзины (сетевая папка или съёмный диск): прежние файлы заменятся безвозвратно, как в Проводнике." })))
            .child(list)
            .child(text_button("conflict-replace", if recycle { "Заменить файлы — прежние в Корзину" } else { "Заменить файлы безвозвратно" }, ButtonKind::Ghost)
                .on_click(cx.listener(|this, _, _, cx| this.resolve_transfer(crate::ConflictChoice::Replace, cx))))
            .child(div().flex().justify_end().gap_2()
                .child(text_button("conflict-cancel", "Отмена", ButtonKind::Ghost)
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_transfer(crate::ConflictChoice::Cancel, cx))))
                .child(text_button("conflict-skip", "Пропустить", ButtonKind::Ghost)
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_transfer(crate::ConflictChoice::Skip, cx))))
                .child(text_button("conflict-keep", "Сохранить оба", ButtonKind::Primary)
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_transfer(crate::ConflictChoice::KeepBoth, cx)))))
            .with_animation("conflict-appear",
                Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta).mt(px(12. * (1. - delta))));
        Some(
            div().id("conflict-backdrop").absolute().inset_0().flex().items_center().justify_center()
                .bg(rgba(0x0000008C))
                .child(dialog)
                .into_any_element()
        )
    }

    fn dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (title, body, confirm, kind): (&str, AnyElement, &str, ButtonKind) = if let Some(error)=&self.close_error {
            ("Сеанс не сохранён", div().flex().flex_col().gap_2()
                .child("Остаться и повторить сохранение или закрыть окно без сохранения последних изменений?")
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child(error.clone()))
                .into_any_element(), "Закрыть без сохранения", ButtonKind::Danger)
        } else if self.creating_folder {
            ("Новая папка", div().flex().flex_col().gap_2()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(format!("В папке: {}", self.pane_path(self.active_side()).display())))
                .child(Input::new(&self.folder_input)).into_any_element(), "Создать", ButtonKind::Primary)
        } else if self.saving_workspace {
            ("Создать проект", div().flex().flex_col().gap_2()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(format!("Вкладки активной панели: {}", self.browser.active_group().tabs.len())))
                .child(Input::new(&self.workspace_input))
                .when_some(self.project_error.clone(), |body,error|body.child(div().text_size(px(12.)).text_color(rgb(DANGER)).child(error)))
                .into_any_element(), "Сохранить", ButtonKind::Primary)
        } else if let Some(plans) = &self.confirm_recycle {
            let what = match plans.as_slice() {
                [(path, _)] => format!("«{}»", browser::display_name(path)),
                many => format!("Элементов: {} («{}» и другие)", many.len(),
                    many.first().map(|(p, _)| browser::display_name(p)).unwrap_or_default()),
            };
            let permanent = plans.iter().filter(|(_, plan)| plan.action == filemanager_core::operations::Action::Delete).count();
            let (title, note, confirm) = if permanent == 0 {
                ("Удалить в Корзину?", "Объект можно будет восстановить из Корзины Windows.".to_owned(), "В Корзину")
            } else if permanent == plans.len() {
                ("Удалить безвозвратно?", "Здесь нет Корзины Windows (сетевая папка или съёмный диск). \
                    Удалённое нельзя будет восстановить, Ctrl+Z не поможет.".to_owned(), "Удалить навсегда")
            } else {
                ("Удалить?", format!("{permanent} из {} — в папках без Корзины Windows: они будут удалены \
                    безвозвратно. Остальные — в Корзину.", plans.len()), "Удалить")
            };
            (title, div().flex().flex_col().gap_1().text_size(px(13.))
                .child(div().text_color(rgb(TEXT)).child(what))
                .child(div().text_size(px(12.)).text_color(rgb(if permanent == 0 { TEXT_MUTED } else { DANGER })).child(note))
                .into_any_element(), confirm, ButtonKind::Danger)
        } else {
            return None;
        };
        let dialog = div().id("dialog").w(px(420.)).p_4().flex().flex_col().gap_3()
            .rounded_md().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
            .child(body)
            .child(div().flex().justify_end().gap_2()
                .child(text_button("dialog-cancel", "Отмена", ButtonKind::Ghost)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.key_dismiss(&keys::DismissOverlay, window, cx);
                    })))
                .child(text_button("dialog-confirm", confirm, kind)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.close_error.take().is_some() {
                            this.close_document_preview(cx);window.remove_window();return;
                        } else if this.saving_workspace {
                            this.save_named_workspace(cx);
                        } else if this.creating_folder {
                            this.create_folder(cx);
                        } else {
                            this.recycle(cx);
                        }
                        let handle = this.focus_handle.clone();
                        window.focus(&handle, cx);
                    }))))
            .with_animation("dialog-appear",
                Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta).mt(px(12. * (1. - delta))));
        Some(
            div().id("dialog-backdrop").absolute().inset_0().flex().items_center().justify_center()
                .bg(rgba(0x0000008C))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    this.key_dismiss(&keys::DismissOverlay, window, cx);
                }))
                .child(dialog)
                .into_any_element()
        )
    }
}

impl Render for Explorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.interface_focus_pending {self.interface_focus_pending=false;window.focus(&self.focus_handle,cx);}
        self.validate_marquee(cx);
        if self.clear_filter {
            self.clear_filter = false;
            self.filter.clear();
            self.search_query.clear();
            self.search_input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.ensure_live_watch(cx);
        // Do not compete with the search walk for I/O while showing results.
        if !self.search_active {
            self.load_visible_directories(cx);
            self.request_icons(window.scale_factor(), cx);
        }
        self.load_selected_details(cx);
        self.load_feed(cx);
        if self.select_first_when_loaded && self.selected.is_none() {
            let folder = self.pane_path(self.active_side());
            if let Some((listing, indices)) = self.folder_view(self.browser.active, &folder) {
                self.select_first_when_loaded = false;
                if let Some(&first) = indices.first() {
                    self.selected = Some(listing.entries[first].path.clone());
                }
            }
        } else if self.selected.is_some() {
            self.select_first_when_loaded = false;
        }

        let window_width=window.viewport_size().width/px(1.);
        let sidebar_width=if self.show_sidebar {SIDEBAR_WIDTH} else {0.};
        let inspector_width=if self.show_inspector {300.} else {0.};
        let pane_width=(window_width-sidebar_width-inspector_width).max(240.);
        self.pane_width=pane_width/self.browser.groups.len() as f32;
        let height=(window.viewport_size().height/px(1.)-64.).max(240.);
        let group_id=format!("main-{}-{}-{}",self.browser.signature(),self.show_inspector,self.search_active);
        let mut panels=h_resizable(SharedString::from(group_id));
        let body=if self.search_active {self.search_results(cx).into_any_element()} else {
            self.layout_view(&self.browser.root.clone(),Vec::new(),pane_width,height,cx)
        };
        panels=panels.child(resizable_panel().size(px(pane_width)).size_range(px(240.)..px(10000.)).child(body));
        if self.show_inspector {
            panels = panels.child(resizable_panel().size(px(inspector_width)).size_range(px(220.)..px(520.))
                .child(self.inspector(cx)));
        }

        let mut root = div().relative().size_full().flex().flex_col().bg(rgb(WINDOW))
            .text_color(rgb(TEXT)).text_size(px(13.))
            .key_context("Filemanager")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this,_: &keys::PaletteEnter,window,cx|this.palette_enter(window,cx)))
            .on_action(cx.listener(|this,_: &keys::SettingsToggle,_,cx|this.settings_toggle(cx)))
            .on_action(cx.listener(|this,_: &keys::SettingsApply,_,cx|this.apply_settings(cx)))
            .on_drag_move::<TabDrag>(cx.listener(|this,event: &gpui::DragMoveEvent<TabDrag>,_,cx| {
                this.file_drag = false;
                if !this.pane_bounds.values().any(|bounds|bounds.contains(&event.event.position)) && this.dock_hover.take().is_some() {cx.notify();}
            }))
            // Leaving the window hands the files to Windows for other programs.
            .on_drag_move::<ExternalPaths>(cx.listener(|this,_: &gpui::DragMoveEvent<ExternalPaths>,_,_| {
                this.file_drag = true;
                this.drag_paths.clear();
            }))
            .on_drag_move::<FileDragInfo>(cx.listener(|this,event: &gpui::DragMoveEvent<FileDragInfo>,window,cx| {
                this.file_drag = true;
                let paths = &event.drag(cx).paths;
                if &this.drag_paths != paths { this.drag_paths = paths.clone(); }
                if outside_window(event.event.position, window) {
                    let paths = event.drag(cx).paths.clone();
                    this.drag_out(paths, window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this,event: &gpui::MouseMoveEvent,window,cx| {
                // Root runs after rows and columns: they have claimed the target.
                this.spring_settle();
                if let Some(drag) = this.right_drag.as_mut() {
                    if event.pressed_button == Some(MouseButton::Right) {
                        if drag.active && outside_window(event.position, window) {
                            let paths = drag.paths.clone();
                            this.drag_out(paths, window, cx);
                            return;
                        }
                        let moved = (event.position.x - drag.start.x).abs() + (event.position.y - drag.start.y).abs();
                        if !drag.active && moved > px(6.) { drag.active = true; }
                        drag.position = event.position;
                        if !drag.claimed { drag.target = None; }
                        drag.claimed = false;
                        if drag.active { cx.notify(); }
                        return;
                    }
                }
                if let Some((side, index, start, width)) = this.list_resize {
                    if !event.dragging() { this.finish_list_resize(); return; }
                    let (min, max) = crate::LIST_WIDTH_RANGE;
                    let mut widths = this.list_widths_for(side);
                    widths[index] = (width - (event.position.x - start) / px(1.)).clamp(min, max);
                    this.pane_list_widths.insert(side, widths);
                    cx.notify();
                    return;
                }
                if this.column_resize.is_some() {
                    if !event.dragging() || this.interface_modal() {
                        this.column_resize = None;
                        return;
                    }
                    let resize = this.column_resize.as_ref().unwrap();
                    let width = crate::layout::resize_columns(resize.start_width, (event.position.x - resize.start_x) / px(1.));
                    this.pane_column_widths.insert(resize.group, width);
                    cx.notify();
                } else { this.move_marquee(event,cx); }
            }))
            .on_mouse_up(MouseButton::Left,cx.listener(|this,_,_,cx| {this.dock_hover=None;this.file_drag=false;this.finish_column_resize();this.finish_list_resize();this.finish_marquee(cx);cx.notify();}))
            .on_mouse_up_out(MouseButton::Left,cx.listener(|this,_,_,cx| {this.dock_hover=None;this.file_drag=false;this.finish_column_resize();this.finish_list_resize();this.finish_marquee(cx);cx.notify();}))
            .on_mouse_up(MouseButton::Right,cx.listener(|this,event: &gpui::MouseUpEvent,window,cx| {
                let Some(drag)=this.right_drag.take() else {return};
                if !drag.active && event.modifiers.shift {
                    // Shift+right click: straight to the Explorer menu, with its extended verbs.
                    this.explorer_menu(drag.paths,true,window,cx);
                } else if !drag.active {
                    this.open_menu(event.position,drag.click_menu,cx);
                } else if let Some(target)=drag.target.filter(|target| !drag.paths.contains(target)
                    && !drag.paths.iter().all(|path| path.parent()==Some(target.as_path()))) {
                    this.open_menu(event.position,MenuTarget::Drop(drag.paths,target),cx);
                }
                cx.notify();
            }))
            .on_mouse_up_out(MouseButton::Right,cx.listener(|this,_,_,cx| {this.right_drag=None;cx.notify();}))
            // Mouse Back/Forward buttons, as in Explorer: they act on the pane under the cursor.
            .on_mouse_down(MouseButton::Navigate(gpui::NavigationDirection::Back),cx.listener(|this,event: &MouseDownEvent,_,cx| this.mouse_history(event,false,cx)))
            .on_mouse_down(MouseButton::Navigate(gpui::NavigationDirection::Forward),cx.listener(|this,event: &MouseDownEvent,_,cx| this.mouse_history(event,true,cx)))
            .on_action(cx.listener(|this,_: &keys::OpenPalette,window,cx|this.open_palette(window,cx)))
            .on_action(cx.listener(|this,_: &keys::OpenSettings,window,cx|this.open_settings(window,cx)))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_back(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_forward(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_up(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_new_tab(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_close_tab(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_next_tab(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_prev_tab(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_split(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_refresh(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_stage(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_find(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_address(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_rename(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_new_folder(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { Self::key_dismiss(this,event,window,cx); }))
            .on_action(cx.listener(|this,event,window,cx| { Self::key_select_next(this,event,window,cx); }))
            .on_action(cx.listener(|this,event,window,cx| { Self::key_select_prev(this,event,window,cx); }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_select_first(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_select_last(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_open(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_column_left(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_column_right(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_recycle(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_toggle_hidden(this,event,window,cx); } }))
            .on_action(cx.listener(|this,_: &keys::SplitRight,window,cx| {if !this.interface_modal() {this.split_group(this.browser.active,Edge::Right,None,window,cx);}}))
            .on_action(cx.listener(|this,_: &keys::SplitDown,window,cx| {if !this.interface_modal() {this.split_group(this.browser.active,Edge::Bottom,None,window,cx);}}))
            .on_action(cx.listener(|this,_: &keys::ClosePane,window,cx| {if !this.interface_modal() {this.close_active_group(cx);window.focus(&this.focus_handle,cx);}}))
            .on_action(cx.listener(|this,_: &keys::ReopenTab,_,cx| {if !this.interface_modal() {this.reopen_tab(cx);}}))
            .on_action(cx.listener(|this,_: &keys::PreviousPane,window,cx| {if !this.interface_modal() {this.cycle_pane(true,window,cx);}}))
            .on_action(cx.listener(|this,_: &keys::PaneLeft,window,cx| {if !this.interface_modal() {this.directional_pane(Edge::Left,window,cx);}}))
            .on_action(cx.listener(|this,_: &keys::PaneRight,window,cx| {if !this.interface_modal() {this.directional_pane(Edge::Right,window,cx);}}))
            .on_action(cx.listener(|this,_: &keys::PaneUp,window,cx| {if !this.interface_modal() {this.directional_pane(Edge::Top,window,cx);}}))
            .on_action(cx.listener(|this,_: &keys::PaneDown,window,cx| {if !this.interface_modal() {this.directional_pane(Edge::Bottom,window,cx);}}))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_switch_pane(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_toggle_sidebar(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_toggle_inspector(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_view_list(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_view_columns(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_copy_path(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_select_all(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_clipboard_copy(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_clipboard_cut(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_clipboard_paste(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_undo(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_extend_next(this,event,window,cx); } }))
            .on_action(cx.listener(|this,event,window,cx| { if !this.interface_modal() { Self::key_extend_prev(this,event,window,cx); } }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Only when the file panes have focus, never inside inputs.
                let modifiers = event.keystroke.modifiers;
                if this.interface_modal() || !this.focus_handle.is_focused(window)
                    || modifiers.control || modifiers.alt || modifiers.platform {
                    return;
                }
                if event.keystroke.key == "space" && !modifiers.shift {
                    this.toggle_quick_look(cx);
                    cx.stop_propagation();
                    return;
                }
                if let Some(text) = event.keystroke.key_char.as_deref()
                    .filter(|t| t.chars().count() == 1 && t.chars().all(|c| !c.is_control() && c != ' '))
                {
                    this.type_ahead(text, cx);
                }
            }))
            // Atlas: the sidebar runs the full height; tabs, navigation,
            // panes and the bottom bar sit to its right.
            .child(div().flex_1().min_h_0().w_full().flex()
                .when(self.show_sidebar, |row| row.child(self.sidebar(cx)))
                .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                    .child(self.title_bar(window,cx))
                        .child(div().flex_1().min_h_0().overflow_hidden().child(panels))
                    .child(self.status_bar(cx))));
        if let Some(menu) = self.context_menu.clone() {
            root = root.child(self.context_menu_view(&menu, window, cx));
        }
        if let Some(dialog) = self.dialog(cx) {
            root = root.child(dialog);
        }
        if let Some(dialog) = self.conflict_dialog(cx) {
            root = root.child(dialog);
        }
        if let Some(dialog) = self.bulk_rename_dialog(cx) { root = root.child(dialog); }
        if let Some(rect)=self.marquee_rectangle() {root=root.child(rect);}
        if let Some(drag)=self.right_drag.as_ref().filter(|drag| drag.active) {
            let name = match drag.paths.as_slice() {
                [single] => browser::display_name(single),
                many => format!("{} элементов", many.len()),
            };
            root=root.child(div().absolute().left(drag.position.x+px(14.)).top(drag.position.y+px(10.))
                .px_3().py_1().rounded_md().bg(rgb(SELECTED)).border_1().border_color(rgb(SELECTED_BORDER))
                .text_color(rgb(SELECTION_TEXT)).text_size(px(13.)).child(name));
        }
        self.follow_quick_look(cx);
        if self.quick_look.is_some() { root=root.child(self.quick_look_view(cx)); }
        if self.comparison.is_some() { root=root.child(self.comparison_view(cx)); }
        if self.about_open { root=root.child(self.about_panel(cx)); }
        if let Some(dialog)=self.update_dialog(cx) { root=root.child(dialog); }
        if let Some(step)=self.tour { root=root.child(self.tour_view(step,cx)); }
        if let Some(overlay)=self.interface_overlay(cx) { root=root.child(overlay); }
        root
    }
}

fn outside_window(position: Point<Pixels>, window: &Window) -> bool {
    let size = window.viewport_size();
    position.x < px(0.) || position.y < px(0.) || position.x >= size.width || position.y >= size.height
}

/// The feature tour: icon, title, text. Shown once at first start and again
/// from «О программе» or the command palette.
const TOUR: &[(&str, &str, &str)] = &[
    ("fm/app.svg", "Добро пожаловать в Papka",
        "Короткий экскурс по возможностям — 7 шагов. Его можно открыть снова: кнопка «?» в заголовке окна \
        или палитра команд (Ctrl+K)."),
    ("fm/columns.svg", "Колонки и список",
        "Колонки показывают весь путь к папке: → открывает папку, ← возвращает к родительской. \
        Ctrl+1 — список с датами и размерами, Ctrl+2 — колонки; у каждой панели вид свой. \
        Боковые кнопки мыши листают историю, как в Проводнике."),
    ("fm/split.svg", "Панели и вкладки",
        "Ctrl+Shift+S делит окно справа, Ctrl+Alt+S — снизу, до четырёх панелей. У каждой панели свои вкладки: \
        Ctrl+T — новая, Ctrl+W — закрыть. Вкладку можно перетащить в другую панель или на край. \
        Закреплённая вкладка (правая кнопка → «Закрепить вкладку») не закрывается."),
    ("fm/layers.svg", "Проекты и избранное",
        "«Создать проект…» в боковой панели запоминает вкладки активной панели. Проект открывается в одной панели, \
        остальные не меняются; крестик на плашке проекта возвращает прежние вкладки. \
        Звезда в строке пути добавляет папку в избранное."),
    ("fm/move.svg", "Перетаскивание и удаление",
        "Перетащите файлы в другую панель: на том же диске — перенос, на другом — копия; с Ctrl — копия, с Shift — перенос. \
        Правой кнопкой — меню «Копировать / Переместить». За край окна — в Проводник или расчётную программу. \
        Del — в Корзину, Ctrl+Z — отменить."),
    ("fm/search.svg", "Поиск и команды",
        "Ctrl+F — поиск файлов, Ctrl+L — ввести путь, Ctrl+K — все команды с клавишами. \
        Набор в английской раскладке тоже находит: «yfcnh» — это «настр»."),
    ("fm/history.svg", "Версии и лента изменений",
        "Версии одного документа («Отчёт_v_1_6») показываются одной строкой — последней; стрелка у метки раскрывает прежние. \
        Ctrl+I открывает панель сведений с лентой изменений папки: что и когда появилось, изменилось или пропало. \
        Хранятся только сведения о файлах, не их копии."),
];
