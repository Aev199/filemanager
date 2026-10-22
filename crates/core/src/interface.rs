//! Pure interface policy; no GUI or filesystem actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub light: bool,
    pub show_hidden: bool,
    pub show_sidebar: bool,
    pub show_inspector: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            light: false,
            show_hidden: false,
            show_sidebar: true,
            show_inspector: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandId {
    Back,
    Forward,
    Up,
    NewTab,
    CloseTab,
    Split,
    SplitRight, SplitDown, ClosePane, ReopenTab,
    NextPane, PreviousPane, PaneLeft, PaneRight, PaneUp, PaneDown, NextTab, PreviousTab,
    Refresh,
    Find,
    Address,
    Rename,
    NewFolder,
    ToggleHidden,
    Sidebar,
    Inspector,
    List,
    Columns,
    PinTab,
    Settings,
    Tour,
    Compare,
    /// Ctrl+P: quick jump to a folder.
    Jump,
    /// Sets (None: removes) the label of the selection.
    SetLabel(Option<crate::labels::Label>),
    /// Shows only items with the label in the active pane (None: all).
    FilterLabel(Option<crate::labels::Label>),
}
#[derive(Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    pub label: &'static str,
    pub shortcut: &'static str,
    /// Section of the palette, in display order, as in Atlas.
    pub group: &'static str,
}
impl CommandSpec {
    pub fn enabled(&self, has_selection: bool, busy: bool) -> bool {
        match self.id {
            CommandId::Rename => has_selection && !busy,
            CommandId::NewFolder => !busy,
            CommandId::SetLabel(_) => has_selection,
            _ => true,
        }
    }
}
pub fn commands(query: &str) -> Vec<CommandSpec> {
    use CommandId::*;
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    const TABS: &str = "Вкладки и панели";
    const NAVIGATION: &str = "Навигация";
    const FILES: &str = "Файлы";
    const VIEW: &str = "Вид";
    const LABELS: &str = "Метки";
    use crate::labels::Label;
    const SET: [(&str, &str); 5] = [
        ("Метка «В работе»", "Alt+1"), ("Метка «На согласовании»", "Alt+2"), ("Метка «Выдано»", "Alt+3"),
        ("Метка «Устарело»", "Alt+4"), ("Метка «Важно»", "Alt+5"),
    ];
    const FILTER: [&str; 5] = [
        "Показать только «В работе»", "Показать только «На согласовании»", "Показать только «Выдано»",
        "Показать только «Устарело»", "Показать только «Важно»",
    ];
    let labels = Label::ALL.into_iter().zip(SET).map(|(label, (name, keys))| (SetLabel(Some(label)), name, keys, LABELS))
        .chain([(SetLabel(None), "Снять метку", "Alt+0", LABELS)])
        .chain(Label::ALL.into_iter().zip(FILTER).map(|(label, name)| (FilterLabel(Some(label)), name, "", LABELS)))
        .chain([(FilterLabel(None), "Показать все файлы, без отбора по метке", "", LABELS)]);
    [
        (NewTab, "Новая вкладка", "Ctrl+T", TABS),
        (CloseTab, "Закрыть вкладку", "Ctrl+W", TABS),
        (PinTab, "Закрепить или открепить вкладку", "", TABS),
        (Split, "Разделить окно", "Ctrl+\\", TABS),
        (SplitRight, "Разделить вправо", "Ctrl+Shift+S", TABS),
        (SplitDown, "Разделить вниз", "Ctrl+Alt+S", TABS),
        (ClosePane, "Закрыть панель", "Ctrl+Shift+W", TABS),
        (ReopenTab, "Открыть закрытую вкладку", "Ctrl+Shift+T", TABS),
        (NextPane, "Следующая панель", "Tab", TABS),
        (PreviousPane, "Предыдущая панель", "Shift+Tab", TABS),
        (PaneLeft, "Панель слева", "Alt+Shift+←", TABS),
        (PaneRight, "Панель справа", "Alt+Shift+→", TABS),
        (PaneUp, "Панель сверху", "Alt+Shift+↑", TABS),
        (PaneDown, "Панель снизу", "Alt+Shift+↓", TABS),
        (NextTab, "Следующая вкладка", "Ctrl+Tab", TABS),
        (PreviousTab, "Предыдущая вкладка", "Ctrl+Shift+Tab", TABS),
        (Back, "Назад", "Alt+←", NAVIGATION),
        (Forward, "Вперёд", "Alt+→", NAVIGATION),
        (Up, "Вверх", "Alt+↑", NAVIGATION),
        (Jump, "Быстрый переход к папке", "Ctrl+P", NAVIGATION),
        (Address, "Перейти по пути", "Ctrl+L", NAVIGATION),
        (Find, "Поиск файлов", "Ctrl+F", NAVIGATION),
        (Refresh, "Обновить папку", "F5", NAVIGATION),
        (NewFolder, "Новая папка", "Ctrl+Shift+N", FILES),
        (Rename, "Переименовать выбранное", "F2", FILES),
        (Compare, "Сравнить с соседней панелью", "", FILES),
        (ToggleHidden, "Показать скрытые файлы", "Ctrl+.", VIEW),
        (Sidebar, "Боковая панель", "Ctrl+B", VIEW),
        (Inspector, "Панель сведений", "Ctrl+I", VIEW),
        (List, "Вид список", "Ctrl+1", VIEW),
        (Columns, "Вид колонки", "Ctrl+2", VIEW),
        (Settings, "Настройки", "Ctrl+,", VIEW),
        (Tour, "Экскурс по возможностям", "", VIEW),
    ]
    .into_iter()
    .chain(labels)
    .map(|(id, label, shortcut, group)| CommandSpec {
        id,
        label,
        shortcut,
        group,
    })
    .filter(|cmd| {
        let label = cmd.label.to_lowercase();
        // A word typed in the wrong keyboard layout ("dbl" for "вид") also matches.
        words.iter().all(|w| label.contains(w.as_str()) || label.contains(&to_russian_layout(w)))
    })
    .collect()
}

/// Re-reads keys typed in the English QWERTY layout as the Russian ЙЦУКЕН
/// keys at the same positions.
pub fn to_russian_layout(text: &str) -> String {
    const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";
    text.chars()
        .map(|c| EN.chars().position(|e| e == c).and_then(|i| RU.chars().nth(i)).unwrap_or(c))
        .collect()
}

/// Whether Windows plays interface animations («Эффекты анимации» in
/// Accessibility settings); elsewhere always.
pub fn system_animations() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
        let mut on: i32 = 1;
        let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) };
        read == 0 || on != 0
    }
    #[cfg(not(windows))]
    {
        true
    }
}
