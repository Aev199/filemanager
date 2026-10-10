//! Human-readable sizes, dates and kinds for the Russian interface.

use std::time::SystemTime;

use chrono::{DateTime, Local};

use crate::browser::Entry;

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    if bytes < 1024 {
        return format!("{bytes} Б");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if value < 10.0 { format!("{value:.1}") } else { format!("{value:.0}") };
    format!("{} {}", text.replace('.', ","), UNITS[unit])
}

pub fn format_time(time: SystemTime) -> String {
    DateTime::<Local>::from(time).format("%d.%m.%Y %H:%M").to_string()
}

pub fn kind_label(entry: &Entry) -> String {
    if entry.is_directory {
        return "Папка".into();
    }
    match entry.extension() {
        Some(ext) => format!("Файл {}", ext.to_uppercase()),
        None => "Файл".into(),
    }
}

/// Russian plural: "1 элемент", "3 элемента", "5 элементов".
pub fn items_label(count: usize) -> String {
    let word = match (count % 10, count % 100) {
        (1, n) if n != 11 => "элемент",
        (2..=4, n) if !(12..=14).contains(&n) => "элемента",
        _ => "элементов",
    };
    format!("{count} {word}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 Б");
        assert_eq!(format_size(1023), "1023 Б");
        assert_eq!(format_size(1536), "1,5 КБ");
        assert_eq!(format_size(50 * 1024 * 1024), "50 МБ");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024 + 1), "3,0 ГБ");
    }

    #[test]
    fn plurals() {
        assert_eq!(items_label(1), "1 элемент");
        assert_eq!(items_label(3), "3 элемента");
        assert_eq!(items_label(11), "11 элементов");
        assert_eq!(items_label(22), "22 элемента");
        assert_eq!(items_label(25), "25 элементов");
        assert_eq!(items_label(101), "101 элемент");
    }
}
