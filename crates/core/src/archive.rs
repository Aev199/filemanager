//! Zip and 7z archives shown as folders: `D:\Проект\Выдача.zip\Чертежи` is
//! the folder "Чертежи" inside the archive. Archives are only read here;
//! files leave them by extraction into a real folder.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::browser::{Entry, Listing};

const EXTENSIONS: [&str; 2] = ["zip", "7z"];

/// Whether the name says zip or 7z (the file itself is not read).
pub fn is_archive_name(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// The archive file a path is in, and the path inside it ("" for its top),
/// with `/` between names. None for ordinary paths.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let mut archive = PathBuf::new();
    let mut components = path.components();
    while let Some(component) = components.next() {
        archive.push(component);
        if matches!(component, Component::Normal(_)) && is_archive_name(&archive) && archive.is_file() {
            let inner: Vec<String> = components.map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
            return Some((archive, inner.join("/")));
        }
    }
    None
}

/// Whether `path` lies inside an archive (the archive file itself does not).
pub fn is_inside(path: &Path) -> bool {
    split(path).is_some_and(|(archive, _)| archive != path)
}

struct Item {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

fn sevenz_error(error: sevenz_rust2::Error) -> io::Error {
    match error {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) =>
            io::Error::new(io::ErrorKind::PermissionDenied, "архив защищён паролем"),
        other => io::Error::other(format!("архив 7z не прочитан: {other}")),
    }
}

fn zip_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(error) => error,
        zip::result::ZipError::UnsupportedArchive(what) => io::Error::other(format!("формат zip не поддерживается: {what}")),
        zip::result::ZipError::InvalidArchive(_) => io::Error::other("архив повреждён или это не zip"),
        other => io::Error::other(format!("архив zip не прочитан: {other}")),
    }
}

fn is_7z(archive: &Path) -> bool {
    archive.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("7z"))
}

/// Zip names are UTF-8 in new archives and the DOS code page (866 for
/// Russian Windows) in old ones.
fn zip_name(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(name) => name.to_owned(),
        Err(_) => raw.iter().map(|&byte| cp866(byte)).collect(),
    }
}

fn cp866(byte: u8) -> char {
    match byte {
        0x00..=0x7F => byte as char,
        0x80..=0xAF => char::from_u32(0x0410 + (byte - 0x80) as u32).unwrap_or('?'),
        0xE0..=0xEF => char::from_u32(0x0440 + (byte - 0xE0) as u32).unwrap_or('?'),
        0xF0 => 'Ё',
        0xF1 => 'ё',
        _ => '_',
    }
}

fn zip_time(time: zip::DateTime) -> Option<SystemTime> {
    let date = chrono::NaiveDate::from_ymd_opt(time.year() as i32, time.month() as u32, time.day() as u32)?;
    let local = date.and_hms_opt(time.hour() as u32, time.minute() as u32, time.second() as u32)?
        .and_local_timezone(chrono::Local).earliest()?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(local.timestamp().max(0) as u64))
}

/// Names cleaned of `\`, leading `/` and `./`.
fn clean(name: &str) -> String {
    name.replace('\\', "/").split('/').filter(|part| !part.is_empty() && *part != ".").collect::<Vec<_>>().join("/")
}

fn items(archive: &Path) -> io::Result<Vec<Item>> {
    if is_7z(archive) {
        let read = sevenz_rust2::Archive::open(archive).map_err(sevenz_error)?;
        Ok(read.files.iter().filter(|file| !file.is_anti_item).map(|file| Item {
            name: clean(&file.name),
            is_dir: file.is_directory,
            size: file.size,
            modified: file.has_last_modified_date.then(|| file.last_modified_date.into()),
        }).collect())
    } else {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(zip_error)?;
        let mut items = Vec::with_capacity(zip.len());
        for index in 0..zip.len() {
            let file = zip.by_index_raw(index).map_err(zip_error)?;
            items.push(Item {
                name: clean(&zip_name(file.name_raw())),
                is_dir: file.is_dir(),
                size: file.size(),
                modified: file.last_modified().and_then(zip_time),
            });
        }
        Ok(items)
    }
}

