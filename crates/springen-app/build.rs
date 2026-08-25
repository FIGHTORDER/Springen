//! Embeds the Windows icon and version information into the executable, so the
//! app carries the contour mark in Explorer, the taskbar and Add/Remove
//! Programs rather than the default blank icon.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../packaging/windows/springen.ico");
    res.set("ProductName", "Springen");
    res.set("FileDescription", "Springen map design tool");
    res.set("CompanyName", "Springen");
    res.set("LegalCopyright", "MIT licensed");
    res.set("OriginalFilename", "springen-app.exe");
    if let Err(e) = res.compile() {
        // A missing windres should not stop a source build.
        println!("cargo:warning=windows resources not embedded: {e}");
    }
}
