#![windows_subsystem = "windows"]

//! Minimal Block widget starter: a timer-driven counter with the standard five parts.

use std::ffi::c_void;
use taskbar_dock::block::{
    self, Canvas, CommandError, CommandReply, Content, Ctx, Frame, MenuBuilder, PanelFrame,
    PanelKind, ReplyToken, Spec,
};
use taskbar_dock::describe::{Kind, Setting};
use taskbar_dock::style::Look;
use taskbar_dock::{draw, settings, theme};
use windows::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, DrawTextW, GdiFlush, GetDC, GetTextExtentPoint32W,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, ANTIALIASED_QUALITY, CLEARTYPE_QUALITY,
    DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FONT_QUALITY, HDC, HFONT, HGDIOBJ, LOGFONTW,
    TRANSPARENT,
};

const TIMER_COUNTER: usize = 1;
const MENU_RESET: usize = 1;

fn log_file() -> std::path::PathBuf {
    taskbar_dock::log::path("__NAME__")
}

fn log(line: &str) {
    taskbar_dock::log::append(&log_file(), line);
}

fn setting_descriptions() -> Vec<Setting> {
    vec![Setting::new("show_seconds", "Show seconds", Kind::Switch).group("Display")]
}

fn defaults() -> serde_json::Value {
    serde_json::json!({ "show_seconds": true })
}

/// Describes this widget without starting its runtime.
fn describe() -> block::Describe {
    block::describe(
        "__SECTION__",
        "__CLASS__",
        "__TITLE__",
        "__DESC__",
        Some(setting_descriptions),
        Some(defaults),
        Some(log_file()),
    )
}

#[derive(Default)]
struct App {
    counter: u64,
    show_seconds: bool,
}

impl App {
    fn reload_settings(&mut self) {
        self.show_seconds = settings::section("__SECTION__")
            .and_then(|s| s.get("show_seconds").and_then(serde_json::Value::as_bool))
            .unwrap_or(true);
    }

    fn counter_text(&self) -> String {
        if self.show_seconds {
            format!("{}s", self.counter)
        } else {
            self.counter.to_string()
        }
    }
}

impl Content for App {
    fn commands(&self) -> Vec<(&'static str, &'static str)> {
        vec![("now", "now: return the current counter as JSON")]
    }

    fn command(
        &mut self,
        name: &str,
        args: &[String],
        token: ReplyToken,
        _ctx: &mut Ctx,
    ) -> CommandReply {
        if name == "now" && args.is_empty() {
            drop(token);
            return CommandReply::Done(Ok(serde_json::json!({ "counter": self.counter })));
        }
        drop(token);
        CommandReply::Done(Err(CommandError {
            usage: true,
            message: if name == "now" {
                "now takes no arguments"
            } else {
                "unknown command"
            }
            .into(),
        }))
    }

    fn measure(&mut self, frame: &Frame, compact: bool) -> i32 {
        // the taskbar clock's font, as every strip (README "Building a new widget" step 4)
        let font = Font::new(strip_text(frame.look), &frame.look.font, CLEARTYPE_QUALITY, frame.scale);
        let text = if compact {
            self.counter_text()
        } else {
            format!("__TITLE__  {}", self.counter_text())
        };
        let hwnd = HWND(0 as *mut c_void);
        let dc = unsafe { GetDC(hwnd) };
        let width = text_width(dc, &font, &text);
        unsafe {
            let _ = ReleaseDC(hwnd, dc);
        }
        width + (theme::space::M * frame.scale).round() as i32 * 2
    }

    fn draw(&mut self, cv: &mut Canvas, frame: &Frame) {
        let palette = theme::Theme::new(frame.look);
        let font = Font::new(strip_text(frame.look), &frame.look.font, CLEARTYPE_QUALITY, frame.scale);
        let text = if frame.compact {
            self.counter_text()
        } else {
            format!("__TITLE__  {}", self.counter_text())
        };
        let color = rgb(255, 255, 255); // white is GDI coverage; Block applies the theme's strip ink below
        draw_text(
            HDC(cv.dc as *mut c_void),
            &font,
            &text,
            color,
            0,
            0,
            cv.w,
            cv.h,
            DT_CENTER | DT_VCENTER,
        );
        unsafe {
            let _ = GdiFlush();
        }
        let ink = palette.strip.fg.tuple();
        let (w, h) = (cv.w, cv.h);
        draw::text_to_color(cv.pixels(), w, h, |_| ink);
    }

    fn tint(&self, _area: block::Hit, _look: &Look) -> Option<block::Tint> {
        None
    }

    fn start(&mut self, ctx: &mut Ctx) {
        self.reload_settings();
        ctx.every(TIMER_COUNTER, 1_000);
    }

    /// Nothing to pause: this widget runs only Block content timers, which Block pauses itself.
    fn idle(&mut self, _idle: bool, _ctx: &mut Ctx) {}

    fn timer(&mut self, id: usize, ctx: &mut Ctx) {
        if id == TIMER_COUNTER {
            self.counter = self.counter.wrapping_add(1);
            ctx.redraw();
            ctx.redraw_panel();
        }
    }

    fn settings_changed(&mut self, ctx: &mut Ctx) {
        self.reload_settings();
        ctx.redraw();
        ctx.redraw_panel();
    }

