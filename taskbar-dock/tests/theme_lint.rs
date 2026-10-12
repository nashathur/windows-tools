//! "Follow the design code", checked by the build: widget source may not hold raw colours, font
//! names or literal font sizes; they come from `taskbar_dock::theme` (tokens, `brand` entries,
//! named overrides). Runs over every widget's `src` folder; `scripts\dev.ps1` runs it for one
//! widget with `THEME_LINT_SRC=<crate>\src`. A line that really needs a literal says so with a
//! `theme-lint: allow <reason>` comment.

use std::path::{Path, PathBuf};

const WIDGETS: &[&str] = &["taskbar-weather/src", "taskbar-discord/src", "taskbar-media/src", "buswidget/rust/src", "taskbar-dock/src/settings_app", "taskbar-dock/src/bin/taskbar-dock-settings.rs"];
const FONT_NAMES: &[&str] = &["Segoe", "Arial", "Calibri", "Consolas", "Tahoma", "Verdana", "Courier", "Times New Roman"];

/// The code part of a line (without a `//` comment) and its string literals.
fn split(line: &str) -> (String, Vec<String>) {
    let (mut code, mut strings, mut cur) = (String::new(), Vec::new(), None::<String>);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match cur.as_mut() {
            Some(s) => match c {
                '\\' => {
                    s.push(c);
                    if let Some(n) = chars.next() {
                        s.push(n);
                    }
                }
                '"' => strings.push(cur.take().unwrap_or_default()),
                _ => s.push(c),
            },
            None => match c {
                '/' if chars.peek() == Some(&'/') => break,
                '"' => {
                    cur = Some(String::new());
                    code.push_str("\"\"");
                }
                '\'' => {
                    // a char literal ('"', '\u{..}') or a lifetime: copy it through
                    code.push(c);
                    let rest: String = chars.clone().take(12).collect();
                    if let Some(end) = rest.find('\'').filter(|&e| e > 0 && (e <= 2 || rest.starts_with('\\'))) {
                        for _ in 0..=end {
                            chars.next();
                        }
                        code.push('\'');
                    }
                }
                _ => code.push(c),
            },
        }
    }
    (code, strings)
}

/// A hex literal of exactly six digits (0xRRGGBB): a colour, not a mask like 0x00ff_ffff.
fn hex_colour(code: &str) -> bool {
    let b = code.as_bytes();
    (0..b.len().saturating_sub(1)).any(|i| {
        let starts = b[i] == b'0' && (b[i + 1] == b'x' || b[i + 1] == b'X') && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'));
        let digits = b[i + 2..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
        starts && digits == 6 && b[i + 2..i + 8].iter().all(u8::is_ascii_hexdigit)
    })
}

/// A font created with a number for its size: `Font::new(s(12.0)`, `CreateFontW(-px(14.0)`, `font(13.0`.
fn literal_font_size(code: &str) -> bool {
    ["Font::new(", "CreateFontW(", "font("].iter().any(|call| {
        code.match_indices(call).any(|(i, _)| {
            if *call == "font(" && code[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == ':') {
                return false; // part of another name (`badge::font(`, `my_font(`)
            }
            let mut rest = code[i + call.len()..].trim_start().trim_start_matches('-');
            for wrap in ["s(", "px("] {
                rest = rest.strip_prefix(wrap).unwrap_or(rest);
            }
            rest.starts_with(|c: char| c.is_ascii_digit())
        })
    })
}

/// The problems on one line, if any.
fn check_line(line: &str) -> Vec<&'static str> {
    if line.contains("theme-lint: allow") {
        return Vec::new();
    }
    let (code, strings) = split(line);
    let mut out = Vec::new();
    if code.contains("rgb(0x") || hex_colour(&code) {
        out.push("raw colour");
    }
    if strings.iter().any(|s| FONT_NAMES.iter().any(|f| s.contains(f))) {
        out.push("font name");
    }
    if literal_font_size(&code) {
        out.push("literal font size");
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn widget_sources_follow_the_theme() {
    let dirs: Vec<PathBuf> = match std::env::var("THEME_LINT_SRC") {
        Ok(d) => vec![PathBuf::from(d)],
        Err(_) => {
            let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
            WIDGETS.iter().map(|w| tools.join(w)).filter(|d| d.exists()).collect()
        }
    };
    assert!(!dirs.is_empty(), "no widget source folders found");
    let mut problems = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        if dir.is_file() {
            files.push(dir.clone());
        } else {
            rust_files(&dir, &mut files);
        }
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                for p in check_line(line) {
                    problems.push(format!("{}:{}: {p} (use taskbar_dock::theme): {}", f.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(problems.is_empty(), "theme lint:\n{}", problems.join("\n"));
}

#[test]
fn lint_catches_and_passes_the_right_things() {
    let bad = [
        r#"let c = rgb(0x1A, 0x1A, 0x1A);"#,
        r#"const RED: u32 = 0xF23F43;"#,
        r#"let f = Font::new(s(12.0), 400, &look.font, q);"#,
        r#"let f = Font::new(64, 400, ICON, q);"#,
        r#"CreateFontW(-px(14.0), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr())"#,
        r#"let fonts = [font(13.0, 400), font(14.0, 400)];"#,
        r#"let face = wide("Segoe UI");"#,
        r#"const ICON_FONT: &str = "Segoe Fluent Icons";"#,
    ];
    for l in bad {
        assert!(!check_line(l).is_empty(), "missed: {l}");
    }
    let good = [
        r#"cv.pixels().iter().map(|p| p | 0xff00_0000).collect()"#,
        r#"bg & 0xff_ffff"#,
        r#"let white = rgb(255, 255, 255);"#,
        r#"/// Segoe Fluent Icons glyphs (Windows 11's own icon font)."#,
        r#"let x = 1; // 0x123456 in a comment"#,
        r#"let f = Font::new(s(theme::BODY.px), theme::BODY.weight, &t.font.text, q);"#,
        r#"let f = Font::new(gw, 400, ICON_FONT, q);"#,
        r#"let f = badge::font(&lk, s);"#,
        r#"let id = 0x1234_5678; let k = 0x02A3;"#,
        r#"let g = '\u{E8A7}'; let q = '"'; let s = "a // b";"#,
        r#"let c = rgb(0x60, 0x60, 0x60); // theme-lint: allow test fixture"#,
    ];
    for l in good {
        assert!(check_line(l).is_empty(), "false alarm {:?}: {l}", check_line(l));
    }
}