/// The contents of `inner` ("" for the top) of `archive`, as a folder listing.
/// Folders named only in the paths of their files are there too.
pub fn listing(archive: &Path, inner: &str) -> io::Result<Listing> {
    let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
    let mut children: BTreeMap<String, Entry> = BTreeMap::new();
    let mut found_inner = inner.is_empty();
    for item in items(archive)? {
        if item.name.is_empty() { continue; }
        if item.name == inner { found_inner = true; continue; }
        let Some(rest) = item.name.strip_prefix(&prefix) else { continue };
        found_inner = true;
        let (name, deeper) = match rest.split_once('/') {
            Some((name, _)) => (name, true),
            None => (rest, false),
        };
        let is_dir = deeper || item.is_dir;
        let path = inner_path(archive, &format!("{prefix}{name}"));
        let entry = children.entry(name.to_owned()).or_insert_with(|| Entry {
            path, name: name.to_owned(), is_directory: is_dir, is_symlink: false, hidden: false,
            size: 0, modified: None,
        });
        if deeper {
            entry.is_directory = true;
        } else {
            entry.is_directory |= item.is_dir;
            if !item.is_dir { entry.size = item.size; }
            entry.modified = entry.modified.or(item.modified);
        }
    }
    if !found_inner {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("в архиве нет папки «{inner}»")));
    }
    Ok(Listing { entries: children.into_values().collect(), truncated: false })
}

/// The path shown for `inner` of `archive`.
pub fn inner_path(archive: &Path, inner: &str) -> PathBuf {
    let mut path = archive.to_path_buf();
    for part in inner.split('/').filter(|part| !part.is_empty()) { path.push(part); }
    path
}

/// A name inside the archive made safe as a relative path: no `..`, no
/// drive or root, no characters Windows forbids.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." { continue; }
        if part == ".." || part.contains(':') { return None; }
        let part: String = part.chars().map(|c| if "<>\"|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
        let part = part.trim_end_matches([' ', '.']);
        if part.is_empty() { return None; }
        path.push(part);
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// A free name in `folder`: `name`, else `name (2)`, `name (3)`…
fn free_name(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() { return first; }
    let (stem, ext) = match name.rfind('.').filter(|dot| *dot > 0) {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };
    (2..).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|path| !path.exists()).unwrap()
}

