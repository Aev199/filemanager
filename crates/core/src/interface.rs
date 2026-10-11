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
    [
        (NewTab, "Новая вкладка", "Ctrl+T", TABS),
        (CloseTab, "Закрыть вкладку", "Ctrl+W", TABS),
        (PinTab, "Закрепить или открепить вкладку", "", TABS),
        (Split, "Разделить окно", "Ctrl+\\", TABS),
        (Back, "Назад", "Alt+←", NAVIGATION),
        (Forward, "Вперёд", "Alt+→", NAVIGATION),
        (Up, "Вверх", "Alt+↑", NAVIGATION),
        (Address, "Перейти по пути", "Ctrl+L", NAVIGATION),
        (Find, "Поиск файлов", "Ctrl+F", NAVIGATION),
        (Refresh, "Обновить папку", "F5", NAVIGATION),
        (NewFolder, "Новая папка", "Ctrl+Shift+N", FILES),
        (Rename, "Переименовать выбранное", "F2", FILES),
        (ToggleHidden, "Показать скрытые файлы", "Ctrl+.", VIEW),
        (Sidebar, "Боковая панель", "Ctrl+B", VIEW),
        (Inspector, "Панель сведений", "Ctrl+I", VIEW),
        (List, "Вид список", "Ctrl+1", VIEW),
        (Columns, "Вид колонки", "Ctrl+2", VIEW),
        (Settings, "Настройки", "Ctrl+,", VIEW),
    ]
    .into_iter()
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
