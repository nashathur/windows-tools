//! Windows settings application executable.

#![windows_subsystem = "windows"]

#[path = "../settings_app/mod.rs"]
mod settings_app;

use std::path::Path;
use taskbar_dock::registry;

/// Parses settings app arguments, handles registration, and starts the window.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--register") {
        if args.len() != 2 {
            println!("--register requires one executable path");
            std::process::exit(1);
        }
        match registry::register(Path::new(&args[1])) {
            Ok(entry) => println!("{}\t{}\t{}", entry.name, entry.exe.display(), entry.class),
            Err(error) => {
                println!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let page = match args.as_slice() {
        [] => settings_app::model::Page::Widgets,
        [flag, name] if flag == "--widget" => settings_app::model::Page::Widget(name.clone()),
        [flag] if flag == "--dock" => settings_app::model::Page::Dock,
        [flag] if flag == "--developer" => settings_app::model::Page::Developer,
        _ => {
            eprintln!("usage: taskbar-dock-settings [--widget <name>|--dock|--developer|--register <exe>]");
            std::process::exit(1);
        }
    };
    let code = settings_app::win::run(page);
    if code != 0 {
        std::process::exit(code);
    }
}
