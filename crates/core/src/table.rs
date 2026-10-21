//! Tables for the quick look: the start of a CSV file or of an Excel or
//! OpenDocument sheet, as text cells. Read-only, bounded in rows and columns.

use std::io::{self, Read};
use std::path::Path;

pub struct Table {
    /// Sheet names (one empty name for a CSV file).
    pub sheets: Vec<String>,
    pub sheet: usize,
    pub rows: Vec<Vec<String>>,
    /// More rows or columns than shown.
    pub cut: bool,
}

fn extension(path: &Path) -> String {
    path.extension().map(|ext| ext.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Whether the quick look shows `path` as a table.
pub fn is_table(path: &Path) -> bool {
    matches!(extension(path).as_str(), "csv" | "tsv" | "xlsx" | "xlsm" | "xlsb" | "xls" | "ods")
}

/// Reads at most `max_rows` × `max_columns` cells of sheet `sheet`.
pub fn read(path: &Path, sheet: usize, max_rows: usize, max_columns: usize) -> io::Result<Table> {
    match extension(path).as_str() {
        "csv" | "tsv" => read_csv(path, max_rows, max_columns),
        _ => read_sheet(path, sheet, max_rows, max_columns),
    }
}

fn read_csv(path: &Path, max_rows: usize, max_columns: usize) -> io::Result<Table> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(4 * 1024 * 1024).read_to_end(&mut bytes)?;
    let text = decode(&bytes);
    let delimiter = if extension(path) == "tsv" { '\t' } else { guess_delimiter(&text) };
    let (mut rows, mut cut) = parse_csv(&text, delimiter, max_rows);
    for row in &mut rows {
        if row.len() > max_columns { row.truncate(max_columns); cut = true; }
    }
    Ok(Table { sheets: vec![String::new()], sheet: 0, rows, cut })
}

/// UTF-8 (with or without a BOM), else Windows-1251, as Russian Excel saves.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        // A cut in the middle of a character at the 4 MB limit.
        Err(error) if error.error_len().is_none() => String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned(),
        Err(_) => encoding_rs::WINDOWS_1251.decode(bytes).0.into_owned(),
    }
}

/// The separator most used in the first line: `;` (Russian Excel), `,` or tab.
fn guess_delimiter(text: &str) -> char {
    let first = text.lines().next().unwrap_or("");
    let mut quoted = false;
    let mut counts = [0usize; 3];
    for c in first.chars() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => counts[0] += 1,
            ',' if !quoted => counts[1] += 1,
            '\t' if !quoted => counts[2] += 1,
            _ => {}
        }
    }
    let best = (0..3).max_by_key(|&index| (counts[index], 3 - index)).unwrap_or(0);
    [';', ',', '\t'][best]
}

/// Fields with quotes, doubled quotes and line breaks inside quotes.
fn parse_csv(text: &str, delimiter: char, max_rows: usize) -> (Vec<Vec<String>>, bool) {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => { field.push('"'); chars.next(); }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                if rows.len() == max_rows { return (rows, chars.peek().is_some()); }
            }
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    (rows, false)
}

fn read_sheet(path: &Path, sheet: usize, max_rows: usize, max_columns: usize) -> io::Result<Table> {
    use calamine::Reader;
    let mut workbook = calamine::open_workbook_auto(path).map_err(|error| io::Error::other(format!("таблица не прочитана: {error}")))?;
    let sheets = workbook.sheet_names();
    if sheets.is_empty() {
        return Err(io::Error::other("в книге нет листов"));
    }
    let sheet = sheet.min(sheets.len() - 1);
    let range = workbook.worksheet_range(&sheets[sheet]).map_err(|error| io::Error::other(format!("лист не прочитан: {error}")))?;
    let (height, width) = range.get_size();
    let rows = range.rows().take(max_rows)
        .map(|row| row.iter().take(max_columns).map(cell_text).collect())
        .collect();
    Ok(Table { sheets, sheet, rows, cut: height > max_rows || width > max_columns })
}

fn cell_text(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Float(value) => number(*value),
        calamine::Data::DateTime(value) => match value.as_datetime() {
            Some(time) if time.time() == chrono::NaiveTime::MIN => time.format("%d.%m.%Y").to_string(),
            Some(time) => time.format("%d.%m.%Y %H:%M").to_string(),
            None => value.to_string(),
        },
        calamine::Data::Bool(value) => if *value { "ИСТИНА" } else { "ЛОЖЬ" }.into(),
        other => other.to_string(),
    }
}

/// 0.30000000000000004 reads as 0,3: ten significant digits, decimal comma.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    let digits = (9 - value.abs().log10().floor() as i32).clamp(0, 12) as usize;
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    text.replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_csv_with_semicolons_and_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Ведомость.csv");
        let (text, _, _) = encoding_rs::WINDOWS_1251.encode("Марка;Кол-во;Примечание\r\nБ25;12;\"с \"\"добавкой\"\";\nвторая строка\"\r\nА500;3,5;\r\n");
        std::fs::write(&path, &text).unwrap();
        let table = read(&path, 0, 100, 10).unwrap();
        assert_eq!(table.rows[0], ["Марка", "Кол-во", "Примечание"]);
        assert_eq!(table.rows[1][2], "с \"добавкой\";\nвторая строка");
        assert_eq!(table.rows[2], ["А500", "3,5", ""]);
        assert!(!table.cut);
        let first = read(&path, 0, 1, 10).unwrap();
        assert_eq!(first.rows.len(), 1);
        assert!(first.cut);
    }

    #[test]
    fn delimiters_and_numbers() {
        assert_eq!(guess_delimiter("a,b,\"c;d\"\n"), ',');
        assert_eq!(guess_delimiter("a\tb\tc"), '\t');
        assert_eq!(guess_delimiter("a;b"), ';');
        assert_eq!(number(0.1 + 0.2), "0,3");
        assert_eq!(number(12.0), "12");
        assert_eq!(number(-1234.5678), "-1234,5678");
        assert!(is_table(Path::new("Смета.XLSX")));
        assert!(!is_table(Path::new("Записка.docx")));
    }

    #[test]
    fn a_damaged_workbook_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Смета.xlsx");
        std::fs::write(&path, b"not a workbook").unwrap();
        assert!(read(&path, 0, 10, 10).is_err());
    }
}
