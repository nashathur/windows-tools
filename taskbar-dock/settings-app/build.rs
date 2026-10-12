fn main() {
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("settings-app.manifest");
    println!("cargo:rustc-link-arg-bin=taskbar-dock-settings=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=taskbar-dock-settings=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