/// Extracts `items` (paths inside `archive`, files or folders with all they
/// hold) into `target`, next to each other as they were selected. A name
/// already taken in `target` gets " (2)". Returns what was created there.
pub fn extract(archive: &Path, items: &[String], target: &Path) -> io::Result<Vec<PathBuf>> {
    if !target.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "папки для извлечения нет"));
    }
    // Each selected item lands in `target` under a free top name.
    let mut tops: Vec<(String, PathBuf)> = Vec::new();
    for item in items {
        let item = clean(item);
        let name = item.rsplit('/').next().unwrap_or(&item).to_owned();
        let Some(safe) = safe_relative(&name) else { continue };
        let top = free_name(target, &safe.to_string_lossy());
        tops.push((item, top));
    }
    let destination = |name: &str| -> Option<PathBuf> {
        tops.iter().find_map(|(item, top)| {
            if name == item { return Some(top.clone()); }
            let rest = name.strip_prefix(item.as_str())?.strip_prefix('/')?;
            safe_relative(rest).map(|rest| top.join(rest))
        })
    };
    let mut written: HashMap<PathBuf, ()> = HashMap::new();
    let mut write = |path: PathBuf, is_dir: bool, reader: &mut dyn io::Read, modified: Option<SystemTime>| -> io::Result<()> {
        if is_dir {
            fs::create_dir_all(&path)?;
        } else {
            if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
            let mut file = fs::File::create_new(&path)?;
            io::copy(reader, &mut file)?;
            drop(file);
            if let Some(modified) = modified {
                let _ = filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(modified));
            }
        }
        written.insert(path, ());
        Ok(())
    };
    if is_7z(archive) {
        let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty()).map_err(sevenz_error)?;
        let mut failure: Option<io::Error> = None;
        reader.for_each_entries(|file, data| {
            if file.is_anti_item { return Ok(true); }
            if let Some(path) = destination(&clean(&file.name)) {
                let modified = file.has_last_modified_date.then(|| file.last_modified_date.into());
                if let Err(error) = write(path, file.is_directory, data, modified) {
                    failure = Some(error);
                    return Ok(false);
                }
            } else {
                // Solid blocks must be read through to reach later files.
                io::copy(data, &mut io::sink())?;
            }
            Ok(true)
        }).map_err(sevenz_error)?;
        if let Some(error) = failure { return Err(error); }
    } else {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(zip_error)?;
        for index in 0..zip.len() {
            let name = clean(&zip_name(zip.by_index_raw(index).map_err(zip_error)?.name_raw()));
            let Some(path) = destination(&name) else { continue };
            let mut file = zip.by_index(index).map_err(|error| match error {
                zip::result::ZipError::UnsupportedArchive(what) if what.contains("Password") || what.contains("encrypted") =>
                    io::Error::new(io::ErrorKind::PermissionDenied, "архив защищён паролем"),
                other => zip_error(other),
            })?;
            let (is_dir, modified) = (file.is_dir(), file.last_modified().and_then(zip_time));
            write(path, is_dir, &mut file, modified)?;
        }
    }
    let created: Vec<PathBuf> = tops.into_iter().map(|(_, top)| top).filter(|top| written.contains_key(top)
        || written.keys().any(|path| path.starts_with(top))).collect();
    if created.is_empty() && !items.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "в архиве нет выбранных файлов"));
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_zip(path: &Path) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("Чертежи/Лист 1.dwg", options).unwrap();
        zip.write_all(b"dwg").unwrap();
        zip.start_file("Чертежи/Узлы/Узел А.pdf", options).unwrap();
        zip.write_all(b"pdf").unwrap();
        zip.start_file("Записка.docx", options).unwrap();
        zip.write_all(b"docx!").unwrap();
        zip.start_file("../escape.txt", options).unwrap();
        zip.write_all(b"no").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn a_zip_reads_as_folders() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("Выдача.zip");
        sample_zip(&archive);
        let (found, inner) = split(&archive.join("Чертежи").join("Узлы")).unwrap();
        assert_eq!((found.as_path(), inner.as_str()), (archive.as_path(), "Чертежи/Узлы"));
        assert!(is_inside(&archive.join("Чертежи")));
        assert!(!is_inside(&archive));
        assert!(split(dir.path()).is_none());

        let top = listing(&archive, "").unwrap();
        let names: Vec<(&str, bool)> = top.entries.iter().map(|entry| (entry.name.as_str(), entry.is_directory)).collect();
        assert!(names.contains(&("Чертежи", true)));
        assert!(names.contains(&("Записка.docx", false)));
        assert_eq!(top.entries.iter().find(|entry| entry.name == "Записка.docx").unwrap().size, 5);
        let drawings = listing(&archive, "Чертежи").unwrap();
        assert_eq!(drawings.entries.len(), 2);
        assert_eq!(drawings.entries[0].path, archive.join("Чертежи").join("Лист 1.dwg"));
        assert!(listing(&archive, "Нет такой").is_err());
    }

    #[test]
    fn extraction_keeps_folders_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("Выдача.zip");
        sample_zip(&archive);
        let target = dir.path().join("Сюда");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("Записка.docx"), b"mine").unwrap();
        let created = extract(&archive, &["Чертежи".into(), "Записка.docx".into(), "../escape.txt".into()], &target).unwrap();
        // A name climbing out with ".." lands inside the target all the same.
        assert_eq!(created, [target.join("Чертежи"), target.join("Записка (2).docx"), target.join("escape.txt")]);
        assert_eq!(fs::read(target.join("Чертежи/Узлы/Узел А.pdf")).unwrap(), b"pdf");
        assert_eq!(fs::read(target.join("Записка.docx")).unwrap(), b"mine");
        assert!(!dir.path().join("escape.txt").exists());
        // One file from deep inside lands alone.
        let created = extract(&archive, &["Чертежи/Узлы/Узел А.pdf".into()], &target).unwrap();
        assert_eq!(created, [target.join("Узел А.pdf")]);
    }

    #[test]
    fn a_7z_reads_and_extracts() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        fs::create_dir_all(source.join("Расчёты")).unwrap();
        fs::write(source.join("Расчёты").join("Модель.lir"), b"model").unwrap();
        fs::write(source.join("readme.txt"), b"hi").unwrap();
        let archive = dir.path().join("Модель.7z");
        sevenz_rust2::compress_to_path(&source, &archive).unwrap();
        let top = listing(&archive, "").unwrap();
        assert!(top.entries.iter().any(|entry| entry.name == "Расчёты" && entry.is_directory));
        let target = dir.path().join("out");
        fs::create_dir(&target).unwrap();
        let created = extract(&archive, &["Расчёты".into()], &target).unwrap();
        assert_eq!(created, [target.join("Расчёты")]);
        assert_eq!(fs::read(target.join("Расчёты/Модель.lir")).unwrap(), b"model");
    }

    #[test]
    fn old_russian_zip_names_are_read() {
        // "Отчёт" in code page 866.
        assert_eq!(zip_name(&[0x8E, 0xE2, 0xE7, 0xF1, 0xE2]), "Отчёт");
        assert_eq!(zip_name("Отчёт".as_bytes()), "Отчёт");
        assert!(safe_relative("a/../b").is_none());
        assert!(safe_relative("C:/x").is_none());
        assert_eq!(safe_relative("a/b?.txt").unwrap(), Path::new("a").join("b_.txt"));
    }
}
