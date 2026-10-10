//! Window layout and rendering. All state lives on `Explorer`; nothing
//! here reads the disk — listings arrive from background snapshots.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use filemanager_core::browser::{self, Entry};
use filemanager_core::format::{format_size, format_time, items_label, kind_label};
use filemanager_core::places::{containing_place, Place, PlaceKind};
use filemanager_core::sort::SortKey;
use gpui::{
    anchored, deferred, div, img, rgb_to_hsla, ObjectFit, ease_out_quint, prelude::*, px, rgb, rgba, svg, uniform_list,
    Animation, AnimationExt, AnyElement, App, ClickEvent, Context, Div, ElementId, Hsla,
    ExternalPaths, MouseButton, MouseDownEvent, SharedString, Stateful, Window,
};
use gpui_component::input::Input;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::scroll::ScrollableElement;
use gpui_component::theme::Theme;
use gpui_component::tooltip::Tooltip;

use crate::keys;
use crate::theme::*;
use crate::{ContextMenu, Explorer, FileDragInfo, FileDragPreview, MenuTarget, Side, TabDrag};

const ROW_HEIGHT: f32 = 28.;
const MILLER_COLUMNS: usize = 3;

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
    colors.selection = rgb_to_hsla(rgba(0x5E8BFF55));
    colors.popover = hsla(RAISED);
    colors.popover_foreground = hsla(TEXT);
    colors.primary = hsla(ACCENT);
    colors.accent = hsla(HOVER);
    colors.accent_foreground = hsla(TEXT);
    colors.scrollbar = rgb_to_hsla(rgba(0x00000000));
    colors.scrollbar_thumb = rgb_to_hsla(rgba(0xFFFFFF22));
    colors.scrollbar_thumb_hover = rgb_to_hsla(rgba(0xFFFFFF40));
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

