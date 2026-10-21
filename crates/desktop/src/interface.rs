//! Palette/preferences overlays and selection gestures.
use crate::{
    Explorer,
    theme::{self, *},
};
use filemanager_core::interface::{CommandId, CommandSpec, Preferences, commands};
use gpui::{AnimationExt, AnyElement, Context, Entity, Focusable, Window, div, prelude::*, px};
use gpui_component::{
    input::{Input, InputState},
    theme::{Theme, ThemeMode},
};

pub(crate) struct Palette {
    pub input: Entity<InputState>,
    pub query: String,
    pub selected: usize,
    /// Keeps the chosen command visible in the height-limited list.
    pub scroll: gpui::ScrollHandle,
    /// Ctrl+P: folders to jump to instead of commands.
    pub folders: Option<crate::jump::Folders>,
}
/// Check boxes of the settings window, in the order arrows walk them.
const SETTING_FLAGS: [&str; 8] = [
    "Светлая тема",
    "Боковая панель",
    "Панель сведений",
    "Показывать скрытые файлы",
    "Значки программ у файлов, как в Проводнике",
    "Показывать только последнюю версию файла (Отчёт_v_1_6 и т. п.)",
    "Проверять обновления раз в день",
    "Класть установленные версии в папку обновлений для коллег",
];

fn flag_value(s: &Settings, index: usize) -> bool {
    match index {
        0 => s.draft.light,
        1 => s.draft.show_sidebar,
        2 => s.draft.show_inspector,
        3 => s.draft.show_hidden,
        4 => s.system_icons,
        5 => s.stack_versions,
        6 => s.update_auto,
        _ => s.update_share,
    }
}

fn toggle_flag(s: &mut Settings, index: usize) {
    let flag = match index {
        0 => &mut s.draft.light,
        1 => &mut s.draft.show_sidebar,
        2 => &mut s.draft.show_inspector,
        3 => &mut s.draft.show_hidden,
        4 => &mut s.system_icons,
        5 => &mut s.stack_versions,
        6 => &mut s.update_auto,
        _ => &mut s.update_share,
    };
    *flag = !*flag;
}

pub(crate) struct Settings {
    pub focus: gpui::FocusHandle,
    pub selected: usize,
    pub draft: Preferences,
    /// Accent schemes being chosen for the dark and the light theme.
    pub accent_dark: usize,
    pub accent_light: usize,
    pub update_auto: bool,
    pub update_channel: filemanager_core::update::Channel,
    pub update_folder: Option<std::path::PathBuf>,
    pub update_share: bool,
    pub system_icons: bool,
    pub stack_versions: bool,
    pub saving: bool,
    pub error: Option<String>,
}

