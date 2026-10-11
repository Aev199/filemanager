use filemanager_core::thumbnail::{validate_bitmap, validate_cached};

#[test]
fn bitmap_dimensions_are_bounded_before_allocation() {
    assert_eq!(validate_bitmap(320, 200, 320).unwrap(), 320 * 200 * 4);
    for (w, h, e) in [
        (1, 1, 0),
        (1, 1, 513),
        (321, 1, 320),
        (1, 321, 320),
        (0, 1, 320),
        (u32::MAX, u32::MAX, 512),
    ] {
        assert!(validate_bitmap(w, h, e).is_err());
    }
}

#[test]
fn corrupt_or_oversized_cache_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cached.png");
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(validate_cached(&path, 320).is_err());
    image::save_buffer(&path, &vec![255; 400 * 4], 400, 1, image::ColorType::Rgba8).unwrap();
    assert!(validate_cached(&path, 320).is_err());
    image::save_buffer(&path, &vec![255; 100 * 4], 100, 1, image::ColorType::Rgba8).unwrap();
    assert!(validate_cached(&path, 320).is_ok());
    image::save_buffer(&path, &vec![255; 512 * 512 * 4], 512, 512, image::ColorType::Rgba8).unwrap();
    assert!(validate_cached(&path, 512).is_ok());
}

#[test]
fn valid_png_header_with_corrupt_pixels_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad-idat.png");
    image::save_buffer(&path, &[255; 16], 2, 2, image::ColorType::Rgba8).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let idat = bytes.windows(4).position(|chunk| chunk == b"IDAT").unwrap();
    bytes[idat + 4] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
    assert!(validate_cached(&path, 320).is_err());
}
