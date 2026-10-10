//! Embeds the application icon and version details into the Windows EXE.

fn main() {
    println!("cargo:rerun-if-changed=assets/app/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/app/icon.ico")
        .set("ProductName", "Filemanager")
        .set("FileDescription", "Filemanager — файловый менеджер")
        .set("OriginalFilename", "Filemanager.exe")
        .set_language(0x0419);
    // A missing resource compiler must not break the build; the EXE then
    // simply keeps the default icon.
    if let Err(error) = resource.compile() {
        println!("cargo:warning=Icon not embedded: {error}");
    }
}