impl Explorer {
    pub(crate) fn interface_modal(&self) -> bool {
        self.palette.is_some() || self.settings.is_some() || self.close_error.is_some() || self.tour.is_some() || self.comparison.is_some()
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
        self.open_palette_mode("Найти команду…", None, window, cx);
    }
    pub(crate) fn open_palette_mode(&mut self, placeholder: &'static str, folders: Option<crate::jump::Folders>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_open_interface() || self.settings.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
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
            scroll: gpui::ScrollHandle::new(),
            folders,
        });
        self.context_menu = None;
        cx.notify();
    }
    pub(crate) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.palette.as_ref().is_some_and(|palette| palette.folders.is_some()) {
            let count = self.jump_results().len();
            if let Some(palette) = self.palette.as_mut() {
                palette.selected = palette.selected.saturating_add_signed(delta).min(count.saturating_sub(1));
                palette.scroll.scroll_to_item(palette.selected);
            }
            cx.notify();
            return;
        }
        if let Some(palette) = self.palette.as_mut() {
            let results = commands(&palette.query);
            palette.selected = palette
                .selected
                .saturating_add_signed(delta)
                .min(results.len().saturating_sub(1));
            // The list also holds one header per section before its commands.
            let mut headers = 0;
            let mut group = "";
            for command in results.iter().take(palette.selected + 1) {
                if command.group != group {
                    group = command.group;
                    headers += 1;
                }
            }
            palette.scroll.scroll_to_item(palette.selected + headers);
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
            CommandId::CloseTab => (self.browser.active_group().tabs.len() > 1 || self.browser.groups.len()>1) && !self.browser.active().pinned,
            CommandId::ClosePane => self.browser.groups.len()>1,
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
        if palette.folders.is_some() {
            let selected = palette.selected;
            if let Some((path, _)) = self.jump_results().into_iter().nth(selected) {
                self.jump_to(path, false, window, cx);
            }
            return;
        }
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
            CloseTab => self.close_tab_at(self.browser.active_group().active_tab, cx),
            Split => self.toggle_split(cx),
            SplitRight => self.split_group(self.browser.active,crate::Edge::Right,None,window,cx),
            SplitDown => self.split_group(self.browser.active,crate::Edge::Bottom,None,window,cx),
            ClosePane => self.close_active_group(cx),
            ReopenTab => self.reopen_tab(cx),
            NextPane => self.cycle_pane(false,window,cx),
            PreviousPane => self.cycle_pane(true,window,cx),
            PaneLeft => self.directional_pane(crate::Edge::Left,window,cx),
            PaneRight => self.directional_pane(crate::Edge::Right,window,cx),
            PaneUp => self.directional_pane(crate::Edge::Top,window,cx),
            PaneDown => self.directional_pane(crate::Edge::Bottom,window,cx),
            NextTab => self.key_next_tab(&crate::keys::NextTab,window,cx),
            PreviousTab => self.key_prev_tab(&crate::keys::PrevTab,window,cx),
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
                self.browser.toggle_pin(self.browser.active, self.browser.active_group().active_tab);
                cx.notify();
            }
            Settings => self.open_settings(window, cx),
            Tour => self.open_tour(cx),
            Compare => self.open_comparison(cx),
            Jump => self.open_jump(window, cx),
            SetLabel(label) => { let paths = self.selection(); self.set_label(paths, label, cx); }
            FilterLabel(label) => self.set_label_filter(label, cx),
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
            accent_dark: theme::accents().0,
            accent_light: theme::accents().1,
            update_auto: self.updates.auto,
            update_channel: self.updates.channel,
            update_folder: self.updates.folder.clone(),
            update_share: self.updates.share,
            system_icons: self.icons.enabled,
            stack_versions: self.stack_versions,
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
            s.selected = s.selected.saturating_add_signed(delta).min(SETTING_FLAGS.len() - 1);
            cx.notify();
        }
    }
    pub(crate) fn settings_toggle(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut().filter(|s| !s.saving) {
            let index = s.selected;
            toggle_flag(s, index);
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
        let (accent_dark, accent_light) = (settings.accent_dark, settings.accent_light);
        let (update_auto, update_channel) = (settings.update_auto, settings.update_channel);
        let (update_folder, update_share) = (settings.update_folder.clone(), settings.update_share);
        let system_icons = settings.system_icons;
        let stack_versions = settings.stack_versions;
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_spawn(async move {
                    let store = filemanager_core::workspace::WorkspaceStore::open(
                        filemanager_core::workspace::WorkspaceStore::default_path(),
                    )?;
                    store.save_preferences(&prefs)?;
                    store.set_value(theme::ACCENT_DARK_KEY, theme::ACCENT_SCHEMES[accent_dark].key)?;
                    store.set_value(theme::ACCENT_LIGHT_KEY, theme::ACCENT_SCHEMES[accent_light].key)
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                if let Err(error) = result {
                    if let Some(settings) = this.settings.as_mut() {
                        settings.saving = false;
                        settings.error = Some(format!("Не удалось сохранить настройки: {error}"));
                    }
                } else {
                    this.set_update_preferences(update_auto, update_channel, update_folder, update_share);
                    this.set_system_icons(system_icons);
                    this.set_stack_versions(stack_versions);
                    this.show_hidden = prefs.show_hidden;
                    this.show_sidebar = prefs.show_sidebar;
                    this.show_inspector = prefs.show_inspector;
                    theme::set_light(prefs.light);
                    theme::set_accents(accent_dark, accent_light);
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
        let body = if let Some(palette) = self.palette.as_ref().filter(|palette| palette.folders.is_some()) {
            let (list, footer) = self.jump_list(palette, cx);
            div().flex().flex_col().p_0()
                .child(div().h(px(44.)).px_2().flex().items_center().border_b_1().border_color(rgb(BORDER))
                    .child(crate::jump::field(palette)))
                .child(list)
                .child(div().h(px(28.)).px_4().flex().items_center().gap_4().border_t_1().border_color(rgb(BORDER))
                    .text_size(px(11.)).text_color(rgb(TEXT_DIM))
                    .child(div().flex_1().child(footer))
                    .child("↑ ↓ выбор")
                    .child("Enter перейти")
                    .child("Ctrl+щелчок — в новой вкладке")
                    .child("Esc закрыть"))
        } else if let Some(palette) = self.palette.as_ref() {
            // Atlas: search field on top, commands in sections with icons
            // and shortcut keys, key hints at the bottom.
            let results = commands(&palette.query);
            let mut list = div().id("palette-list").max_h(px(380.)).overflow_y_scroll().py_1()
                .track_scroll(&palette.scroll);
            if results.is_empty() {
                list = list.child(div().px_4().py_3().text_color(rgb(TEXT_MUTED)).child("Команды не найдены"));
            }
            let count = results.len();
            let mut group = "";
            for (index, command) in results.into_iter().enumerate() {
                if command.group != group {
                    group = command.group;
                    list = list.child(
                        div().px_4().pt_2().pb_1().text_size(px(11.)).text_color(rgb(TEXT_DIM)).child(group),
                    );
                }
                let enabled = self.command_enabled(&command);
                let chosen = index == palette.selected;
                let fg = if chosen { SELECTION_TEXT } else { TEXT };
                list = list.child(
                    div()
                        .id(("palette-command", index))
                        .mx_1()
                        .h(px(30.))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_md()
                        .when(chosen, |row| row.bg(rgb(SELECTED)))
                        .text_color(rgb(fg))
                        .when(!enabled, |row| row.opacity(0.4))
                        .when(enabled, |row| {
                            row.cursor_pointer().when(!chosen, |row| row.hover(|s| s.bg(rgb(HOVER))))
                        })
                        .child(gpui::svg().path(command_icon(command.id)).size(px(14.)).flex_none()
                            .text_color(rgb(if chosen { SELECTION_TEXT } else { TEXT_MUTED })))
                        .child(div().flex_1().min_w_0().truncate().child(command.label))
                        .when(!command.shortcut.is_empty(), |row| row.child(
                            div().flex_none().px_1p5().rounded_sm().border_1()
                                .border_color(rgb(if chosen { SELECTION_TEXT } else { BORDER_STRONG }))
                                .text_size(px(11.))
                                .text_color(rgb(if chosen { SELECTION_TEXT } else { TEXT_MUTED }))
                                .child(command.shortcut)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.execute_command(command, window, cx)
                        })),
                );
            }
            div()
                .flex()
                .flex_col()
                .p_0()
                .child(
                    div().h(px(44.)).px_2().flex().items_center().border_b_1().border_color(rgb(BORDER))
                        .child(Input::new(&palette.input).appearance(false)
                            .prefix(gpui::svg().path("fm/search.svg").size(px(14.)).text_color(rgb(TEXT_MUTED)))),
                )
                .child(list)
                .child(
                    div().h(px(28.)).px_4().flex().items_center().gap_4().border_t_1().border_color(rgb(BORDER))
                        .text_size(px(11.)).text_color(rgb(TEXT_DIM))
                        .child(div().flex_1().child(format!("Команд: {count}")))
                        .child("↑ ↓ выбор")
                        .child("Enter выполнить")
                        .child("Esc закрыть"),
                )
        } else if let Some(settings) = self.settings.as_ref() {
            // A check box row; arrows walk them, Space toggles.
            let check = |index: usize, cx: &mut Context<Self>| {
                let on = flag_value(settings, index);
                div().id(("preference", index)).h(px(30.)).px_2().rounded_md().cursor_pointer()
                    .flex().items_center().gap_2().border_1()
                    .border_color(if index == settings.selected { rgb(ACCENT) } else { gpui::rgba(0) })
                    .hover(|row| row.bg(rgb(HOVER)))
                    .child(div().size(px(16.)).flex_none().rounded_sm().flex().items_center().justify_center()
                        .when(on, |b| b.bg(rgb(ACCENT)).child(gpui::svg().path("fm/check.svg").size(px(12.)).text_color(rgb(SELECTION_TEXT))))
                        .when(!on, |b| b.border_1().border_color(rgb(BORDER_STRONG))))
                    .child(div().min_w_0().truncate().child(SETTING_FLAGS[index]))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = this.settings.as_mut().filter(|s| !s.saving) {
                            s.selected = index;
                            toggle_flag(s, index);
                            cx.notify();
                        }
                    }))
            };
            let section = |title: &'static str| div().flex().flex_col().gap_0p5().p_1().rounded_md().bg(rgb(PANEL))
                .border_1().border_color(rgb(BORDER))
                .child(div().px_2().pt_1().pb_0p5().text_size(px(11.)).font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT_DIM)).child(title));
            let mut body = div().flex().flex_col().gap_3().text_size(px(13.))
                .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Настройки"));

            // Appearance, with the accent colours chosen separately for each theme.
            let mut look = section("ВИД").child(check(0, cx)).child(check(1, cx)).child(check(2, cx));
            for (light, label, chosen) in [(false, "Акцент тёмной темы", settings.accent_dark), (true, "Акцент светлой темы", settings.accent_light)] {
                let swatches = theme::ACCENT_SCHEMES.iter().enumerate().map(|(index, scheme)| {
                    let colour = if light { scheme.light[0] } else { scheme.dark[0] };
                    div().id((if light { "accent-light" } else { "accent-dark" }, index))
                        .size(px(20.)).flex_none().rounded_full().cursor_pointer()
                        .bg(gpui::rgb(colour))
                        .when(index == chosen, |dot| dot.border_2().border_color(rgb(TEXT)))
                        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(scheme.label).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.settings.as_mut().filter(|s| !s.saving) {
                                if light { s.accent_light = index; } else { s.accent_dark = index; }
                                cx.notify();
                            }
                        }))
                });
                look = look.child(div().h(px(30.)).px_2().flex().items_center().gap_2()
                    .child(div().flex_1().child(label))
                    .child(div().flex().gap_1p5().children(swatches)));
            }
            body = body.child(look)
                .child(section("ФАЙЛЫ").child(check(3, cx)).child(check(4, cx)).child(check(5, cx)));

            // Updates: the channel, and the shared folder for networks where
            // only the browser may reach the internet.
            {
                use filemanager_core::update::Channel;
                let channel = settings.update_channel;
                let chip = |id: &'static str, label: &'static str, chosen: bool| div().id(id).h(px(24.)).px_2().flex().flex_none()
                    .items_center().rounded_md().cursor_pointer().text_size(px(12.)).border_1()
                    .when(chosen, |chip| chip.bg(rgb(ACCENT_SOFT)).border_color(rgb(ACCENT)).text_color(rgb(TEXT)))
                    .when(!chosen, |chip| chip.border_color(rgb(BORDER)).text_color(rgb(TEXT_MUTED))
                        .hover(|s| s.bg(rgb(HOVER)).text_color(rgb(TEXT))))
                    .child(label);
                let folder_text = settings.update_folder.as_ref()
                    .map_or("не задана".to_string(), |folder| folder.display().to_string());
                body = body.child(section("ОБНОВЛЕНИЯ")
                    .child(check(6, cx))
                    .child(div().h(px(30.)).px_2().flex().items_center().gap_2()
                        .child(div().flex_1().child("Канал"))
                        .children([Channel::Stable, Channel::Beta].into_iter().map(|option| {
                            chip(option.key(), option.label(), option == channel)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(s) = this.settings.as_mut().filter(|s| !s.saving) { s.update_channel = option; cx.notify(); }
                                }))
                        })))
                    .child(div().h(px(30.)).px_2().flex().items_center().gap_2()
                        .child(div().flex_none().child("Папка обновлений"))
                        .child(div().flex_1().min_w_0().truncate().text_color(rgb(TEXT_MUTED)).child(folder_text))
                        .child(chip("update-folder-here", "Текущая папка", false)
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Взять папку, открытую в активной панели").build(window, cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let folder = this.pane_path(this.active_side());
                                if let Some(s) = this.settings.as_mut().filter(|s| !s.saving) { s.update_folder = Some(folder); cx.notify(); }
                            })))
                        .when(settings.update_folder.is_some(), |row| row.child(chip("update-folder-clear", "Убрать", false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(s) = this.settings.as_mut().filter(|s| !s.saving) { s.update_folder = None; cx.notify(); }
                            })))))
                    .child(check(7, cx))
                    .child(div().px_2().pb_1().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                        .child("Если программе закрыт интернет: обновления берутся из этой папки или из файла Papka.exe, скачанного браузером в «Загрузки».")));
            }
            if let Some(error) = settings.error.as_ref() {
                body = body.child(div().text_size(px(12.)).text_color(rgb(DANGER)).child(error.clone()));
            }
            body.child(
                div().flex().items_center().gap_2()
                    .child(div().flex_1().text_size(px(11.)).text_color(rgb(TEXT_DIM))
                        .child("↑ ↓ выбор · Пробел переключить · Enter сохранить"))
                    .child(crate::view::text_button("cancel-settings", "Отмена", crate::view::ButtonKind::Ghost)
                        .on_click(cx.listener(|this, _, window, cx| { this.dismiss_interface(window, cx); })))
                    .child(crate::view::text_button("apply-settings", if settings.saving { "Сохранение…" } else { "Сохранить" },
                        crate::view::ButtonKind::Primary)
                        .on_click(cx.listener(|this, _, _, cx| this.apply_settings(cx)))),
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
                    body.w(px(if self.palette.is_some() { 560. } else { 520. }))
                        .max_w_full()
                        .when(self.palette.is_none(), |body| body.p_4())
                        .overflow_hidden()
                        .rounded_lg()
                        .bg(rgb(RAISED))
                        .border_1()
                        .border_color(rgb(BORDER_STRONG))
                        // Atlas fades and lifts its popups in about 120 ms;
                        // navigation itself stays instant.
                        .with_animation(
                            if self.palette.is_some() { "palette-appear" } else { "settings-appear" },
                            gpui::Animation::new(std::time::Duration::from_millis(120))
                                .with_easing(gpui::ease_out_quint()),
                            |body, delta| body.opacity(delta).mt(px(-6. * (1. - delta))),
                        ),
                )
                .into_any_element(),
        )
    }
}

