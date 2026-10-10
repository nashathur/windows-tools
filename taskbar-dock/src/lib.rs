//! Shared building blocks for the taskbar widgets (bus widget, taskbar-weather, and any new one):
//!
//! - [`settings`]: the one settings file, `%APPDATA%\TaskbarDock\settings.json`, with a section
//!   per widget plus the shared `dock` section.
//! - [`style`]: the shared look and behaviour from the `dock` section (gap, font, radii,
//!   animation timings, hover delays, and "follow Windows" switches), resolved against
//!   the current Windows settings.
//! - [`system`]: the Windows settings the widgets follow live (theme, animation effects,
//!   mouse hover time, text size, time format, display scaling).
//! - [`dock`]: the stacking protocol: widgets docked into the taskbar form a stack against the
//!   tray, can be dragged to reorder, and reflow instantly when a neighbour changes.
//! - [`panel`]: how a widget's panel opens and closes (hover, click, right-click, drag, fade):
//!   one behaviour for every widget.
//! - [`draw`]: the shared drawing on a per-pixel-alpha strip (text on the clock's baselines,
//!   hover highlight, rounded shapes, progress ring).
//! - [`badge`]: a coloured, rounded badge with short text (e.g. a bus line number), sized to
//!   line up with the tray icons.
//! - [`anim`]: easing, and a slow pulse for "alive, nothing else to show".
//! - [`marquee`]: scrolling text for a line too long for its box.
//! - [`menu_panel`]: the right-click panel (a widget's menu in the panels' look, with switches).
//! - [`strip`]: how a strip moves in the dock (glide to its slot, drag to reorder, zoom while
//!   picked up), one behaviour for every widget.
//! - [`startup`]: the list of widgets started at sign-in (by the `taskbar-dock-start` launcher).
//! - [`icons`]: the taskbar's own buttons (UI Automation), so the stack never covers them.
//! - [`theme`]: the design code: every colour, font, size and spacing (light / dark).
//!
//! See README.md for how to build a new widget with it.

pub mod anim;
pub mod badge;
pub mod crash;
pub mod dock;
pub mod draw;
pub mod emoji;
pub mod icons;
pub mod marquee;
pub mod menu_panel;
pub mod panel;
pub mod settings;
pub mod startup;
pub mod strip;
pub mod style;
pub mod system;
pub mod theme;
pub mod window;

/// A UTF-16, NUL-terminated copy of `s` for Win32 calls.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
