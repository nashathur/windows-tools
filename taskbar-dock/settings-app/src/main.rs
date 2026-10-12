#![windows_subsystem = "windows"]

use std::path::Path;
use taskbar_dock::registry;
use taskbar_dock_settings::model::Page;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--register") {
        if args.len() != 2 {
            std::process::exit(1);
        }
        match registry::register(Path::new(&args[1])) {
            Ok(entry) => {
                println!("{}\t{}\t{}", entry.name, entry.exe.display(), entry.class);
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let page = match args.as_slice() {
        [] => Page::Widgets,
        [flag, name] if flag == "--widget" => Page::Widget(name.clone()),
        [flag] if flag == "--dock" => Page::Dock,
        [flag] if flag == "--developer" => Page::Developer,
        _ => std::process::exit(1),
    };
    std::process::exit(taskbar_dock_settings::win::run(page));
}