fn section_title(text: &'static str) -> Div {
    div().px_3().pt_4().pb_1().text_size(px(11.)).text_color(rgb(TEXT_DIM))
        .font_weight(gpui::FontWeight::SEMIBOLD).child(text)
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

    fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div().id("tab-strip").flex_1().min_w_0().h_full().flex().items_end()
            .gap_1().px_2().overflow_x_scroll();
        for (index, tab) in self.browser.tabs.iter().enumerate() {
            let active = index == self.browser.active_tab;
            let closable = self.browser.tabs.len() > 1;
            strip = strip.child(
                div().id(("tab", index)).group("tab").flex_none().h(px(32.)).min_w(px(120.))
                    .max_w(px(220.)).pl_3().pr_1().flex().items_center().gap_2()
                    .rounded_t_md().cursor_pointer()
                    .when(active, |this| this.bg(rgb(SURFACE)).text_color(rgb(TEXT))
                        .border_t_2().border_color(rgb(ACCENT)))
                    .when(!active, |this| this.text_color(rgb(TEXT_MUTED))
                        .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT))))
                    .child(small_icon("fm/folder-fill.svg", if active { FOLDER } else { TEXT_DIM }))
                    .child(div().flex_1().min_w_0().truncate().child(tab.title.clone()))
                    .when(closable, |this| this.child(
                        div().id(("tab-close", index)).flex_none().size(px(20.)).rounded_sm()
                            .flex().items_center().justify_center()
                            .invisible().group_hover("tab", |style| style.visible())
                            .when(active, |this| this.visible())
                            .hover(|style| style.bg(rgb(PRESSED)))
                            .child(small_icon("fm/x.svg", TEXT_MUTED))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(index, cx);
                            }))
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.switch_tab(index, cx)))
                    .on_drag(TabDrag { index, title: tab.title.clone() }, |drag: &TabDrag, position, _, cx| {
                        cx.new(|_| FileDragPreview { name: drag.title.clone(), position })
                    })
                    .drag_over::<TabDrag>(|style, _, _, _| style.border_l_2().border_color(rgb(ACCENT)))
                    .on_drop(cx.listener(move |this, drag: &TabDrag, _, cx| {
                        this.browser.move_tab(drag.index, index);
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, _, cx| {
                        this.close_tab_at(index, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_menu(event.position, MenuTarget::Tab(index), cx);
                    }))
            );
        }
        strip.child(
            icon_button("new-tab", "fm/plus.svg", "Новая вкладка (Ctrl+T)", false, true)
                .mb(px(1.))
                .on_click(cx.listener(|this, _, _, cx| this.add_tab(cx)))
        )
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().w_full().h(px(40.)).flex_none().flex().items_end().bg(rgb(WINDOW))
            .border_b_1().border_color(rgb(BORDER))
            .child(
                div().flex_none().h_full().flex().items_center().pl_2()
                    .child(icon_button("toggle-sidebar", "fm/panel-left.svg", "Боковая панель (Ctrl+B)", self.show_sidebar, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_sidebar = !this.show_sidebar;
                            cx.notify();
                        })))
            )
            .child(self.tab_strip(cx))
            .child(
                div().flex_none().h_full().flex().items_center().pr_2()
                    .child(icon_button("toggle-inspector", "fm/panel-right.svg", "Панель сведений (Ctrl+I)", self.show_inspector, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_inspector = !this.show_inspector;
                            cx.notify();
                        })))
            )
    }

    // ------------------------------------------------------------- toolbar

    fn breadcrumbs(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.address_editing {
            return div().flex_1().min_w_0().h(px(30.))
                .child(Input::new(&self.address_input).h(px(30.)))
                .into_any_element();
        }
        let side = self.active_side();
        let path = self.pane_path(side);
        let chain = browser::ancestors_from_root(&path);
        // Long paths keep the root and the last folders, like Explorer.
        let skip = if chain.len() > 6 { chain.len() - 4 } else { 0 };
        let mut crumbs = div().id("breadcrumbs").flex_1().min_w_0().h(px(30.)).px_1()
            .flex().items_center().gap_0p5().overflow_hidden().rounded_md()
            .bg(rgb(SURFACE)).border_1().border_color(rgb(BORDER)).cursor_text()
            .on_click(cx.listener(|this, _, window, cx| {
                this.address_editing = true;
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
                div().id(("crumb", index)).flex_none().max_w(px(220.)).truncate().px_1p5().py_0p5()
                    .rounded_sm().cursor_pointer()
                    .text_color(rgb(if last { TEXT } else { TEXT_MUTED }))
                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .child(browser::display_name(folder))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let side = this.active_side();
                        this.navigate_side(side, &target, None, cx);
                    }))
            );
        }
        crumbs.into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.browser.active().active();
        let can_back = pane.can_go_back();
        let can_forward = pane.can_go_forward();
        let can_up = pane.path.parent().is_some();
        let split = self.browser.active().right.is_some();
        div().w_full().h(px(46.)).flex_none().flex().items_center().gap_1().px_2()
            .bg(rgb(SURFACE)).border_b_1().border_color(rgb(BORDER))
            .child(icon_button("back", "fm/arrow-left.svg", "Назад (Alt+←)", false, can_back)
                .on_click(cx.listener(|this, _, _, cx| this.go_back(false, cx))))
            .child(icon_button("forward", "fm/arrow-right.svg", "Вперёд (Alt+→)", false, can_forward)
                .on_click(cx.listener(|this, _, _, cx| this.go_back(true, cx))))
            .child(icon_button("up", "fm/arrow-up.svg", "Вверх (Backspace)", false, can_up)
                .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))))
            .child(icon_button("refresh", "fm/refresh.svg", "Обновить (F5)", false, true)
                .on_click(cx.listener(|this, _, _, cx| this.refresh_visible_directories(cx))))
            .child(div().w_2())
            .child(self.breadcrumbs(cx))
            .child(div().w_2())
            .child(
                div().flex_none().flex().items_center().p_0p5().gap_0p5().rounded_md().bg(rgb(PANEL))
                    .child(icon_button("view-list", "fm/list.svg", "Список (Ctrl+1)", !self.miller_mode, true)
                        .on_click(cx.listener(|this, _, _, cx| this.set_miller(false, cx))))
                    .child(icon_button("view-columns", "fm/columns.svg", "Колонки (Ctrl+2)", self.miller_mode, true)
                        .on_click(cx.listener(|this, _, _, cx| this.set_miller(true, cx))))
            )
            .child(icon_button("split", "fm/split.svg", "Разделить окно (Ctrl+\\)", split, true)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_split(cx))))
            .child(icon_button("hidden", if self.show_hidden { "fm/eye.svg" } else { "fm/eye-off.svg" },
                "Скрытые файлы (Ctrl+H)", self.show_hidden, true)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_hidden = !this.show_hidden;
                    cx.notify();
                })))
            .child(div().w_1())
            .child(
                div().flex_none().w(px(240.))
                    .child(Input::new(&self.search_input).h(px(30.))
                        .prefix(small_icon("fm/search.svg", TEXT_DIM)).cleanable(true))
            )
    }

    // ------------------------------------------------------------- sidebar

    fn place_row(&self, id: (&'static str, usize), place: &Place, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let path = place.path.clone();
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
            .on_drop(cx.listener(|this, data: &FileDragInfo, _, cx| this.stage_paths(&data.paths, cx)))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.pane_path(self.active_side());
        let all: Vec<Place> = self.places.iter().chain(self.drives.iter()).cloned().collect();
        let active_place = containing_place(&all, &current).map(|place| place.path.clone());
        let mut side = div().id("sidebar").size_full().min_h_0().flex().flex_col().pb_3()
            .overflow_y_scrollbar().bg(rgb(PANEL)).text_size(px(13.))
            .child(section_title("ИЗБРАННОЕ"));
        for (index, place) in self.places.iter().enumerate() {
            let active = active_place.as_ref() == Some(&place.path);
            side = side.child(self.place_row(("place", index), place, active, cx));
        }
        side = side.child(section_title("ДИСКИ"));
        for (index, place) in self.drives.iter().enumerate() {
            let active = active_place.as_ref() == Some(&place.path);
            side = side.child(self.place_row(("drive", index), place, active, cx));
            if let Some(&(free, total)) = self.drive_space.get(&place.path) {
                let used = 1. - free as f32 / total as f32;
                let color = if used > 0.9 { DANGER } else { ACCENT };
                side = side.child(
                    div().mx_2().pl(px(32.)).pr_2().pb_1().flex().flex_col().gap_1()
                        .child(div().h(px(4.)).w_full().rounded_full().bg(rgb(BORDER))
                            .child(div().h_full().rounded_full().bg(rgb(color)).w(gpui::relative(used))))
                        .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                            .child(format!("свободно {} из {}", format_size(free), format_size(total))))
                );
            }
        }
        side = side.child(section_title("DROP ZONE")).child(self.drop_zone(cx));
        side = side.child(section_title("РАБОЧИЕ ПРОСТРАНСТВА"));
        for (index, name) in self.workspace_names.iter().enumerate() {
            let open_name = name.clone();
            let delete_name = name.clone();
            side = side.child(
                div().id(("workspace", index)).group("workspace").mx_2().h(px(28.)).px_2().flex().items_center()
                    .gap_2().rounded_md().cursor_pointer().text_color(rgb(TEXT_MUTED))
                    .hover(|style| style.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .child(icon("fm/layers.svg", TEXT_MUTED))
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .child(div().id(("workspace-delete", index)).size(px(20.)).rounded_sm().flex()
                        .items_center().justify_center().invisible().group_hover("workspace", |style| style.visible())
                        .hover(|style| style.bg(rgb(PRESSED)))
                        .child(small_icon("fm/x.svg", TEXT_DIM))
                        .tooltip(|window, cx| Tooltip::new("Удалить рабочее пространство").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_workspace(&delete_name, cx);
                        })))
                    .on_click(cx.listener(move |this, _, _, cx| this.restore_workspace(&open_name, cx)))
            );
        }
        side = side
            .child(self.sidebar_action("save-workspace", "fm/plus.svg", "Сохранить вкладки как…",
                cx.listener(|this, _, window, cx| this.begin_save_workspace(window, cx))))
            .child(self.sidebar_action("operation-review", "fm/history.svg", "Журнал операций",
                cx.listener(|this, _, _, cx| this.toggle_operation_review(cx))));
        side.into_any_element()
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
            .child(label)
            .on_click(click)
    }

    fn drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.zone.items();
        let mut zone = div().id("drop-zone").mx_3().mt_1().p_2().rounded_lg().flex().flex_col().gap_1()
            .border_1().border_dashed().border_color(rgb(BORDER_STRONG)).bg(rgb(SURFACE))
            .drag_over::<FileDragInfo>(|style, _, _, _| style.border_color(rgb(ACCENT)).bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener(|this, data: &FileDragInfo, _, cx| this.stage_paths(&data.paths, cx)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.border_color(rgb(ACCENT)).bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener(|this, data: &ExternalPaths, _, cx| this.stage_paths(data.paths(), cx)));
        if items.is_empty() {
            zone = zone.child(
                div().py_2().flex().flex_col().items_center().gap_1().text_color(rgb(TEXT_DIM))
                    .text_size(px(12.))
                    .child(icon("fm/inbox.svg", TEXT_DIM))
                    .child("Перетащите сюда файлы")
            );
            return zone;
        }
        for path in items.iter().take(5) {
            zone = zone.child(
                div().flex().items_center().gap_2().text_size(px(12.)).text_color(rgb(TEXT))
                    .child(small_icon(if path.is_dir() { "fm/folder-fill.svg" } else { "fm/file.svg" },
                        if path.is_dir() { FOLDER } else { TEXT_MUTED }))
                    .child(div().flex_1().min_w_0().truncate().child(browser::display_name(path)))
            );
        }
        if items.len() > 5 {
            zone = zone.child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                .child(format!("и ещё {}", items.len() - 5)));
        }
        zone.child(
            div().pt_1().flex().gap_1()
                .child(text_button("zone-copy", "Копировать сюда", ButtonKind::Primary).flex_1().text_size(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| this.paste(cx))))
                .child(text_button("zone-move", "Переместить", ButtonKind::Ghost).text_size(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| this.move_staged(cx))))
        ).child(
            div().id("zone-clear").text_size(px(12.)).text_color(rgb(TEXT_DIM)).cursor_pointer()
                .hover(|style| style.text_color(rgb(TEXT)))
                .child("Очистить")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.zone.clear();
                    this.status = "Drop Zone очищена, файлы не изменены".into();
                    cx.notify();
                }))
        )
    }

    // --------------------------------------------------------------- panes

    /// Folders shown in a pane: the trail of the current folder and, in
    /// columns mode, a preview of the selected subfolder.
    pub(crate) fn pane_columns(&self, side: Side) -> Vec<PathBuf> {
        let path = self.pane_path(side);
        if !self.miller_mode {
            return vec![path];
        }
        let preview = (side == self.active_side()).then(|| self.selected.clone()).flatten()
            .filter(|selected| {
                selected.parent() == Some(path.as_path())
                    && self.directory_cache.get(&path).is_some_and(|listing| {
                        listing.entries.iter().any(|entry| &entry.path == selected && entry.is_directory)
                    })
            });
        // As many columns as fit at a readable width (at least two). A
        // selected subfolder is previewed as the last column; the trail
        // gives up its oldest column so the total stays readable.
        let fit = ((self.pane_width / 210.) as usize).clamp(2, MILLER_COLUMNS);
        let trail = if preview.is_some() { fit - 1 } else { fit };
        let mut columns: Vec<PathBuf> = path.ancestors().take(trail).map(Path::to_path_buf).collect();
        columns.reverse();
        columns.extend(preview);
        columns
    }

    fn pane(&mut self, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let split = self.browser.active().right.is_some();
        let focused = side == self.active_side();
        let path = self.pane_path(side);
        let body = if self.miller_mode {
            let folders = self.pane_columns(side);
            let group_id = format!(
                "miller-{}-{}-{}-{}",
                self.browser.active_tab,
                if side == Side::Left { "l" } else { "r" },
                folders.len(),
                self.pane_width as u32 / 40,
            );
            let minimum_width = px(180. * folders.len() as f32);
            let mut group = h_resizable(group_id);
            for (index, folder) in folders.iter().enumerate() {
                // The folder of the next column is highlighted as the trail.
                let trail = folders.get(index + 1).cloned();
                let column = self.column(folder.clone(), side, trail, cx);
                let width = (self.pane_width / folders.len() as f32).max(180.);
                group = group.child(resizable_panel().size(px(width)).size_range(px(160.)..px(900.)).child(column));
            }
            div().id(if side == Side::Left { "columns-left" } else { "columns-right" })
                .flex_1().min_h_0().overflow_x_scrollbar()
                .child(div().w_full().min_w(minimum_width).h_full().child(group))
                .into_any_element()
        } else {
            div().flex_1().min_h_0().flex().flex_col()
                .child(self.list_header(cx))
                .child(self.column(path.clone(), side, None, cx))
                .into_any_element()
        };
        div().size_full().flex().flex_col().overflow_hidden().bg(rgb(SURFACE))
            .when(split, |this| this.child(
                div().h(px(28.)).flex_none().px_3().flex().items_center().gap_2()
                    .border_b_1().border_color(rgb(BORDER))
                    .bg(rgb(if focused { RAISED } else { PANEL }))
                    .when(focused, |this| this.border_t_2().border_color(rgb(ACCENT)))
                    .child(small_icon("fm/folder-fill.svg", if focused { FOLDER } else { TEXT_DIM }))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.))
                        .text_color(rgb(if focused { TEXT } else { TEXT_MUTED }))
                        .child(path.display().to_string()))
            ))
            .child(body)
            // Files from Explorer are copied into the pane's folder.
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)))
            .on_drop(cx.listener({
                let folder = path.clone();
                move |this, data: &ExternalPaths, _, cx| {
                    this.drop_into(data.paths(), folder.clone(), true, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                let tab = this.browser.active_mut();
                let target = side == Side::Right && tab.right.is_some();
                if tab.focus_right != target {
                    tab.focus_right = target;
                    this.selected = None;
                }
                let handle = this.focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// Narrow panes drop the type and date columns, keeping names readable.
    fn list_columns(&self) -> (bool, bool) {
        (self.pane_width >= 520., self.pane_width >= 680.)
    }

    fn list_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = self.sort;
        let (show_modified, show_kind) = self.list_columns();
        let header = |id: &'static str, label: &'static str, key: SortKey| {
            let active = spec.key == key;
            div().id(id).h_full().px_2().flex().items_center().gap_1().cursor_pointer()
                .text_color(rgb(if active { TEXT } else { TEXT_DIM }))
                .hover(|style| style.text_color(rgb(TEXT)))
                .child(label)
                .when(active, |this| this.child(if spec.descending { "↓" } else { "↑" }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sort = this.sort.toggled(key);
                    cx.notify();
                }))
        };
        div().h(px(28.)).flex_none().flex().items_center().gap_2().pl(px(36.)).pr(px(12.))
            .border_b_1().border_color(rgb(BORDER)).text_size(px(12.)).bg(rgb(SURFACE))
            .child(header("sort-name", "Имя", SortKey::Name).flex_1().min_w_0())
            .when(show_modified, |this| this.child(header("sort-modified", "Изменён", SortKey::Modified).w(px(140.)).flex_none()))
            .when(show_kind, |this| this.child(header("sort-kind", "Тип", SortKey::Kind).w(px(110.)).flex_none()))
            .child(header("sort-size", "Размер", SortKey::Size).w(px(90.)).flex_none().justify_end())
    }

    fn column(&mut self, folder: PathBuf, side: Side, trail: Option<PathBuf>, cx: &mut Context<Self>) -> AnyElement {
        let list_mode = !self.miller_mode;
        let mut column = div().size_full().min_h_0().flex().flex_col()
            .when(!list_mode, |this| this.border_r_1().border_color(rgb(BORDER)));
        let background_folder = folder.clone();
        column = column.on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.open_menu(event.position, MenuTarget::Folder(background_folder.clone()), cx);
        }));

        if let Some((listing, indices)) = self.folder_view(&folder) {
            if indices.is_empty() {
                return column.child(
                    div().flex_1().flex().flex_col().items_center().justify_center().gap_2()
                        .text_color(rgb(TEXT_DIM)).text_size(px(12.))
                        .child(icon("fm/folder-fill.svg", BORDER_STRONG))
                        .child(if listing.entries.is_empty() { "Папка пуста" } else { "Только скрытые файлы" })
                ).into_any_element();
            }
            let handle = self.scroll_handle(&folder);
            let count = indices.len();
            let list_id = SharedString::from(format!("list-{}", folder.display()));
            column = column.child(
                div().flex_1().min_h_0().child(
                    uniform_list(list_id, count, cx.processor({ let listing = listing.clone(); move |this, range: Range<usize>, _, cx| {
                        range.map(|position| {
                            let entry = &listing.entries[indices[position]];
                            let on_trail = trail.as_ref() == Some(&entry.path);
                            this.row(entry, &folder, side, on_trail, list_mode, cx)
                        }).collect::<Vec<_>>()
                    }}))
                    .track_scroll(&handle)
                    .h_full()
                    .px_1()
                )
            );
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

    fn row(&self, entry: &Entry, folder: &Path, side: Side, on_trail: bool, list_mode: bool, cx: &mut Context<Self>) -> AnyElement {
        let path = entry.path.clone();
        let is_dir = entry.is_directory;
        let selected = self.is_selected(&path);
        let focused_pane = side == self.active_side();
        let (icon_path, tint) = entry_icon(entry);
        let bg = if selected && focused_pane {
            Some(SELECTED)
        } else if selected || on_trail {
            Some(TRAIL)
        } else {
            None
        };
        let text = if entry.hidden { TEXT_MUTED } else { TEXT };
        let mut row = div().id(SharedString::from(format!("row-{}", path.display())))
            .w_full().h(px(ROW_HEIGHT)).px_2().flex().items_center().gap_2().rounded_md()
            .text_size(px(13.)).text_color(rgb(text)).cursor_pointer()
            .when_some(bg, |this, bg| this.bg(rgb(bg)))
            .when(bg.is_none(), |this| this.hover(|style| style.bg(rgb(HOVER))))
            .child(icon(icon_path, tint).when(entry.hidden, |svg| svg.opacity(0.55)))
            .child(div().flex_1().min_w_0().truncate().child(entry.name.clone()));
        if list_mode {
            let (show_modified, show_kind) = self.list_columns();
            row = row
                .when(show_modified, |row| row.child(div().w(px(140.)).flex_none().px_2().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(entry.modified.map(format_time).unwrap_or_default())))
                .when(show_kind, |row| row.child(div().w(px(110.)).flex_none().px_2().truncate().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(kind_label(entry))))
                .child(div().w(px(90.)).flex_none().px_2().flex().justify_end().text_size(px(12.))
                    .text_color(rgb(TEXT_MUTED))
                    .child(if is_dir { String::new() } else { format_size(entry.size) }));
        } else if is_dir {
            row = row.child(small_icon("fm/chevron-right.svg", TEXT_DIM));
        }
        if is_dir {
            let internal_target = path.clone();
            let external_target = path.clone();
            row = row
                .drag_over::<FileDragInfo>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)).border_1().border_color(rgb(ACCENT)))
                .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)).border_1().border_color(rgb(ACCENT)))
                // Inside the app a drop moves (Ctrl copies); from Explorer it copies.
                .on_drop(cx.listener(move |this, data: &FileDragInfo, window, cx| {
                    cx.stop_propagation();
                    let copy = window.modifiers().control;
                    this.drop_into(&data.paths, internal_target.clone(), copy, cx);
                }))
                .on_drop(cx.listener(move |this, data: &ExternalPaths, _, cx| {
                    cx.stop_propagation();
                    this.drop_into(data.paths(), external_target.clone(), true, cx);
                }));
        }
        let column_folder = folder.to_path_buf();
        let menu_path = path.clone();
        let menu_folder = folder.to_path_buf();
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
                this.open_menu(event.position, MenuTarget::Entry(menu_path.clone()), cx);
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.open_entry(path.clone(), is_dir, side, cx);
                } else {
                    this.click_entry(side, &column_folder, path.clone(), event.modifiers(), cx);
                }
            }))
            .into_any_element()
    }

    fn search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let root = self.search_root.clone();
        let mut rows = div().id("search-results").size_full().min_h_0().flex().flex_col()
            .overflow_y_scrollbar().bg(rgb(SURFACE)).py_1()
            .child(
                div().px_3().py_2().flex().items_center().gap_2().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(small_icon("fm/search.svg", TEXT_MUTED))
                    .child(format!("Результаты поиска «{}»", self.search_query.trim()))
            );
        if self.search_busy {
            rows = rows.child(div().px_3().py_2().text_color(rgb(TEXT_DIM)).child("Ищем в индексе…"));
        } else if self.search_results.is_empty() {
            rows = rows.child(div().px_3().py_2().text_color(rgb(TEXT_DIM))
                .child("Ничего не найдено. Поиск идёт по именам файлов внутри текущей папки."));
        }
        for (index, path) in self.search_results.iter().enumerate() {
            let target = path.clone();
            let relative = root.as_deref().and_then(|root| path.strip_prefix(root).ok())
                .map(|p| p.display().to_string()).unwrap_or_else(|| path.display().to_string());
            let is_dir = path.extension().is_none() && path.is_dir();
            let (icon_path, tint) = if is_dir { ("fm/folder-fill.svg", FOLDER) }
                else { file_icon(path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref()) };
            rows = rows.child(
                div().id(("search-hit", index)).h(px(ROW_HEIGHT)).mx_1().px_2().flex().items_center().gap_2()
                    .rounded_md().cursor_pointer().hover(|style| style.bg(rgb(HOVER)))
                    .child(icon(icon_path, tint))
                    .child(div().flex_none().max_w(px(280.)).truncate().child(browser::display_name(path)))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child(relative))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_search_result(target.clone(), cx)))
            );
        }
        rows.into_any_element()
    }

    // ----------------------------------------------------------- inspector

    fn property(label: &'static str, value: impl Into<SharedString>) -> impl IntoElement {
        div().flex().gap_2().text_size(px(12.))
            .child(div().w(px(76.)).flex_none().text_color(rgb(TEXT_DIM)).child(label))
            .child(div().flex_1().min_w_0().text_color(rgb(TEXT)).child(value.into()))
    }

    fn inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div().id("inspector").size_full().min_h_0().flex().flex_col().gap_3().p_4()
            .overflow_y_scrollbar().bg(rgb(PANEL)).text_size(px(13.));
        if self.operation_review {
            panel = panel.child(div().flex().items_center().justify_between()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Незавершённые операции"))
                    .child(icon_button("close-review", "fm/x.svg", "Закрыть", false, true)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_operation_review(cx)))))
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child("Список только читает журнал SQLite и ничего не меняет. Перед повтором проверьте источник и назначение в Проводнике."));
            if self.operation_alerts.is_empty() {
                panel = panel.child(div().text_color(rgb(TEXT_DIM)).child("Незавершённых записей нет."));
            }
            for entry in &self.operation_alerts {
                panel = panel.child(
                    div().p_2().rounded_md().bg(rgb(RAISED)).flex().flex_col().gap_1().text_size(px(12.))
                        .child(format!("#{} · {} · {}", entry.id, entry.action, entry.status))
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
                if let Some((listing, _)) = self.folder_view(&folder) {
                    for entry in listing.entries.iter().filter(|e| marked.contains(&e.path)) {
                        if entry.is_directory { folders += 1 } else { files += 1; bytes += entry.size; }
                    }
                }
            }
            return panel
                .child(div().flex().flex_col().items_center().gap_2().pt_2()
                    .child(svg().path("fm/layers.svg").size(px(56.)).text_color(rgb(ACCENT)))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("Выбрано: {}", marked.len()))))
                .child(div().flex().flex_col().gap_1p5()
                    .child(Self::property("Файлов", files.to_string()))
                    .child(Self::property("Папок", folders.to_string()))
                    .child(Self::property("Размер", format!("{} (без папок)", format_size(bytes)))))
                .child(div().flex().gap_2()
                    .child(text_button("group-stage", "В Drop Zone", ButtonKind::Primary).flex_1()
                        .on_click(cx.listener(|this, _, _, cx| this.stage(cx))))
                    .child(text_button("group-recycle", "В Корзину", ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _, _, cx| this.recycle(cx)))))
                .into_any_element();
        }
        let selected_entry = self.selected.clone().and_then(|selected| {
            let folder = selected.parent()?.to_path_buf();
            let (listing, _) = self.folder_view(&folder)?;
            listing.entries.iter().find(|entry| entry.path == selected).cloned()
        });
        let current = self.pane_path(self.active_side());
        let (title, icon_path, tint) = match &selected_entry {
            Some(entry) => {
                let (icon_path, tint) = entry_icon(entry);
                (entry.name.clone(), icon_path, tint)
            }
            None => (browser::display_name(&current), "fm/folder-fill.svg", FOLDER),
        };
        panel = panel.child(
            div().flex().flex_col().items_center().gap_2().pt_2()
                .child(svg().path(icon_path).size(px(56.)).text_color(rgb(tint)))
                .child(div().w_full().text_center().font_weight(gpui::FontWeight::SEMIBOLD)
                    .overflow_hidden().child(title))
        );
        match &selected_entry {
            Some(entry) => {
                let mut props = div().flex().flex_col().gap_1p5()
                    .child(Self::property("Тип", kind_label(entry)));
                if !entry.is_directory {
                    props = props.child(Self::property("Размер", format!("{} ({} байт)", format_size(entry.size), entry.size)));
                }
                if let Some(modified) = entry.modified {
                    props = props.child(Self::property("Изменён", format_time(modified)));
                }
                props = props.child(Self::property("Путь", entry.path.display().to_string()));
                panel = panel.child(props);
                let open_path = entry.path.clone();
                let is_dir = entry.is_directory;
                let copy_target = entry.path.clone();
                panel = panel.child(
                    div().flex().gap_2()
                        .child(text_button("inspector-open", "Открыть", ButtonKind::Primary).flex_1()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let side = this.active_side();
                                this.open_entry(open_path.clone(), is_dir, side, cx);
                            })))
                        .child(text_button("inspector-copy-path", "Копировать путь", ButtonKind::Ghost)
                            .on_click(cx.listener(move |this, _, _, cx| this.copy_path(&copy_target, cx))))
                );
                if let Some(undo) = self.undo_stack.last() {
                    panel = panel.child(
                        text_button("undo-last", format!("Отменить: {}", undo.label), ButtonKind::Ghost)
                            .on_click(cx.listener(|this, _, _, cx| this.undo_last(cx)))
                    );
                }
                panel = panel.child(self.preview_section(entry));
            }
            None => {
                let count = self.folder_view(&current).map(|(_, indices)| indices.len());
                panel = panel.child(div().flex().flex_col().gap_1p5()
                    .child(Self::property("Папка", current.display().to_string()))
                    .when_some(count, |this, count| this.child(Self::property("Содержит", items_label(count)))));
            }
        }
        panel = panel.child(self.history_section(cx));
        panel.into_any_element()
    }

    fn preview_section(&self, entry: &Entry) -> AnyElement {
        // Images are decoded by GPUI off the UI thread and cached; very
        // large files are skipped so a 500 MB TIFF cannot eat memory.
        const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
        let is_image = matches!(entry.extension().as_deref(),
            Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "tif" | "tiff"));
        if is_image && entry.size <= IMAGE_LIMIT {
            return div().flex().flex_col().gap_1()
                .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child("ПРЕДПРОСМОТР · ИЗОБРАЖЕНИЕ"))
                .child(div().w_full().h(px(220.)).p_1().rounded_md().bg(rgb(SURFACE))
                    .border_1().border_color(rgb(BORDER)).flex().items_center().justify_center()
                    .child(img(entry.path.clone()).size_full().object_fit(ObjectFit::Contain)
                        .with_fallback(|| div().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                            .child("Не удалось показать изображение").into_any_element())))
                .into_any_element();
        }
        if self.inspector_loading {
            return div().text_size(px(12.)).text_color(rgb(TEXT_DIM)).child("Загрузка предпросмотра…").into_any_element();
        }
        match &self.inspector_preview {
            Some((kind, description)) if self.inspector_path == self.selected && kind != "folder" => div().flex().flex_col().gap_1()
                .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child(match kind.as_str() {
                    "text" => "ПРЕДПРОСМОТР · ТЕКСТ",
                    "image" => "ПРЕДПРОСМОТР · ИЗОБРАЖЕНИЕ",
                    "binary" => "ПРЕДПРОСМОТР · ДВОИЧНЫЙ ФАЙЛ",
                    _ => "СВЕДЕНИЯ",
                }))
                .child(div().id("preview-text").max_h(px(240.)).overflow_y_scrollbar().p_2().rounded_md()
                    .bg(rgb(SURFACE)).border_1().border_color(rgb(BORDER))
                    .text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child(description.clone()))
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn history_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let watching = self.watched_root.is_some();
        let mut section = div().flex().flex_col().gap_2().pt_2().border_t_1().border_color(rgb(BORDER))
            .child(div().flex().items_center().justify_between()
                .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child("ИСТОРИЯ ИЗМЕНЕНИЙ"))
                .child(text_button("watch-toggle", if watching { "Остановить" } else { "Следить за папкой" },
                    ButtonKind::Ghost).text_size(px(12.))
                    .on_click(cx.listener(|this, _, _, cx| this.watch(cx)))));
        if let Some(root) = &self.watched_root {
            section = section.child(div().flex().items_center().gap_2().text_size(px(12.)).text_color(rgb(SUCCESS))
                .child(small_icon("fm/eye-watch.svg", SUCCESS))
                .child(div().flex_1().min_w_0().truncate().child(format!("Наблюдение: {}", root.display()))));
        }
        if self.selected.is_none() || self.journal.is_none() {
            return section.into_any_element();
        }
        if self.inspector_history.is_empty() && !self.inspector_loading {
            section = section.child(div().text_size(px(12.)).text_color(rgb(TEXT_DIM))
                .child("Изменений не записано. Хранятся только метаданные, без копий файлов."));
        }
        for event in &self.inspector_history {
            let event_id = event.id;
            let saved_comment = event.comment.clone();
            let saved_author = event.author.clone().unwrap_or_default();
            let chosen = self.selected_history_event == Some(event_id);
            section = section.child(
                div().id(("history-event", event_id as usize)).p_2().rounded_md().cursor_pointer()
                    .flex().flex_col().gap_0p5().text_size(px(12.))
                    .bg(rgb(if chosen { ACCENT_SOFT } else { SURFACE }))
                    .border_1().border_color(rgb(if chosen { SELECTED_BORDER } else { BORDER }))
                    .hover(|style| style.border_color(rgb(BORDER_STRONG)))
                    .child(div().text_color(rgb(TEXT)).child(format!("{} · {}", event.kind_label(), event.display_time())))
                    .child(div().text_color(rgb(TEXT_MUTED)).child(format!("Автор: {}",
                        event.author.as_deref().unwrap_or("не указан"))))
                    .child(div().text_color(rgb(TEXT_DIM)).child(format!("Учётная запись: {}", event.recorded_by)))
                    .when(!event.comment.is_empty(), |this| this.child(
                        div().text_color(rgb(TEXT)).child(event.comment.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected_history_event = Some(event_id);
                        this.comment_input.update(cx, |input, cx| input.set_value(saved_comment.clone(), window, cx));
                        this.author_input.update(cx, |input, cx| input.set_value(saved_author.clone(), window, cx));
                        cx.notify();
                    }))
            );
        }
        if self.selected_history_event.is_some() {
            section = section
                .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child("АВТОР"))
                .child(Input::new(&self.author_input))
                .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child("КОММЕНТАРИЙ"))
                .child(Input::new(&self.comment_input))
                .child(text_button("save-comment", "Сохранить комментарий", ButtonKind::Primary)
                    .on_click(cx.listener(|this, _, _, cx| this.save_comment(cx))));
        }
        section.into_any_element()
    }

    // -------------------------------------------------------- status bar

    fn status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.pane_path(self.active_side());
        let count = self.folder_view(&current).map(|(_, indices)| indices.len());
        let marked = self.marked.len();
        let selection = if marked > 1 { Some(format!("Выбрано: {marked}")) } else { self.selected.clone().and_then(|selected| {
            let folder = selected.parent()?.to_path_buf();
            let (listing, _) = self.folder_view(&folder)?;
            listing.entries.iter().find(|entry| entry.path == selected).map(|entry| {
                if entry.is_directory { format!("Выбрано: {}", entry.name) }
                else { format!("Выбрано: {} · {}", entry.name, format_size(entry.size)) }
            })
        }) };
        let mut bar = div().w_full().h(px(28.)).flex_none().px_3().flex().items_center().gap_4()
            .bg(rgb(WINDOW)).border_t_1().border_color(rgb(BORDER))
            .text_size(px(12.)).text_color(rgb(TEXT_MUTED))
            .when_some(count, |this, count| this.child(items_label(count)))
            .when_some(selection, |this, text| this.child(div().max_w(px(360.)).truncate().child(text)))
            .child(div().flex_1().min_w_0().truncate().text_color(rgb(TEXT_DIM))
                .child(crate::messages::localize(&self.status)));
        if self.copy_in_progress {
            bar = bar.child(
                div().id("cancel-copy").px_2().rounded_sm().cursor_pointer().text_color(rgb(DANGER))
                    .hover(|style| style.bg(rgb(HOVER)))
                    .child("Отменить копирование")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_copy(cx)))
            );
        }
        if !self.zone.items().is_empty() {
            bar = bar.child(div().flex().items_center().gap_1()
                .child(small_icon("fm/inbox.svg", ACCENT))
                .child(format!("Drop Zone: {}", self.zone.items().len())));
        }
        if self.watched_root.is_some() {
            bar = bar.child(div().flex().items_center().gap_1().text_color(rgb(SUCCESS))
                .child(small_icon("fm/eye-watch.svg", SUCCESS)).child("Наблюдение"));
        }
        bar
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
        div().id(id).h(px(28.)).px_2().flex().items_center().gap_2().rounded_md()
            .text_size(px(13.)).text_color(rgb(color))
            .when(enabled, |this| this.cursor_pointer().hover(|style| style.bg(rgb(if danger { 0x3A2128 } else { HOVER }))))
            .child(small_icon(icon_path, if !enabled { TEXT_DIM } else if danger { DANGER } else { TEXT_MUTED }))
            .child(div().flex_1().child(label.into()))
            .child(div().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child(shortcut))
    }

    fn separator() -> Div {
        div().my_1().h(px(1.)).bg(rgb(BORDER))
    }

    fn context_menu_view(&self, menu: &ContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().id(("context-menu", menu.serial as usize)).w(px(280.)).p_1().flex().flex_col()
            .rounded_lg().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }));
        let staged = self.zone.items().len();
        match &menu.target {
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
                let copy_target = path.clone();
                let stage_target = self.selection();
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
                    .child(Self::menu_item("m-copy-path", "fm/clipboard.svg", "Копировать путь", "Ctrl+Shift+C", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.copy_path(&copy_target, cx);
                        })))
                    .child(Self::menu_item("m-stage", "fm/inbox.svg",
                        if count > 1 { format!("Добавить в Drop Zone ({count})") } else { "Добавить в Drop Zone".into() }, "Ctrl+Shift+S", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.stage_paths(&stage_target, cx);
                        })))
                    .child(Self::separator())
                    .child(Self::menu_item("m-recycle", "fm/trash.svg",
                        if count > 1 { format!("Удалить в Корзину ({count})") } else { "Удалить в Корзину".into() }, "Del", true, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.recycle(cx);
                        })));
            }
            MenuTarget::Folder(folder) => {
                let copy_target = folder.clone();
                let tab_target = folder.clone();
                let (f1, f2, f3, f4) = (folder.clone(), folder.clone(), folder.clone(), folder.clone());
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
                        format!("Копировать сюда из Drop Zone ({staged})"), "", false, staged > 0)
                        .when(staged > 0, |this| this.on_click(cx.listener(move |this, _, _, cx| {
                            this.enter_folder(&f3, cx);
                            this.paste(cx);
                        }))))
                    .child(Self::menu_item("m-move-here", "fm/move.svg", "Переместить сюда из Drop Zone", "", false, staged > 0)
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
                        })));
            }
            MenuTarget::Tab(index) => {
                let index = *index;
                let many = self.browser.tabs.len() > 1;
                list = list
                    .child(Self::menu_item("m-tab-new", "fm/plus.svg", "Новая вкладка", "Ctrl+T", false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.add_tab(cx);
                        })))
                    .child(Self::menu_item("m-tab-duplicate", "fm/copy.svg", "Дублировать", "", false, true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            if let Some(path) = this.browser.tabs.get(index).map(|tab| tab.active().path.clone()) {
                                this.open_in_new_tab(&path, cx);
                            }
                        })))
                    .child(Self::separator())
                    .child(Self::menu_item("m-tab-close", "fm/x.svg", "Закрыть", "Ctrl+W", false, many)
                        .when(many, |this| this.on_click(cx.listener(move |this, _, _, cx| this.close_tab_at(index, cx)))))
                    .child(Self::menu_item("m-tab-close-others", "fm/x.svg", "Закрыть другие", "", false, many)
                        .when(many, |this| this.on_click(cx.listener(move |this, _, _, cx| {
                            this.context_menu = None;
                            this.close_other_tabs(index, cx);
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

    fn conflict_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (transfer, conflicts) = self.pending_transfer.as_ref()?;
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
            .rounded_xl().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD)
                .child(format!("Совпадают имена: {}", conflicts.len())))
            .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                .child(format!("В папке «{}» уже есть объекты с такими именами. Существующие файлы не заменяются.",
                    browser::display_name(&transfer.target))))
            .child(list)
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
        let (title, body, confirm, kind): (&str, AnyElement, &str, ButtonKind) = if self.renaming {
            let name = self.selected.as_deref().map(browser::display_name).unwrap_or_default();
            ("Переименовать", div().flex().flex_col().gap_2()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED)).child(format!("Текущее имя: {name}")))
                .child(Input::new(&self.rename_input)).into_any_element(), "Переименовать", ButtonKind::Primary)
        } else if self.creating_folder {
            ("Новая папка", div().flex().flex_col().gap_2()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(format!("В папке: {}", self.pane_path(self.active_side()).display())))
                .child(Input::new(&self.folder_input)).into_any_element(), "Создать", ButtonKind::Primary)
        } else if self.saving_workspace {
            ("Сохранить рабочее пространство", div().flex().flex_col().gap_2()
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child(format!("Вкладок: {}. Сохраняются только пути к папкам и вид окна.", self.browser.tabs.len())))
                .child(Input::new(&self.workspace_input)).into_any_element(), "Сохранить", ButtonKind::Primary)
        } else if let Some(plans) = &self.confirm_recycle {
            let what = match plans.as_slice() {
                [(path, _)] => format!("«{}»", browser::display_name(path)),
                many => format!("Элементов: {} («{}» и другие)", many.len(),
                    many.first().map(|(p, _)| browser::display_name(p)).unwrap_or_default()),
            };
            ("Удалить в Корзину?", div().flex().flex_col().gap_1().text_size(px(13.))
                .child(div().text_color(rgb(TEXT)).child(what))
                .child(div().text_size(px(12.)).text_color(rgb(TEXT_MUTED))
                    .child("Объект можно будет восстановить из Корзины Windows."))
                .into_any_element(), "В Корзину", ButtonKind::Danger)
        } else {
            return None;
        };
        let dialog = div().id("dialog").w(px(420.)).p_4().flex().flex_col().gap_3()
            .rounded_xl().bg(rgb(RAISED)).border_1().border_color(rgb(BORDER_STRONG)).shadow_lg()
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
                        if this.renaming {
                            this.commit_rename(cx);
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
        // Do not compete with SQLite search for I/O while displaying results.
        if !self.search_active {
            self.load_visible_directories(cx);
        }
        self.load_selected_details(cx);
        if self.select_first_when_loaded && self.selected.is_none() {
            let folder = self.pane_path(self.active_side());
            if let Some((listing, indices)) = self.folder_view(&folder) {
                self.select_first_when_loaded = false;
                if let Some(&first) = indices.first() {
                    self.selected = Some(listing.entries[first].path.clone());
                }
            }
        } else if self.selected.is_some() {
            self.select_first_when_loaded = false;
        }

        let split = self.browser.active().right.is_some();
        // Panels without an explicit size get the group average; give each
        // a size derived from the window so panes split the space evenly.
        let window_width = window.viewport_size().width / px(1.);
        let sidebar_width = if self.show_sidebar { 220. } else { 0. };
        let inspector_width = if self.show_inspector { 290. } else { 0. };
        let pane_count = if split && !self.search_active { 2. } else { 1. };
        let pane_width = ((window_width - sidebar_width - inspector_width) / pane_count).max(280.);
        self.pane_width = pane_width;
        let group_id = format!("main-{}-{}-{}-{}", self.show_sidebar, self.show_inspector, pane_count, self.search_active);
        let mut panels = h_resizable(SharedString::from(group_id));
        if self.show_sidebar {
            panels = panels.child(resizable_panel().size(px(sidebar_width)).size_range(px(160.)..px(360.))
                .child(self.sidebar(cx)));
        }
        if self.search_active {
            panels = panels.child(resizable_panel().size(px(pane_width)).size_range(px(320.)..px(4000.))
                .child(self.search_results(cx)));
        } else {
            panels = panels.child(resizable_panel().size(px(pane_width)).size_range(px(280.)..px(4000.))
                .child(self.pane(Side::Left, cx)));
            if split {
                panels = panels.child(resizable_panel().size(px(pane_width)).size_range(px(280.)..px(4000.))
                    .child(self.pane(Side::Right, cx)));
            }
        }
        if self.show_inspector {
            panels = panels.child(resizable_panel().size(px(inspector_width)).size_range(px(220.)..px(520.))
                .child(self.inspector(cx)));
        }

        let mut root = div().relative().size_full().flex().flex_col().bg(rgb(WINDOW))
            .text_color(rgb(TEXT)).text_size(px(13.))
            .key_context("Filemanager")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::key_back))
            .on_action(cx.listener(Self::key_forward))
            .on_action(cx.listener(Self::key_up))
            .on_action(cx.listener(Self::key_new_tab))
            .on_action(cx.listener(Self::key_close_tab))
            .on_action(cx.listener(Self::key_next_tab))
            .on_action(cx.listener(Self::key_prev_tab))
            .on_action(cx.listener(Self::key_split))
            .on_action(cx.listener(Self::key_refresh))
            .on_action(cx.listener(Self::key_stage))
            .on_action(cx.listener(Self::key_find))
            .on_action(cx.listener(Self::key_address))
            .on_action(cx.listener(Self::key_rename))
            .on_action(cx.listener(Self::key_new_folder))
            .on_action(cx.listener(Self::key_dismiss))
            .on_action(cx.listener(Self::key_select_next))
            .on_action(cx.listener(Self::key_select_prev))
            .on_action(cx.listener(Self::key_select_first))
            .on_action(cx.listener(Self::key_select_last))
            .on_action(cx.listener(Self::key_open))
            .on_action(cx.listener(Self::key_column_left))
            .on_action(cx.listener(Self::key_column_right))
            .on_action(cx.listener(Self::key_recycle))
            .on_action(cx.listener(Self::key_toggle_hidden))
            .on_action(cx.listener(Self::key_switch_pane))
            .on_action(cx.listener(Self::key_toggle_sidebar))
            .on_action(cx.listener(Self::key_toggle_inspector))
            .on_action(cx.listener(Self::key_view_list))
            .on_action(cx.listener(Self::key_view_columns))
            .on_action(cx.listener(Self::key_copy_path))
            .on_action(cx.listener(Self::key_select_all))
            .on_action(cx.listener(Self::key_clipboard_copy))
            .on_action(cx.listener(Self::key_clipboard_cut))
            .on_action(cx.listener(Self::key_clipboard_paste))
            .on_action(cx.listener(Self::key_undo))
            .on_action(cx.listener(Self::key_extend_next))
            .on_action(cx.listener(Self::key_extend_prev))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Only when the file panes have focus, never inside inputs.
                let modifiers = event.keystroke.modifiers;
                if !this.focus_handle.is_focused(window)
                    || modifiers.control || modifiers.alt || modifiers.platform {
                    return;
                }
                if let Some(text) = event.keystroke.key_char.as_deref()
                    .filter(|t| t.chars().count() == 1 && t.chars().all(|c| !c.is_control() && c != ' '))
                {
                    this.type_ahead(text, cx);
                }
            }))
            .child(self.title_bar(cx))
            .child(self.toolbar(cx))
            .child(div().flex_1().min_h_0().overflow_hidden().child(panels))
            .child(self.status_bar(cx));
        if let Some(menu) = self.context_menu.clone() {
            root = root.child(self.context_menu_view(&menu, cx));
        }
        if let Some(dialog) = self.dialog(cx) {
            root = root.child(dialog);
        }
        if let Some(dialog) = self.conflict_dialog(cx) {
            root = root.child(dialog);
        }
        root
    }
}
