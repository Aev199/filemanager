//! The preview picture AutoCAD stores inside every DWG file (BMP in older
//! versions, PNG since 2013). Read directly, so drawings get thumbnails on
//! computers without AutoCAD or DWG TrueView. Only the picture is read.
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const SENTINEL: [u8; 16] = [0x1F, 0x25, 0x6D, 0x07, 0xD4, 0x36, 0x28, 0x28, 0x9D, 0x57, 0xCA, 0x3F, 0x9D, 0x44, 0x10, 0x2B];
const LIMIT: u32 = 16 * 1024 * 1024;

fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }

fn read_u32(file: &mut File) -> io::Result<u32> {
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// The drawing's preview picture.
pub fn preview(path: &Path) -> io::Result<image::DynamicImage> {
    let mut file = File::open(path)?;
    let mut version = [0; 6];
    file.read_exact(&mut version)?;
    if &version[..4] != b"AC10" { return Err(invalid("Это не файл DWG")); }
    file.seek(SeekFrom::Start(0x0D))?;
    let seeker = read_u32(&mut file)?;
    file.seek(SeekFrom::Start(seeker as u64))?;
    let mut sentinel = [0; 16];
    file.read_exact(&mut sentinel)?;
    if sentinel != SENTINEL { return Err(invalid("В файле DWG нет картинки предпросмотра")); }
    let _overall = read_u32(&mut file)?;
    let mut count = [0; 1];
    file.read_exact(&mut count)?;
    let mut entries = Vec::new();
    for _ in 0..count[0] {
        let mut code = [0; 1];
        file.read_exact(&mut code)?;
        let start = read_u32(&mut file)?;
        let size = read_u32(&mut file)?;
        entries.push((code[0], start, size));
    }
    // PNG first: it is the sharper picture of newer files.
    for wanted in [6u8, 2] {
        let Some(&(_, start, size)) = entries.iter().find(|(code, _, size)| *code == wanted && *size > 0) else { continue };
        if size > LIMIT { return Err(invalid("Картинка DWG слишком большая")); }
        file.seek(SeekFrom::Start(start as u64))?;
        let mut data = vec![0; size as usize];
        file.read_exact(&mut data)?;
        return if wanted == 6 {
            image::load_from_memory_with_format(&data, image::ImageFormat::Png).map_err(io::Error::other)
        } else {
            image::load_from_memory_with_format(&with_file_header(&data)?, image::ImageFormat::Bmp).map_err(io::Error::other)
        };
    }
    Err(invalid("В файле DWG нет картинки предпросмотра"))
}

/// DWG keeps a bare DIB; a BMP file adds the 14-byte header before it.
fn with_file_header(dib: &[u8]) -> io::Result<Vec<u8>> {
    if dib.len() < 40 { return Err(invalid("Повреждённая картинка DWG")); }
    let header = u32::from_le_bytes(dib[0..4].try_into().unwrap());
    let bits = u16::from_le_bytes(dib[14..16].try_into().unwrap());
    let used = u32::from_le_bytes(dib[32..36].try_into().unwrap());
    let colours = if bits <= 8 { if used > 0 { used } else { 1 << bits } } else { used };
    let offset = 14 + header + colours * 4;
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal DWG: version, image seeker at 0x0D, the picture section.
    fn drawing(code: u8, picture: &[u8]) -> Vec<u8> {
        let mut file = b"AC1032".to_vec();
        file.resize(0x40, 0);
        let section = file.len() as u32;
        file[0x0D..0x11].copy_from_slice(&section.to_le_bytes());
        file.extend_from_slice(&SENTINEL);
        file.extend_from_slice(&0u32.to_le_bytes());
        file.push(1);
        let start = file.len() as u32 + 9;
        file.push(code);
        file.extend_from_slice(&start.to_le_bytes());
        file.extend_from_slice(&(picture.len() as u32).to_le_bytes());
        file.extend_from_slice(picture);
        file
    }

    #[test]
    fn reads_the_bmp_picture() {
        // 2×1 pixels, 24 bit, rows padded to 4 bytes: blue then red.
        let mut dib = Vec::new();
        for value in [40u32, 2, 1] { dib.extend_from_slice(&value.to_le_bytes()); }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[255, 0, 0, 0, 0, 255, 0, 0]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.dwg");
        std::fs::write(&path, drawing(2, &dib)).unwrap();
        let picture = preview(&path).unwrap().to_rgb8();
        assert_eq!(picture.dimensions(), (2, 1));
        assert_eq!(picture.get_pixel(0, 0).0, [0, 0, 255]);
        assert_eq!(picture.get_pixel(1, 0).0, [255, 0, 0]);
    }

    #[test]
    fn reads_the_png_picture_and_refuses_other_files() {
        let mut png = Vec::new();
        image::RgbImage::from_pixel(3, 2, image::Rgb([1, 2, 3]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.dwg");
        std::fs::write(&path, drawing(6, &png)).unwrap();
        assert_eq!(preview(&path).unwrap().to_rgb8().dimensions(), (3, 2));
        std::fs::write(&path, b"not a drawing at all, just text").unwrap();
        assert!(preview(&path).is_err());
    }
}
