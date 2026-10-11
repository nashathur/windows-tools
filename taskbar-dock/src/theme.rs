//! The design code: every colour, font, size and spacing a widget draws with comes from here
//! (values measured in `plan\tokens.md`). Widgets never write a colour, font name or font size
//! themselves; `scripts\dev.ps1` and the `widget_sources_follow_the_theme` test check that.
//!
//! - [`Theme`]: colours for light or dark (from [`Look::light`]) and the font families.
//! - [`brand`]: the allow-list of widget colours that aren't part of the theme (Discord's
//!   blurple, a bus line's fallback badge, ...), and the named overrides of widgets that keep
//!   a value of their own.
//! - [`TextStyle`] constants (`CAPTION` ... `DISPLAY`), [`space`] and [`size`]: px at 100%,
//!   multiply by the display scale and round.

use crate::style::Look;

/// A colour, 8 bits per channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// From 0xRRGGBB.
    pub const fn hex(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// GDI's COLORREF (0x00BBGGRR), for SetTextColor, brushes, pens.
    pub const fn colorref(self) -> u32 {
        self.0 as u32 | (self.1 as u32) << 8 | (self.2 as u32) << 16
    }

    /// 0xRRGGBB.
    pub const fn rgb24(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32
    }

    /// An opaque DIB pixel (0xFFRRGGBB).
    pub const fn pixel(self) -> u32 {
        0xff00_0000 | self.rgb24()
    }

    /// (r, g, b) as `draw` takes them.
    pub const fn tuple(self) -> (u32, u32, u32) {
        (self.0 as u32, self.1 as u32, self.2 as u32)
    }

    /// This colour moved `k` (0..1) of the way toward `to`.
    pub fn mix(self, to: Rgb, k: f32) -> Rgb {
        let k = k.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k).round() as u8;
        Rgb(m(self.0, to.0), m(self.1, to.1), m(self.2, to.2))
    }
}

/// Panel surface colours.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Colors {
    pub bg: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub faint: Rgb,
    /// Separators.
    pub line: Rgb,
    /// A row or glyph highlighted on `bg`.
    pub hover: Rgb,
    /// A filled button at rest, and hovered.
    pub button: Rgb,
    pub button_hover: Rgb,
    /// Bar, slider and switch background.
    pub track: Rgb,
    /// A missing cover.
    pub placeholder: Rgb,
    /// Count pill background and its text.
    pub chip: Rgb,
    pub chip_text: Rgb,
    /// Slider / switch on (= `fg`), switch off and its knob (= `dim`).
    pub fill: Rgb,
    pub knob: Rgb,
    /// Text on a coloured fill (`danger_fill`, a brand colour).
    pub on_fill: Rgb,
}

/// Strip colours (on the taskbar). Text is drawn as white coverage, then coloured.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StripColors {
    pub fg: Rgb,
    pub dim: Rgb,
    /// Coverage of a disabled glyph (of `fg`).
    pub disabled: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Status {
    /// The Windows accent colour (its text shade for this theme).
    pub accent: Rgb,
    pub danger: Rgb,
    /// A filled destructive button (white text), and hovered.
    pub danger_fill: Rgb,
    pub danger_fill_hover: Rgb,
    /// A switched-on destructive toggle's background.
    pub danger_soft: Rgb,
    pub ok: Rgb,
    pub warn: Rgb,
}

/// Font families: `text` for all text (the user's `look.font`), `icon`, `emoji`.
#[derive(Clone, PartialEq, Debug)]
pub struct Fonts {
    pub text: String,
    pub icon: &'static str,
    pub emoji: &'static str,
}

/// A text size (px at 100%) and weight.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextStyle {
    pub px: f32,
    pub weight: i32,
}

impl TextStyle {
    /// The size in px at `scale`, rounded (as the widgets' `s(v)`).
    pub fn px_at(self, scale: f32) -> i32 {
        (self.px * scale).round() as i32
    }
}

