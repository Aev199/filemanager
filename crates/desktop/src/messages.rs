//! Russian wording for errors that come from the core (which keeps English
//! for the CLI and logs). Unknown text is shown unchanged.

const PHRASES: &[(&str, &str)] = &[
    ("Destination already exists", "Объект с таким именем уже существует, замена запрещена"),
    ("No-overwrite moves are implemented for Windows only", "Безопасный перенос без перезаписи работает только в Windows"),
    ("Cannot move or copy a folder inside itself", "Нельзя переместить или скопировать папку внутрь самой себя"),
    ("Source was changed/replaced since preparation; operation refused", "Источник изменился после подготовки, операция отменена"),
    ("Source changed between preflight and opening its handle; copy refused", "Источник изменился перед копированием, операция отменена"),
    ("Source changed while copying; destination was not published", "Источник изменился во время копирования, копия не создана"),
    ("Source length changed while copying; destination was not published", "Размер источника изменился во время копирования, копия не создана"),
    ("Source changed during folder copy", "Папка изменилась во время копирования"),
    ("Copy cancelled", "Копирование отменено"),
    ("Symbolic links are not supported in folder operations", "Символические ссылки в папках не поддерживаются"),
    ("Symbolic links must be handled separately", "Символические ссылки не поддерживаются"),
    ("Windows junctions and reparse points are not supported", "Точки соединения Windows не поддерживаются"),
    ("Unsupported file type in folder operation", "Неподдерживаемый тип файла в папке"),
    ("Unsupported source type", "Неподдерживаемый тип источника"),
    ("Source is not a regular file", "Источник не является обычным файлом"),
    ("Cannot operate on a filesystem root", "Нельзя выполнять операции с корнем диска"),
    ("Cannot stage a filesystem root", "Корень диска нельзя добавить в Drop Zone"),
    ("Folder has more than 100,000 items; narrow the operation", "В папке больше 100 000 элементов, выберите меньшую часть"),
    ("Another Filemanager window is modifying files; retry after it finishes", "Другое окно Filemanager изменяет файлы, повторите после завершения"),
    ("No filesystem changes while another executor is active", "Другое окно выполняет операции, изменения отложены"),
    ("Cannot lock operation journal; filesystem action refused", "Журнал операций занят, действие отменено"),
    ("Destination changed or was replaced; undo refused", "Объект изменён после операции, отмена невозможна"),
    ("Original path was occupied", "Исходное имя уже занято"),
    ("Undo is available only for moves and renames", "Отмена доступна только для переноса и переименования"),
    ("Reserved Windows device name", "Зарезервированное имя устройства Windows"),
    ("Invalid Windows file or folder name", "Недопустимое имя файла или папки"),
    ("Destination parent is not a directory", "Папка назначения недоступна"),
    ("Index limit reached; change rolled back", "Достигнут предел индекса, изменения отменены"),
    ("Too many files to index; choose a smaller folder", "Слишком много файлов для индекса, выберите папку поменьше"),
    ("Access is denied.", "Доступ запрещён."),
    ("Permission denied", "Доступ запрещён"),
    ("No such file or directory", "Файл или папка не найдены"),
    ("The system cannot find the file specified.", "Файл не найден."),
    ("The system cannot find the path specified.", "Путь не найден."),
];

pub fn localize(text: &str) -> String {
    let mut out = text.to_string();
    for (english, russian) in PHRASES {
        if out.contains(english) {
            out = out.replace(english, russian);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn replaces_known_phrases_only() {
        assert_eq!(
            super::localize("Ошибка: notes.txt: Destination already exists"),
            "Ошибка: notes.txt: Объект с таким именем уже существует, замена запрещена"
        );
        assert_eq!(super::localize("something else"), "something else");
    }
}