pub(crate) struct Marquee {
    folder: std::path::PathBuf,
    pub(crate) side: crate::GroupId,
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
        side: crate::GroupId,
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
        let Some(handle) = self.scroll_handles.get(&(side,folder.to_path_buf())) else {
            return;
        };
        let state = handle.0.borrow();
        let bounds = state.base_handle.bounds();
        let scroll = (-state.base_handle.offset().y / px(1.)).max(0.);
        drop(state);
        if !bounds.contains(&event.position) {
            return;
        }
        let Some((listing, indices)) = self.folder_view(side, folder) else {
            return;
        };
        let local_y = (event.position.y - bounds.origin.y) / px(1.);
        let row = crate::layout::row_index(local_y, scroll);
        // Reserve the right gutter; rows retain their normal file drag gesture.
        if row < indices.len() && event.position.x < bounds.right() - px(12.) {
            return;
        }
        let switched = self.active_side() != side;
        self.browser.focus(side);
        if switched {
            // The other pane's selection must not stay active here, even
            // when the gesture ends without moving.
            self.selected = None;
            self.marked.clear();
            self.anchor = None;
        }
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
            tab: self.browser.active_group().active_tab,
            listing,
            indices,
            bounds,
            scroll,
            sort: self.sort_for(side),
            hidden: self.show_hidden,
            miller: self.miller(side),
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
        let view = self.folder_view(self.browser.active, &folder);
        let Some(m) = self.marquee.as_ref() else {
            return false;
        };
        let same_location = self.browser.active_group().active_tab == m.tab
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
            && self.sort_for(m.side) == m.sort
            && self.show_hidden == m.hidden
            && self.miller(m.side) == m.miller
            && self.filter == m.filter
            && self
                .directory_cache
                .get(&m.folder)
                .is_some_and(|l| std::sync::Arc::ptr_eq(l, &m.listing))
            && self.scroll_handles.get(&(m.side,m.folder.clone())).is_some_and(|h| {
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
            crate::layout::ROW_HEIGHT,
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
                crate::layout::ROW_HEIGHT,
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
                .bg(crate::theme::accent_alpha(0x22))
                .border_1()
                .border_color(rgb(ACCENT))
                .into_any_element(),
        )
    }
}