pub const CAPTION: TextStyle = TextStyle { px: 12.0, weight: 400 };
pub const CAPTION_STRONG: TextStyle = TextStyle { px: 12.0, weight: 600 };
pub const ROW: TextStyle = TextStyle { px: 13.0, weight: 400 };
pub const BODY: TextStyle = TextStyle { px: 14.0, weight: 400 };
pub const BODY_LARGE: TextStyle = TextStyle { px: 15.0, weight: 400 };
pub const TITLE_SMALL: TextStyle = TextStyle { px: 14.0, weight: 600 };
pub const TITLE: TextStyle = TextStyle { px: 15.0, weight: 600 };
pub const DISPLAY: TextStyle = TextStyle { px: 46.0, weight: 300 };
pub const FOOTNOTE: TextStyle = TextStyle { px: 11.0, weight: 400 };
/// A badge's text in a panel header (the bus line number), bold.
pub const BADGE: TextStyle = TextStyle { px: 18.0, weight: 700 };
/// Panel glyphs (icon font) and media's transport buttons.
pub const ICON: TextStyle = TextStyle { px: 16.0, weight: 400 };
pub const ICON_LARGE: TextStyle = TextStyle { px: 20.0, weight: 400 };
pub const EMOJI: TextStyle = TextStyle { px: 20.0, weight: 400 };
pub const EMOJI_LARGE: TextStyle = TextStyle { px: 42.0, weight: 400 };
/// The icon and emoji font families (also in [`Fonts`]).
pub const ICON_FONT: &str = "Segoe Fluent Icons";
pub const EMOJI_FONT: &str = "Segoe UI Emoji";
/// A strip glyph is drawn at this size, then fitted into the tray-icon box.
pub const GLYPH_RASTER_PX: i32 = 64;

/// Spacing, px at 100% (4 px grid; `TIGHT` is the universal icon-text gap).
pub mod space {
    pub const XS: f32 = 4.0;
    pub const TIGHT: f32 = 6.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 20.0;
    /// Inside a main (left-click / hover) panel, and a menu panel.
    pub const PANEL_PADDING: f32 = L;
    pub const MENU_PADDING: f32 = M;
    pub const ROW_HEIGHT: f32 = 32.0;
    pub const ROW_HEIGHT_LARGE: f32 = 40.0;
    /// Between menu sections: 4 + 1 px line + 4.
    pub const SECTION_GAP: f32 = 8.0;
}

/// Sizes, px at 100%.
pub mod size {
    pub const PANEL_NARROW: f32 = 280.0;
    pub const PANEL_STANDARD: f32 = 320.0;
    pub const PANEL_WIDE: f32 = 360.0;
    pub const ICON_BOX: f32 = crate::style::TRAY_ICON;
    pub const COVER_SMALL: f32 = 44.0;
}

/// Windows' default accent text shades (blue), used when the registry has none.
const ACCENT_FALLBACK: (Rgb, Rgb) = (Rgb::hex(0x0067C0), Rgb::hex(0x60CDFF));

/// Opacity (0..1) of a brand colour behind a strip ([`crate::block::Tint`]), three levels shared
/// by every widget with one (discord, media): `quiet` while paused / muted, `active` while
/// playing / unmuted, `hover` under the mouse.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TintLevels {
    pub quiet: f32,
    pub active: f32,
    pub hover: f32,
}

/// The three tint levels by default (the `dock` section's `tint_dark` / `tint_light` change them:
/// [`crate::style`]): a light taskbar needs a little less (the colour shows more on it).
pub fn tint_levels(light: bool) -> TintLevels {
    // in 255ths: active and hover are discord's blurple boxes before the three levels
    let (quiet, active, hover) = if light { (38.0, 70.0, 105.0) } else { (46.0, 85.0, 125.0) };
    TintLevels { quiet: quiet / 255.0, active: active / 255.0, hover: hover / 255.0 }
}

/// Everything a widget colours and sets text with, for light or dark.
#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub light: bool,
    pub color: Colors,
    pub strip: StripColors,
    pub status: Status,
    pub font: Fonts,
    /// Brand tint strengths behind a strip: the look's (settings), else [`tint_levels`].
    pub tint: TintLevels,
}

impl Theme {
    /// The theme for `look` (light/dark, font), with the live Windows accent colour.
    pub fn new(look: &Look) -> Theme {
        Theme { tint: look.tint, ..Theme::with(look.light, &look.font, windows_accent(look.light)) }
    }

