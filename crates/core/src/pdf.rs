//! PDF pages for the quick look, drawn by the PDF engine built into Windows
//! (Windows.Data.Pdf, Windows 10 and later): nothing to install.

use std::io;
use std::path::Path;

/// Page `index` (from 0; past the end: the last) of the PDF, as PNG
/// `width` pixels wide, and the number of pages.
#[cfg(windows)]
pub fn render_page(path: &Path, index: u32, width: u32) -> io::Result<(u32, Vec<u8>)> {
    use windows::core::HSTRING;
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};

    let plain = crate::path_utils::normalize_extended_path(path);
    let run = || -> windows::core::Result<(u32, Vec<u8>)> {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(plain.to_string_lossy().as_ref()))?.join()?;
        let document = PdfDocument::LoadFromFileAsync(&file)?.join()?;
        let pages = document.PageCount()?;
        let page = document.GetPage(index.min(pages.saturating_sub(1)))?;
        let options = PdfPageRenderOptions::new()?;
        options.SetDestinationWidth(width)?;
        // PNG is the default encoding of a rendered page.
        let stream = InMemoryRandomAccessStream::new()?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)?.join()?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size)?.join()?;
        let mut png = vec![0; size as usize];
        reader.ReadBytes(&mut png)?;
        Ok((pages, png))
    };
    // WinRT calls need the thread in a COM apartment.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    run().map_err(|error| io::Error::other(format!("PDF не открыт: {}", error.message())))
}

#[cfg(not(windows))]
pub fn render_page(_path: &Path, _index: u32, _width: u32) -> io::Result<(u32, Vec<u8>)> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "страницы PDF показывает только Windows"))
}