/// Icon of a palette command, from the same set as the toolbar.
fn command_icon(id: CommandId) -> &'static str {
    use CommandId::*;
    match id {
        NewTab => "fm/plus.svg",
        CloseTab => "fm/x.svg",
        PinTab => "fm/layers.svg",
        Split | SplitRight | SplitDown | ClosePane | NextPane | PreviousPane | PaneLeft | PaneRight | PaneUp | PaneDown => "fm/split.svg",
        ReopenTab => "fm/undo.svg",
        NextTab | PreviousTab => "fm/layers.svg",
        Back => "fm/arrow-left.svg",
        Forward => "fm/arrow-right.svg",
        Up => "fm/arrow-up.svg",
        Address => "fm/external.svg",
        Find => "fm/search.svg",
        Refresh => "fm/refresh.svg",
        NewFolder => "fm/folder-plus.svg",
        Rename => "fm/pencil.svg",
        ToggleHidden => "fm/eye.svg",
        Sidebar => "fm/panel-left.svg",
        Inspector => "fm/panel-right.svg",
        List => "fm/list.svg",
        Columns => "fm/columns.svg",
        Settings => "fm/settings.svg",
        Tour => "fm/help.svg",
        Compare => "fm/split.svg",
        Jump => "fm/folder-fill.svg",
        SetLabel(_) => "fm/tag.svg",
        FilterLabel(_) => "fm/eye.svg",
    }
}
