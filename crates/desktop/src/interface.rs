//! Palette/preferences overlays and selection gestures.
use crate::{
    Explorer,
    theme::{self, *},
};
use filemanager_core::interface::{CommandId, CommandSpec, Preferences, commands};
use gpui::{AnyElement, Context, Entity, Focusable, Window, div, prelude::*, px};
use gpui_component::{
    input::{Input, InputState},
    theme::{Theme, ThemeMode},
};

pub(crate) struct Palette {
    pub input: Entity<InputState>,
    pub query: String,
    pub selected: usize,
}
pub(crate) struct Settings {
    pub focus: gpui::FocusHandle,
    pub selected: usize,
    pub draft: Preferences,
    pub saving: bool,
    pub error: Option<String>,
}

impl Explorer {
    pub(crate) fn interface_modal(&self) -> bool {
        self.palette.is_some() || self.settings.is_some()
    }
    fn can_open_interface(&self) -> bool {
        !self.renaming
            && !self.creating_folder
            && !self.saving_workspace
            && self.confirm_recycle.is_none()
            && self.pending_transfer.is_none()
            && self.bulk_rename.is_none()
            && self.bulk_rename_errors.is_empty()
    }
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_open_interface() || self.settings.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Найти команду…"));
        cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                gpui_component::input::InputEvent::Change => {
                    if let Some(palette) = this.palette.as_mut() {
                        palette.query = input.read(cx).value().to_string();
                        palette.selected = 0;
                    }
                    cx.notify();
                }
                gpui_component::input::InputEvent::PressEnter { .. } => {
                    this.palette_enter(window, cx)
                }
                _ => {}
            },
        )
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.palette = Some(Palette {
            input,
            query: String::new(),
            selected: 0,
        });
        self.context_menu = None;
        cx.notify();
    }
    pub(crate) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.as_mut() {
            let len = commands(&palette.query).len();
            palette.selected = palette
                .selected
                .saturating_add_signed(delta)
                .min(len.saturating_sub(1));
            cx.notify();
        }
    }
    fn command_enabled(&self, command: &CommandSpec) -> bool {
        if !command.enabled(
            self.selected.is_some() || !self.marked.is_empty(),
            self.operation_busy,
        ) {
            return false;
        }
        match command.id {
            CommandId::CloseTab => self.browser.tabs.len() > 1 && !self.browser.active().pinned,
            CommandId::Back => self.browser.active().active().can_go_back(),
            CommandId::Forward => self.browser.active().active().can_go_forward(),
            CommandId::Up => self.browser.active().active().path.parent().is_some(),
            _ => true,
        }
    }
    pub(crate) fn palette_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        if let Some(command) = commands(&palette.query).get(palette.selected) {
            self.execute_command(*command, window, cx);
        }
    }
    fn execute_command(
        &mut self,
        command: CommandSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.command_enabled(&command) {
            return;
        }
        self.palette = None;
        window.focus(&self.focus_handle, cx);
        use CommandId::*;
        match command.id {
            Back => self.go_back(false, cx),
            Forward => self.go_back(true, cx),
            Up => self.go_up(cx),
            NewTab => self.add_tab(cx),
            CloseTab => self.close_tab_at(self.browser.active_tab, cx),
            Split => self.toggle_split(cx),
            Refresh => self.refresh_visible_directories(cx),
            Find => self.key_find(&crate::keys::Find, window, cx),
            Address => self.key_address(&crate::keys::AddressBar, window, cx),
            Rename => self.begin_rename(window, cx),
            NewFolder => self.begin_new_folder(window, cx),
            ToggleHidden => self.key_toggle_hidden(&crate::keys::ToggleHidden, window, cx),
            Sidebar => self.key_toggle_sidebar(&crate::keys::ToggleSidebar, window, cx),
            Inspector => self.key_toggle_inspector(&crate::keys::ToggleInspector, window, cx),
            List => self.set_miller(false, cx),
            Columns => self.set_miller(true, cx),
            PinTab => {
                self.browser.toggle_pin(self.browser.active_tab);
                cx.notify();
            }
            Settings => self.open_settings(window, cx),
        }
    }
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_open_interface() || self.palette.is_some() || self.settings.is_some() {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.settings = Some(Settings {
            focus,
            selected: 0,
            draft: Preferences {
                light: theme::is_light(),
                show_hidden: self.show_hidden,
                show_sidebar: self.show_sidebar,
                show_inspector: self.show_inspector,
            },
            saving: false,
            error: None,
        });
        self.context_menu = None;
        cx.notify();
    }
    pub(crate) fn dismiss_interface(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.interface_modal() {
            return false;
        }
        if self.settings.as_ref().is_some_and(|s| s.saving) {
            return true;
        }
        self.palette = None;
        self.settings = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
        true
    }
    pub(crate) fn settings_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut().filter(|s| !s.saving) {
            s.selected = s.selected.saturating_add_signed(delta).min(3);
            cx.notify();
        }
    }
    pub(crate) fn settings_toggle(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut().filter(|s| !s.saving) {
            let flag = match s.selected {
                0 => &mut s.draft.light,
                1 => &mut s.draft.show_hidden,
                2 => &mut s.draft.show_sidebar,
                _ => &mut s.draft.show_inspector,
            };
            *flag = !*flag;
            cx.notify();
        }
    }
    pub(crate) fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        if settings.saving {
            return;
        }
        settings.saving = true;
        settings.error = None;
        let prefs = settings.draft;
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_spawn(async move {
                    filemanager_core::workspace::WorkspaceStore::open(
                        filemanager_core::workspace::WorkspaceStore::default_path(),
                    )?
                    .save_preferences(&prefs)
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if let Err(error) = result {
                    if let Some(settings) = this.settings.as_mut() {
                        settings.saving = false;
                        settings.error = Some(format!("Не удалось сохранить настройки: {error}"));
                    }
                } else {
                    this.show_hidden = prefs.show_hidden;
                    this.show_sidebar = prefs.show_sidebar;
                    this.show_inspector = prefs.show_inspector;
                    theme::set_light(prefs.light);
                    Theme::change(
                        if prefs.light {
                            ThemeMode::Light
                        } else {
                            ThemeMode::Dark
                        },
                        None,
                        cx,
                    );
                    crate::view::apply_component_theme(cx);
                    this.settings = None;
                    this.interface_focus_pending = true;
                    this.status = "Настройки сохранены".into();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn interface_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let body = if let Some(palette) = self.palette.as_ref() {
            let results = commands(&palette.query);
            let mut body = div()
                .flex()
                .flex_col()
                .gap_1()
                .child("Команды · Ctrl+K")
                .child(Input::new(&palette.input))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(TEXT_MUTED))
                        .child("↑ ↓ — выбор · Enter — выполнить · Esc — закрыть"),
                );
            if results.is_empty() {
                body = body.child(
                    div()
                        .p_3()
                        .text_color(rgb(TEXT_MUTED))
                        .child("Команды не найдены"),
                );
            }
            for (index, command) in results.into_iter().enumerate() {
                let enabled = self.command_enabled(&command);
                body = body.child(
                    div()
                        .id(("palette-command", index))
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .when(index == palette.selected, |row| row.bg(rgb(SELECTED)))
                        .when(!enabled, |row| row.opacity(0.4))
                        .when(enabled, |row| {
                            row.cursor_pointer().hover(|s| s.bg(rgb(HOVER)))
                        })
                        .child(div().flex_1().child(command.label))
                        .child(div().text_color(rgb(TEXT_MUTED)).child(command.shortcut))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.execute_command(command, window, cx)
                        })),
                );
            }
            body
        } else if let Some(settings) = self.settings.as_ref() {
            let mut body = div().flex().flex_col().gap_3().child("Настройки").child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(TEXT_MUTED))
                    .child("↑ ↓ — выбор · Пробел — переключить · Enter — сохранить · Esc — отмена"),
            );
            for (index, label, value) in [
                (0, "Светлая тема", settings.draft.light),
                (1, "Показывать скрытые файлы", settings.draft.show_hidden),
                (2, "Боковая панель", settings.draft.show_sidebar),
                (3, "Панель сведений", settings.draft.show_inspector),
            ] {
                body = body.child(
                    div()
                        .id(("preference", index as usize))
                        .when(index as usize == settings.selected, |row| {
                            row.border_1().border_color(rgb(ACCENT))
                        })
                        .p_2()
                        .rounded_md()
                        .bg(rgb(SURFACE))
                        .cursor_pointer()
                        .child(format!("{}  {label}", if value { "☑" } else { "☐" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.settings.as_mut().filter(|s| !s.saving) {
                                let flag = match index {
                                    0 => &mut s.draft.light,
                                    1 => &mut s.draft.show_hidden,
                                    2 => &mut s.draft.show_sidebar,
                                    _ => &mut s.draft.show_inspector,
                                };
                                *flag = !*flag;
                                cx.notify();
                            }
                        })),
                );
            }
            if let Some(error) = settings.error.as_ref() {
                body = body.child(div().text_color(rgb(DANGER)).child(error.clone()));
            }
            body.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("apply-settings")
                            .p_2()
                            .rounded_md()
                            .bg(rgb(ACCENT_SOFT))
                            .cursor_pointer()
                            .child(if settings.saving {
                                "Сохранение…"
                            } else {
                                "Сохранить"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.apply_settings(cx))),
                    )
                    .child(
                        div()
                            .id("cancel-settings")
                            .p_2()
                            .cursor_pointer()
                            .child("Отмена")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_interface(window, cx);
                            })),
                    ),
            )
        } else {
            return None;
        };
        Some(
            div()
                .id("interface-overlay")
                .key_context(if self.palette.is_some() {
                    "CommandPalette"
                } else {
                    "SettingsOverlay"
                })
                .when_some(self.settings.as_ref(), |overlay, s| {
                    overlay.track_focus(&s.focus)
                })
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(64.))
                .bg(gpui::rgba(0x00000088))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .child(
                    body.w(px(520.))
                        .max_w_full()
                        .p_4()
                        .rounded_lg()
                        .bg(rgb(RAISED))
                        .border_1()
                        .border_color(rgb(BORDER_STRONG)),
                )
                .into_any_element(),
        )
    }
}

