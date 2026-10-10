//! Bounded, streaming verification. Retains no bytes or hashes after the call.
use crate::operations::CopyControl;
use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Stream {
    suffix: OsString,
    size: u64,
}
#[derive(Debug, PartialEq, Eq)]
struct Item {
    relative: PathBuf,
    directory: bool,
    streams: Vec<Stream>,
}

fn enumerate(root: &Path, control: &CopyControl) -> io::Result<Vec<Item>> {
    let mut items = Vec::new();
    for next in walkdir::WalkDir::new(root).follow_links(false) {
        control.check()?;
        let entry = next.map_err(io::Error::other)?;
        crate::folder_copy::reject_link(entry.path())?;
        let meta = fs::symlink_metadata(entry.path())?;
        items.push(Item {
            relative: entry
                .path()
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_owned(),
            directory: meta.is_dir(),
            streams: streams(entry.path(), &meta)?,
        });
        if items.len() > 100_001 {
            return Err(io::Error::other("Verification tree limit exceeded"));
        }
    }
    items.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(items)
}

fn stream_path(path: &Path, suffix: &std::ffi::OsStr) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn open_read(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    options.open(path)
}

pub(crate) fn verify(source: &Path, target: &Path, control: &CopyControl) -> io::Result<()> {
    let items = enumerate(source, control)?;
    if items != enumerate(target, control)? {
        return Err(io::Error::other(
            "Copy tree or data streams differ from the source",
        ));
    }
    let mut left = vec![0; 256 * 1024];
    let mut right = vec![0; 256 * 1024];
    for item in &items {
        for stream in &item.streams {
            control.check()?;
            // Joining an empty relative path adds a trailing separator to a file.
            let a_path = if item.relative.as_os_str().is_empty() {
                source.to_owned()
            } else {
                source.join(&item.relative)
            };
            let b_path = if item.relative.as_os_str().is_empty() {
                target.to_owned()
            } else {
                target.join(&item.relative)
            };
            let mut a = open_read(&stream_path(&a_path, &stream.suffix))?;
            let mut b = open_read(&stream_path(&b_path, &stream.suffix))?;
            let mut remaining = stream.size;
            while remaining > 0 {
                control.check()?;
                let count = remaining.min(left.len() as u64) as usize;
                a.read_exact(&mut left[..count])?;
                b.read_exact(&mut right[..count])?;
                if left[..count] != right[..count] {
                    return Err(io::Error::other(format!(
                        "Copy contents differ: {}",
                        item.relative.display()
                    )));
                }
                remaining -= count as u64;
            }
            // Growth after stream enumeration must not validate a truncated copy.
            if a.read(&mut left[..1])? != 0 || b.read(&mut right[..1])? != 0 {
                return Err(io::Error::other("Stream grew during copy verification"));
            }
        }
    }
    control.check()?;
    if items != enumerate(source, control)? || items != enumerate(target, control)? {
        return Err(io::Error::other("Tree changed during copy verification"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn streams(_: &Path, meta: &fs::Metadata) -> io::Result<Vec<Stream>> {
    Ok(if meta.is_file() {
        vec![Stream {
            suffix: OsString::new(),
            size: meta.len(),
        }]
    } else {
        Vec::new()
    })
}

#[cfg(windows)]
fn streams(path: &Path, meta: &fs::Metadata) -> io::Result<Vec<Stream>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            FindClose, FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard,
            WIN32_FIND_STREAM_DATA,
        },
    };
    struct Search(HANDLE);
    impl Drop for Search {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };
    let handle = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        // FAT-like volumes have no named streams. Comparing this default-only
        // manifest to NTFS still rejects any named streams lost on the target.
        if matches!(error.raw_os_error(), Some(38 | 87)) {
            return Ok(if meta.is_file() {
                vec![Stream {
                    suffix: OsString::new(),
                    size: meta.len(),
                }]
            } else {
                Vec::new()
            });
        }
        return Err(error);
    }
    let search = Search(handle);
    let mut result = Vec::new();
    loop {
        let len = data
            .cStreamName
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| io::Error::other("Invalid stream name"))?;
        let name = OsString::from_wide(&data.cStreamName[..len]);
        if data.StreamSize < 0 {
            return Err(io::Error::other("Invalid stream size"));
        }
        let suffix = if name == "::$DATA" {
            OsString::new()
        } else {
            name
        };
        result.push(Stream {
            suffix,
            size: data.StreamSize as u64,
        });
        if result.len() > 1024 {
            return Err(io::Error::other("Too many data streams to verify safely"));
        }
        if unsafe { FindNextStreamW(search.0, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) }
            == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(38) {
                return Err(error);
            }
            break;
        }
    }
    result.sort();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::CopyControl;
    use std::fs;

    #[test]
    fn equal_single_files_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("source");
        let b = tmp.path().join("copy");
        fs::write(&a, b"original").unwrap();
        fs::write(&b, b"original").unwrap();
        verify(&a, &b, &CopyControl::default()).unwrap();
    }
    #[test]
    fn equal_tree_and_empty_directories_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("source");
        let b = tmp.path().join("copy");
        for root in [&a, &b] {
            fs::create_dir(root).unwrap();
            fs::create_dir(root.join("empty")).unwrap();
            fs::write(root.join("данные.bin"), vec![45; 600_000]).unwrap();
        }
        verify(&a, &b, &CopyControl::default()).unwrap();
    }
    #[test]
    fn equal_size_but_different_contents_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("source");
        let b = tmp.path().join("copy");
        fs::write(&a, b"original").unwrap();
        fs::write(&b, b"modified").unwrap();
        assert!(verify(&a, &b, &CopyControl::default()).is_err());
        assert_eq!(fs::read(&a).unwrap(), b"original");
    }
    #[test]
    fn changed_tree_and_cancelled_verification_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("source");
        let b = tmp.path().join("copy");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("new work"), b"keep").unwrap();
        assert!(verify(&a, &b, &CopyControl::default()).is_err());
        let control = CopyControl::default();
        control.cancel();
        assert_eq!(
            verify(&a, &a, &control).unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
    }
    #[cfg(unix)]
    #[test]
    fn verification_never_follows_links() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("source");
        let b = tmp.path().join("link");
        fs::write(&a, b"original").unwrap();
        symlink(&a, &b).unwrap();
        assert!(verify(&a, &b, &CopyControl::default()).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn altered_alternate_stream_and_missing_directory_stream_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("source");
        let b = tmp.path().join("copy");
        fs::write(&a, b"original").unwrap();
        fs::write(&b, b"original").unwrap();
        fs::write(tmp.path().join("source:notes"), b"one").unwrap();
        fs::write(tmp.path().join("copy:notes"), b"two").unwrap();
        assert!(verify(&a, &b, &CopyControl::default()).is_err());
        fs::write(tmp.path().join("copy:notes"), b"one").unwrap();
        verify(&a, &b, &CopyControl::default()).unwrap();
        let d = tmp.path().join("dir");
        let e = tmp.path().join("other");
        fs::create_dir(&d).unwrap();
        fs::create_dir(&e).unwrap();
        fs::write(tmp.path().join("dir:notes"), b"directory data").unwrap();
        assert!(verify(&d, &e, &CopyControl::default()).is_err());
    }
}
