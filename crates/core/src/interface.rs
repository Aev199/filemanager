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
            show_inspector: true,
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
    [
        (Back, "Назад", "Alt+←"),
        (Forward, "Вперёд", "Alt+→"),
        (Up, "Вверх", "Alt+↑"),
        (NewTab, "Новая вкладка", "Ctrl+T"),
        (CloseTab, "Закрыть вкладку", "Ctrl+W"),
        (Split, "Разделить окно", "Ctrl+\\"),
        (Refresh, "Обновить папку", "F5"),
        (Find, "Поиск файлов", "Ctrl+F"),
        (Address, "Перейти по пути", "Ctrl+L"),
        (Rename, "Переименовать выбранное", "F2"),
        (NewFolder, "Новая папка", "Ctrl+Shift+N"),
        (ToggleHidden, "Показать скрытые файлы", "Ctrl+H"),
        (Sidebar, "Боковая панель", "Ctrl+B"),
        (Inspector, "Панель сведений", "Ctrl+I"),
        (List, "Вид список", "Ctrl+1"),
        (Columns, "Вид колонки", "Ctrl+2"),
        (PinTab, "Закрепить или открепить вкладку", ""),
        (Settings, "Настройки", "Ctrl+,"),
    ]
    .into_iter()
    .map(|(id, label, shortcut)| CommandSpec {
        id,
        label,
        shortcut,
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