pub(crate) struct Marquee {
    folder: std::path::PathBuf,
    side: crate::Side,
    tab: usize,
    listing: std::sync::Arc<filemanager_core::browser::Listing>,
    indices: std::sync::Arc<Vec<usize>>,
    bounds: gpui::Bounds<gpui::Pixels>,
    scroll: f32,
    sort: filemanager_core::sort::SortSpec,
    hidden: bool,
    miller: bool,
    filter: String,
    start: gpui::Point<gpui::Pixels>,
    current: gpui::Point<gpui::Pixels>,
    selected: Option<std::path::PathBuf>,
    marked: Vec<std::path::PathBuf>,
    anchor: Option<std::path::PathBuf>,
    additive: bool,
    moved: bool,
}
impl Explorer {
    pub(crate) fn begin_marquee(
        &mut self,
        folder: &std::path::Path,
        side: crate::Side,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.interface_modal()
            || !self.can_open_interface()
            || self.search_active
            || self.renaming
            || self.pane_path(side) != folder
        {
            return;
        }
        // First focus the other pane normally; its effective filter must settle
        // before a gesture captures indices. This start must not cross views.
        if self.active_side() != side && !self.filter.is_empty() {
            return;
        }
        let Some(handle) = self.scroll_handles.get(folder) else {
            return;
        };
        let state = handle.0.borrow();
        let bounds = state.base_handle.bounds();
        let scroll = (-state.base_handle.offset().y / px(1.)).max(0.);
        drop(state);
        if !bounds.contains(&event.position) {
            return;
        }
        let Some((listing, indices)) = self.folder_view(folder) else {
            return;
        };
        let local_y = (event.position.y - bounds.origin.y) / px(1.);
        let row = ((local_y + scroll) / 28.).floor() as usize;
        // Reserve the right gutter; rows retain their normal file drag gesture.
        if row < indices.len() && event.position.x < bounds.right() - px(12.) {
            return;
        }
        let switched = self.active_side() != side;
        self.browser.active_mut().focus_right = side == crate::Side::Right;
        let selected = if switched {
            None
        } else {
            self.selected.clone()
        };
        let marked = if switched {
            Vec::new()
        } else {
            self.marked.clone()
        };
        let anchor = if switched { None } else { self.anchor.clone() };
        self.marquee = Some(Marquee {
            folder: folder.to_path_buf(),
            side,
            tab: self.browser.active_tab,
            listing,
            indices,
            bounds,
            scroll,
            sort: self.sort,
            hidden: self.show_hidden,
            miller: self.miller_mode,
            filter: self.filter.clone(),
            start: event.position,
            current: event.position,
            selected,
            marked,
            anchor,
            additive: event.modifiers.control,
            moved: false,
        });
        self.context_menu = None;
        window.focus(&self.focus_handle, cx);
        cx.stop_propagation();
        cx.notify();
    }
    pub(crate) fn validate_marquee(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(folder) = self.marquee.as_ref().map(|m| m.folder.clone()) else {
            return false;
        };
        let view = self.folder_view(&folder);
        let Some(m) = self.marquee.as_ref() else {
            return false;
        };
        let same_location = self.browser.active_tab == m.tab
            && self.active_side() == m.side
            && self.pane_path(m.side) == m.folder;
        let same_view = view.is_some_and(|(listing, indices)| {
            std::sync::Arc::ptr_eq(&listing, &m.listing)
                && std::sync::Arc::ptr_eq(&indices, &m.indices)
        });
        let valid = same_location
            && same_view
            && !self.interface_modal()
            && !self.search_active
            && self.sort == m.sort
            && self.show_hidden == m.hidden
            && self.miller_mode == m.miller
            && self.filter == m.filter
            && self
                .directory_cache
                .get(&m.folder)
                .is_some_and(|l| std::sync::Arc::ptr_eq(l, &m.listing))
            && self.scroll_handles.get(&m.folder).is_some_and(|h| {
                let s = h.0.borrow();
                s.base_handle.bounds() == m.bounds
                    && (-s.base_handle.offset().y / px(1.)).max(0.) == m.scroll
            });
        if !valid {
            self.marquee = None;
            if same_location {
                self.selected = None;
                self.marked.clear();
                self.anchor = None;
            }
            cx.notify();
        }
        valid
    }
    pub(crate) fn move_marquee(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.validate_marquee(cx) {
            return;
        }
        if !event.dragging() {
            self.marquee = None;
            cx.notify();
            return;
        }
        let m = self.marquee.as_mut().unwrap();
        m.current = gpui::point(
            event.position.x.clamp(m.bounds.left(), m.bounds.right()),
            event.position.y.clamp(m.bounds.top(), m.bounds.bottom()),
        );
        if !m.moved
            && (m.current.x - m.start.x).abs() < px(3.)
            && (m.current.y - m.start.y).abs() < px(3.)
        {
            return;
        }
        m.moved = true;
        let rows = filemanager_core::selection::marquee_rows(
            m.indices.len(),
            28.,
            (m.start.y - m.bounds.top()) / px(1.),
            (m.current.y - m.bounds.top()) / px(1.),
            m.bounds.size.height / px(1.),
            m.scroll,
        );
        let mut chosen: std::collections::HashSet<std::path::PathBuf> = if m.additive {
            if m.marked.is_empty() {
                m.selected.iter().cloned().collect()
            } else {
                m.marked.iter().cloned().collect()
            }
        } else {
            std::collections::HashSet::new()
        };
        if m.current.x != m.start.x {
            for row in rows {
                chosen.insert(m.listing.entries[m.indices[row]].path.clone());
            }
        }
        self.marked = if m.additive {
            m.indices
                .iter()
                .map(|&i| &m.listing.entries[i].path)
                .filter(|p| chosen.contains(*p))
                .cloned()
                .collect()
        } else {
            let rows = filemanager_core::selection::marquee_rows(
                m.indices.len(),
                28.,
                (m.start.y - m.bounds.top()) / px(1.),
                (m.current.y - m.bounds.top()) / px(1.),
                m.bounds.size.height / px(1.),
                m.scroll,
            );
            if m.current.x == m.start.x {
                Vec::new()
            } else {
                rows.map(|i| m.listing.entries[m.indices[i]].path.clone())
                    .collect()
            }
        };
        self.selected = self.marked.first().cloned();
        self.anchor = self.selected.clone();
        cx.stop_propagation();
        cx.notify();
    }
    pub(crate) fn finish_marquee(&mut self, cx: &mut Context<Self>) {
        if !self.validate_marquee(cx) {
            return;
        }
        let m = self.marquee.take().unwrap();
        if !m.moved && !m.additive {
            self.selected = None;
            self.marked.clear();
            self.anchor = None;
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(crate) fn escape_marquee(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.validate_marquee(cx) {
            return false;
        }
        let m = self.marquee.take().unwrap();
        self.selected = m.selected;
        self.marked = m.marked;
        self.anchor = m.anchor;
        cx.notify();
        true
    }
    pub(crate) fn marquee_rectangle(&self) -> Option<AnyElement> {
        let m = self.marquee.as_ref().filter(|m| m.moved)?;
        Some(
            div()
                .absolute()
                .left(m.start.x.min(m.current.x))
                .top(m.start.y.min(m.current.y))
                .w((m.start.x - m.current.x).abs())
                .h((m.start.y - m.current.y).abs())
                .bg(gpui::rgba(0x5E8BFF22))
                .border_1()
                .border_color(rgb(ACCENT))
                .into_any_element(),
        )
    }
}
