// Embed buswidget.exe.manifest (Windows 8+ support, needed for the taskbar-attached strip).
fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest = std::path::Path::new(&dir).join("buswidget.exe.manifest");
    println!("cargo:rerun-if-changed=buswidget.exe.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
}
