//! Embeds the application icon and version details into the Windows EXE.

fn main() {
    println!("cargo:rerun-if-changed=assets/app/icon.ico");
    println!("cargo:rerun-if-env-changed=PAPKA_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/app/icon.ico")
        .set("ProductName", "Papka")
        .set("FileDescription", "Papka — файловый менеджер")
        .set("OriginalFilename", "Papka.exe")
        .set_language(0x0419);
    // The CI build number goes into the file's properties: a Papka.exe
    // downloaded in a browser is recognised as a newer version by it.
    let version = std::env::var("PAPKA_VERSION").unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    let parts: Vec<u64> = version.split('.').map_while(|part| part.parse().ok()).chain(std::iter::repeat(0)).take(4).collect();
    let packed = parts.iter().fold(0u64, |packed, part| (packed << 16) | (part & 0xFFFF));
    resource
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
    // A missing resource compiler must not break the build; the EXE then
    // simply keeps the default icon.
    if let Err(error) = resource.compile() {
        println!("cargo:warning=Icon not embedded: {error}");
    }
}