    /// The theme for light or dark, font family `text` and accent (None: Windows' default blue).
    pub fn with(light: bool, text: &str, accent: Option<Rgb>) -> Theme {
        let l = light;
        let pick = |lv: u32, dv: u32| Rgb::hex(if l { lv } else { dv });
        let fg = pick(0x1A1A1A, 0xFFFFFF);
        let dim = pick(0x5F5F5F, 0xA8A8A8);
        let color = Colors {
            bg: pick(0xF3F3F3, 0x202020),
            fg,
            dim,
            faint: pick(0x8A8A8A, 0x808080),
            line: pick(0xE0E0E0, 0x3A3A3A),
            hover: pick(0xE4E4E4, 0x333333),
            button: pick(0xE6E6E6, 0x2D2D2D),
            button_hover: pick(0xDADADA, 0x383838),
            track: pick(0xD6D6D6, 0x454545),
            placeholder: pick(0xDEDEDE, 0x383838),
            chip: pick(0xD0D0D0, 0x3C3C3C),
            chip_text: fg,
            fill: fg,
            knob: dim,
            on_fill: Rgb::hex(0xFFFFFF),
        };
        let strip = StripColors { fg: pick(0x1A1A1A, 0xFFFFFF), dim: pick(0x5A5A5A, 0xC8C8C8), disabled: 96.0 / 255.0 };
        let fallback = if l { ACCENT_FALLBACK.0 } else { ACCENT_FALLBACK.1 };
        let status = Status {
            accent: accent.unwrap_or(fallback),
            danger: pick(0xC42B1C, 0xFF99A4),
            danger_fill: Rgb::hex(0xC43135),
            danger_fill_hover: Rgb::hex(0xD83C3E),
            danger_soft: pick(0xF6D6D7, 0x4A2426),
            ok: pick(0x0F7B0F, 0x6CCB5F),
            warn: pick(0x9D5D00, 0xFCE100),
        };
        Theme { light, color, strip, status, font: Fonts { text: text.to_string(), icon: ICON_FONT, emoji: EMOJI_FONT }, tint: tint_levels(light) }
    }

    /// The allow-listed widget colour `name` for this theme (see [`brand`]).
    pub fn brand(&self, name: &str) -> Rgb {
        brand(name, self.light)
    }
}

/// The allow-list: (name, light, dark). Brand and data colours, then named overrides (a widget
/// keeping its own value instead of a theme token; remove one when the widget adopts the token).
pub const BRANDS: &[(&str, u32, u32)] = &[
    ("discord.blurple", 0x5865F2, 0x5865F2),
    ("discord.green", 0x23A55A, 0x23A55A),
    ("discord.red", 0xF23F43, 0xF23F43),
    ("bus.badge_fallback", 0x00A8A8, 0x00A8A8),
    ("media.live", 0xFF4E45, 0xFF4E45),
    ("weather.temp_bar", 0xE8892B, 0xFFB454),
    // overrides
    // rain chance in Windows' default accent blue (the live accent would change it)
    ("weather.rain", 0x0067C0, 0x60CDFF),
    // the attention dot on the strip (`warn` is a different yellow)
    ("discord.attention", 0xF0A030, 0xF0A030),
];

/// Named font overrides (see [`BRANDS`]): text that keeps "Segoe UI" instead of `look.font`.
pub mod font_override {
    /// The bus "Change stop…" picker (a Win32 dialog until step 11).
    pub const BUS_PICKER: &str = "Segoe UI";
}

/// Brand colour `name` from [`BRANDS`] for light or dark. An unknown name is a bug: it panics
/// in debug builds and gives magenta in release, so it shows.
pub fn brand(name: &str, light: bool) -> Rgb {
    match BRANDS.iter().find(|b| b.0 == name) {
        Some(&(_, l, d)) => Rgb::hex(if light { l } else { d }),
        None => {
            debug_assert!(false, "theme::brand: unknown name {name}");
            Rgb::hex(0xFF00FF)
        }
    }
}