    fn menu(&mut self, menu: &mut MenuBuilder) {
        menu.item(taskbar_dock::menu_panel::Row::action(
            MENU_RESET,
            '\u{E777}',
            "Reset counter",
        ));
    }

    fn menu_action(&mut self, id: usize, ctx: &mut Ctx) {
        if id == MENU_RESET {
            self.counter = 0;
            ctx.redraw();
            ctx.redraw_panel();
        }
    }

    fn panel_size(&mut self, kind: PanelKind, scale: f32) -> Option<(i32, i32)> {
        (kind == PanelKind::Main).then(|| {
            (
                (theme::size::PANEL_NARROW * scale).round() as i32,
                ((theme::space::PANEL_PADDING * 2.0 + theme::space::ROW_HEIGHT * 2.0) * scale)
                    .round() as i32,
            )
        })
    }

    fn panel_draw(&mut self, _kind: PanelKind, cv: &mut Canvas, frame: &PanelFrame) {
        let palette = frame.theme();
        let pad = (theme::space::PANEL_PADDING * frame.scale).round() as i32;
        let row = (theme::space::ROW_HEIGHT * frame.scale).round() as i32;
        let title_font = Font::new(
            theme::TITLE,
            &palette.font.text,
            CLEARTYPE_QUALITY,
            frame.scale,
        );
        let body_font = Font::new(
            theme::BODY,
            &palette.font.text,
            CLEARTYPE_QUALITY,
            frame.scale,
        );
        let dc = HDC(cv.dc as *mut c_void);
        draw_text(
            dc,
            &title_font,
            "__TITLE__",
            colorref(palette.color.fg),
            pad,
            pad,
            cv.w - 2 * pad,
            row,
            DT_SINGLELINE,
        );
        draw_text(
            dc,
            &body_font,
            &format!("Counter: {}", self.counter_text()),
            colorref(palette.color.dim),
            pad,
            pad + row,
            cv.w - 2 * pad,
            row,
            DT_SINGLELINE,
        );
        unsafe {
            let _ = GdiFlush();
        }
    }

    fn tray_icon(&mut self, cv: &mut Canvas, look: &Look) -> bool {
        let size = theme::GLYPH_RASTER_PX;
        let Some(mut glyph) = Canvas::new(size, size) else {
            return false;
        };
        let palette = theme::Theme::new(look);
        let font = Font::new(theme::ICON, palette.font.icon, ANTIALIASED_QUALITY, 1.0);
        draw_text(
            HDC(glyph.dc as *mut c_void),
            &font,
            "\u{25CF}",
            rgb(255, 255, 255),
            0,
            0,
            size,
            size,
            DT_CENTER | DT_VCENTER,
        );
        unsafe {
            let _ = GdiFlush();
        }
        let (w, h) = (cv.w, cv.h);
        let (top, box_size) = draw::icon_box(h, 1.0);
        draw::fit_icon(cv.pixels(), w, 0, top, box_size, glyph.pixels(), size, size);
        let ((r, g, b), _) = look.text_rgb();
        draw::text_to_color(cv.pixels(), w, h, |_| (r, g, b));
        true
    }

    fn tray_tip(&self) -> String {
        format!("__TITLE__: {}", self.counter_text())
    }
}

/// Strip text: the taskbar clock's size, regular weight. A strip whose text changes length should
/// measure its longest text, so the width doesn't jump with every update.
fn strip_text(look: &Look) -> theme::TextStyle {
    theme::TextStyle { px: look.font_size, weight: 400 }
}

struct Font(HFONT);

impl Font {
    fn new(style: theme::TextStyle, face: &str, quality: FONT_QUALITY, scale: f32) -> Self {
        let mut lf = LOGFONTW {
            lfHeight: -style.px_at(scale).max(1),
            lfWeight: style.weight,
            lfQuality: quality,
            ..Default::default()
        };
        for (dst, src) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
            *dst = src;
        }
        Self(unsafe { CreateFontIndirectW(&lf) })
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 .0));
        }
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn colorref(c: theme::Rgb) -> COLORREF {
    let (r, g, b) = c.tuple();
    rgb(r as u8, g as u8, b as u8)
}

fn text_width(dc: HDC, font: &Font, text: &str) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(font.0 .0));
        let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
        SelectObject(dc, old);
    }
    size.cx
}

fn draw_text(
    dc: HDC,
    font: &Font,
    text: &str,
    color: COLORREF,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(font.0 .0));
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, color);
        let _ = DrawTextW(
            dc,
            &mut wide,
            &mut rect,
            flags | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(dc, old);
    }
}

fn main() {
    block::describe_if_asked(describe);
    let mut spec = Spec::new("__CLASS__", "__SECTION__", "__TITLE__", log);
    spec.description = "__DESC__";
    spec.settings = Some(setting_descriptions);
    spec.defaults = Some(defaults);
    spec.log_file = Some(log_file());
    block::run(spec, App::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_matches_widget_constants() {
        let description = describe();
        assert_eq!(description.section, "__SECTION__");
        assert_eq!(description.class, "__CLASS__");
        taskbar_dock::describe::validate(&description).unwrap();
    }
}