/// The accent colour's text shade from Windows (AccentPalette: light 3..1, base, dark 1..3):
/// dark 1 on light, light 2 on dark. None if Windows has none.
fn windows_accent(light: bool) -> Option<Rgb> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut buf = [0u8; 32];
    let mut sz = buf.len() as u32;
    let (k, n) = (crate::wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"), crate::wide("AccentPalette"));
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_BINARY, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut sz)
    };
    if r != 0 || sz < 32 {
        return None;
    }
    let i = if light { 4 } else { 1 } * 4;
    Some(Rgb(buf[i], buf[i + 1], buf[i + 2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_has_three_rising_levels_and_discords_strengths_stay() {
        for light in [true, false] {
            let t = tint_levels(light);
            assert!(0.0 < t.quiet && t.quiet < t.active && t.active < t.hover && t.hover < 1.0);
            assert_eq!(Theme::with(light, "x", None).tint, t);
        }
        // discord's boxes before the three levels (2026-10-11): 85 / 125 dark, 70 / 105 light
        let (d, l) = (tint_levels(false), tint_levels(true));
        assert_eq!(((d.active * 255.0).round(), (d.hover * 255.0).round()), (85.0, 125.0));
        assert_eq!(((l.active * 255.0).round(), (l.hover * 255.0).round()), (70.0, 105.0));
    }

    fn light() -> Theme {
        Theme::with(true, "Segoe UI Variable Text", None)
    }
    fn dark() -> Theme {
        Theme::with(false, "Segoe UI Variable Text", None)
    }

    #[test]
    fn rgb_conversions() {
        let c = Rgb::hex(0x123456);
        assert_eq!(c, Rgb(0x12, 0x34, 0x56));
        assert_eq!(c.colorref(), 0x563412);
        assert_eq!(c.rgb24(), 0x123456);
        assert_eq!(c.pixel(), 0xff12_3456);
        assert_eq!(c.tuple(), (0x12, 0x34, 0x56));
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(200, 100, 50), 0.5), Rgb(100, 50, 25));
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(200, 100, 50), 2.0), Rgb(200, 100, 50));
    }

    #[test]
    fn panel_colours_light_and_dark() {
        let (l, d) = (light().color, dark().color);
        let pairs = [
            (l.bg, d.bg, 0xF3F3F3, 0x202020),
            (l.fg, d.fg, 0x1A1A1A, 0xFFFFFF),
            (l.dim, d.dim, 0x5F5F5F, 0xA8A8A8),
            (l.faint, d.faint, 0x8A8A8A, 0x808080),
            (l.line, d.line, 0xE0E0E0, 0x3A3A3A),
            (l.hover, d.hover, 0xE4E4E4, 0x333333),
            (l.button, d.button, 0xE6E6E6, 0x2D2D2D),
            (l.button_hover, d.button_hover, 0xDADADA, 0x383838),
            (l.track, d.track, 0xD6D6D6, 0x454545),
            (l.placeholder, d.placeholder, 0xDEDEDE, 0x383838),
            (l.chip, d.chip, 0xD0D0D0, 0x3C3C3C),
            (l.chip_text, d.chip_text, 0x1A1A1A, 0xFFFFFF),
            (l.on_fill, d.on_fill, 0xFFFFFF, 0xFFFFFF),
        ];
        for (i, (a, b, x, y)) in pairs.into_iter().enumerate() {
            assert_eq!((a.rgb24(), b.rgb24()), (x, y), "token {i}");
        }
        assert_eq!((l.fill, l.knob), (l.fg, l.dim));
        assert_eq!((d.fill, d.knob), (d.fg, d.dim));
    }

    #[test]
    fn strip_colours_match_look_text_rgb() {
        for t in [light(), dark()] {
            let look = Look { light: t.light, ..crate::style::Style::default().resolve(&crate::system::read()) };
            let (fg, dim) = look.text_rgb();
            assert_eq!((t.strip.fg.tuple(), t.strip.dim.tuple()), (fg, dim));
        }
        assert_eq!((light().strip.disabled * 255.0).round(), 96.0);
    }

    #[test]
    fn status_colours() {
        let (l, d) = (light().status, dark().status);
        assert_eq!((l.accent.rgb24(), d.accent.rgb24()), (0x0067C0, 0x60CDFF));
        assert_eq!((l.danger.rgb24(), d.danger.rgb24()), (0xC42B1C, 0xFF99A4));
        assert_eq!((l.danger_fill, d.danger_fill), (Rgb::hex(0xC43135), Rgb::hex(0xC43135)));
        assert_eq!(l.danger_fill_hover, Rgb::hex(0xD83C3E));
        assert_eq!((l.danger_soft.rgb24(), d.danger_soft.rgb24()), (0xF6D6D7, 0x4A2426));
        assert_eq!((l.ok.rgb24(), d.ok.rgb24()), (0x0F7B0F, 0x6CCB5F));
        assert_eq!((l.warn.rgb24(), d.warn.rgb24()), (0x9D5D00, 0xFCE100));
        let green = Rgb::hex(0x366B02);
        assert_eq!(Theme::with(true, "x", Some(green)).status.accent, green);
    }

    #[test]
    fn brands_and_overrides() {
        assert_eq!(brand("discord.blurple", true), Rgb::hex(0x5865F2));
        assert_eq!(brand("discord.green", false), Rgb::hex(0x23A55A));
        assert_eq!(brand("discord.red", true), Rgb::hex(0xF23F43));
        assert_eq!(brand("bus.badge_fallback", true), Rgb::hex(0x00A8A8));
        assert_eq!(brand("media.live", false), Rgb::hex(0xFF4E45));
        assert_eq!((brand("weather.temp_bar", true), brand("weather.temp_bar", false)), (Rgb::hex(0xE8892B), Rgb::hex(0xFFB454)));
        assert_eq!(light().brand("weather.rain"), Rgb::hex(0x0067C0));
        assert_eq!(dark().brand("weather.rain"), Rgb::hex(0x60CDFF));
        let mut names: Vec<_> = BRANDS.iter().map(|b| b.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), BRANDS.len(), "duplicate brand name");
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn unknown_brand_shows() {
        assert_eq!(brand("nope", true), Rgb::hex(0xFF00FF));
    }

    #[test]
    fn type_tokens() {
        let t = light();
        assert_eq!(t.font.text, "Segoe UI Variable Text");
        assert_eq!((t.font.icon, t.font.emoji), ("Segoe Fluent Icons", "Segoe UI Emoji"));
        let sizes = [
            (CAPTION, 12.0, 400), (CAPTION_STRONG, 12.0, 600), (ROW, 13.0, 400), (BODY, 14.0, 400),
            (BODY_LARGE, 15.0, 400), (TITLE_SMALL, 14.0, 600), (TITLE, 15.0, 600), (DISPLAY, 46.0, 300),
            (FOOTNOTE, 11.0, 400), (BADGE, 18.0, 700), (ICON, 16.0, 400), (ICON_LARGE, 20.0, 400), (EMOJI, 20.0, 400), (EMOJI_LARGE, 42.0, 400),
        ];
        for (s, px, w) in sizes {
            assert_eq!((s.px, s.weight), (px, w));
        }
        assert_eq!(TITLE.px_at(1.5), 23);
        assert_eq!(CAPTION.px_at(1.25), 15);
    }

    #[test]
    fn spacing_and_sizes() {
        use space::*;
        assert_eq!([XS, TIGHT, S, M, L, XL], [4.0, 6.0, 8.0, 12.0, 16.0, 20.0]);
        assert_eq!((PANEL_PADDING, MENU_PADDING, ROW_HEIGHT, ROW_HEIGHT_LARGE, SECTION_GAP), (16.0, 12.0, 32.0, 40.0, 8.0));
        use size::*;
        assert_eq!((PANEL_NARROW, PANEL_STANDARD, PANEL_WIDE), (280.0, 320.0, 360.0));
        assert_eq!((ICON_BOX, COVER_SMALL), (16.0, 44.0));
    }

    #[test]
    fn theme_follows_look() {
        let mut look = crate::style::Style::default().resolve(&crate::system::read());
        look.light = true;
        assert!(Theme::new(&look).light);
        assert_eq!(Theme::new(&look).color.bg, Rgb::hex(0xF3F3F3));
        look.light = false;
        assert_eq!(Theme::new(&look).color.bg, Rgb::hex(0x202020));
        assert_eq!(Theme::new(&look).font.text, look.font);
    }
}
