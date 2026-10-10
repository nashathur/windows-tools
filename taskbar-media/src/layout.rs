//! Where things go on the strip, and what the mouse is over.
//!
//! Full:    `| cover+ring | title / artist | ⏮ ⏯ ⏭ |`
//! Compact: `| cover+ring | ⏮ ⏯ ⏭ |`
//!
//! The cover spans the two text lines; the ring goes around it, 1 px away and 2 px wide, with
//! the same margin to the hover highlight on the left, top and bottom. Sizes are px at 100%,
//! scaled.

use taskbar_dock::draw;

pub const PAD_RIGHT: f32 = 2.0;
/// The ring around the cover: its gap to the cover and its width.
pub const RING_GAP: f32 = 1.0;
pub const RING_WIDTH: f32 = 2.0;
pub const TEXT_GAP: f32 = 6.0;
/// Each button's clickable width; the glyph is tray-icon sized in its middle.
pub const BUTTON: f32 = 24.0;
pub const BUTTONS: usize = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Strip {
    pub width: i32,
    /// The cover's box: left x and size (top comes from draw::icon_box).
    pub cover_x: i32,
    pub cover: i32,
    /// The ring's outer box: left x and size.
    pub ring_x: i32,
    pub ring: i32,
    /// Text column: left x and width (0 when compact).
    pub text_x: i32,
    pub text_w: i32,
    /// Buttons: left x of the first, and each one's width.
    pub buttons_x: i32,
    pub button_w: i32,
    /// The strip's height and scale as drawn (0 / 1.0 until then): a button is only hit inside
    /// its highlight box (`draw::button_box`), not the whole column.
    pub height: i32,
    pub scale: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// The text, or anywhere else on the strip itself.
    Body,
    /// The cover with its ring: a click opens the app's window.
    Cover,
    Button(usize),
}

fn px(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

/// The layout for text `text_w` px wide (the longer line, already capped at the maximum
/// width), at `scale` (display scale × drag zoom), for a `cover` px square whose ring starts
/// `left` px from the strip's left edge (the same margin as above and below it, inside the
/// hover highlight: see [`margin`]).
pub fn strip(scale: f32, cover: i32, left: i32, compact: bool, text_w: i32) -> Strip {
    let around = ring_around(scale);
    let ring = cover + 2 * around;
    let ring_x = left;
    let cover_x = ring_x + around;
    let (text_x, text_w) = if compact { (ring_x + ring, 0) } else { (ring_x + ring + px(TEXT_GAP, scale), text_w.max(0)) };
    let buttons_x = text_x + text_w + if compact { px(2.0, scale) } else { px(TEXT_GAP, scale) };
    let button_w = px(BUTTON, scale);
    Strip {
        width: buttons_x + BUTTONS as i32 * button_w + px(PAD_RIGHT, scale),
        cover_x,
        cover,
        ring_x,
        ring,
        text_x,
        text_w,
        buttons_x,
        button_w,
        height: 0,
        scale,
    }
}

/// The layout at any `width` between compact and full (`text_w`: the full text column): the
/// text column narrows first, then the gaps beside it close, so the cover stays on the left
/// and the buttons on the right. For the collapse/expand animation. Outside that range: the
/// full or the compact layout.
pub fn at_width(scale: f32, cover: i32, left: i32, text_w: i32, width: i32) -> Strip {
    let full = strip(scale, cover, left, false, text_w);
    let compact = strip(scale, cover, left, true, 0);
    if width >= full.width {
        return full;
    }
    if width <= compact.width {
        return compact;
    }
    let no_text = strip(scale, cover, left, false, 0);
    if width >= no_text.width {
        return strip(scale, cover, left, false, width - no_text.width);
    }
    Strip { width, buttons_x: compact.buttons_x + (width - compact.width), ..compact }
}

/// Px from the cover to the ring's outside, each side.
pub fn ring_around(scale: f32) -> i32 {
    px(RING_GAP + RING_WIDTH, scale)
}

/// The cover size and the even margin around its ring inside the hover highlight, which is
/// `highlight` px tall: the ring is centred in it and kept as far from its left edge as from
/// its top and bottom. A `wanted` cover that would leave an odd space is made 1 px smaller.
pub fn margin(wanted: i32, highlight: i32, scale: f32) -> (i32, i32) {
    let around = ring_around(scale);
    let mut cover = wanted;
    if (highlight - (cover + 2 * around)) % 2 != 0 {
        cover -= 1;
    }
    (cover, ((highlight - (cover + 2 * around)) / 2).max(0))
}

impl Strip {
    /// Left and right x of button `i`.
    pub fn button(&self, i: usize) -> (i32, i32) {
        let l = self.buttons_x + i as i32 * self.button_w;
        (l, l + self.button_w)
    }

    /// What's under strip point (`x`, `y`): a button only inside its highlight box (the glyph
    /// plus the shared margin, `draw::button_box`), the rest is the strip itself. Before the
    /// first draw (height 0) a button's whole column counts.
    pub fn hit(&self, x: i32, y: i32) -> Part {
        let button = (0..BUTTONS).find(|&i| {
            let col = self.button(i);
            if self.height == 0 {
                return x >= col.0 && x < col.1;
            }
            draw::contains(draw::button_box(col, self.height, self.scale), x, y)
        });
        if let Some(i) = button {
            return Part::Button(i);
        }
        // the cover with its ring (centred vertically, like the strip's content)
        let on_cover = self.height > 0 && draw::contains(draw::centered_box(self.ring_x, self.ring, self.height), x, y);
        if on_cover { Part::Cover } else { Part::Body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_strip_at_100_percent() {
        let s = strip(1.0, 16, 4, false, 120);
        // ring 22 px from x 4, cover inside it 3 px in
        assert_eq!((s.ring_x, s.ring, s.cover_x), (4, 22, 7));
        assert_eq!((s.text_x, s.text_w), (32, 120));
        assert_eq!(s.buttons_x, 158);
        assert_eq!(s.width, 158 + 72 + 2);
        assert_eq!(s.button(1), (182, 206));
    }

    #[test]
    fn compact_has_no_text() {
        let s = strip(1.0, 16, 4, true, 120);
        assert_eq!(s.text_w, 0);
        assert_eq!(s.buttons_x, 28);
        assert_eq!(s.width, 28 + 72 + 2);
    }

    #[test]
    fn in_between_widths_narrow_the_text_then_close_the_gaps() {
        // full 232, without text 112, compact 102 (100%, 16 px cover, 120 px text)
        let s = at_width(1.0, 16, 4, 120, 150);
        assert_eq!((s.width, s.text_x, s.text_w), (150, 32, 38));
        assert_eq!(s.buttons_x + 72 + 2, 150, "buttons stay at the right edge");
        let s = at_width(1.0, 16, 4, 120, 105);
        assert_eq!((s.width, s.text_w, s.buttons_x), (105, 0, 31));
        assert_eq!(at_width(1.0, 16, 4, 120, 400), strip(1.0, 16, 4, false, 120));
        assert_eq!(at_width(1.0, 16, 4, 120, 50), strip(1.0, 16, 4, true, 120));
    }

    #[test]
    fn scales_with_the_display() {
        let s = strip(1.5, 24, 6, false, 180);
        assert_eq!((s.ring, s.cover_x - s.ring_x), (34, 5));
        assert_eq!(s.button_w, 36);
    }

    #[test]
    fn the_ring_has_even_margins_in_the_highlight() {
        // 100%: a 40 px highlight (rows 4..44), a 26 px cover → 32 px ring, 4 px all round
        assert_eq!(margin(26, 40, 1.0), (26, 4));
        // a 27 px cover would leave 7 px: one less, for 4 and 4
        assert_eq!(margin(27, 40, 1.0), (26, 4));
        // 150%: a 60 px highlight, 5 px around the cover
        assert_eq!(margin(39, 60, 1.5), (38, 6));
    }

    #[test]
    fn hit_testing() {
        let mut s = strip(1.0, 16, 4, false, 120);
        // before the first draw: whole columns
        assert_eq!(s.hit(10, 24), Part::Body);
        assert_eq!(s.hit(157, 24), Part::Body);
        assert_eq!(s.hit(158, 0), Part::Button(0));
        assert_eq!(s.hit(229, 47), Part::Button(2));
        assert_eq!(s.hit(230, 24), Part::Body); // right padding
        // drawn on a 48 px taskbar: the box is 24 × 24, rows 12..36, as wide as the column
        s.height = 48;
        assert_eq!(s.hit(190, 24), Part::Button(1));
        assert_eq!(s.hit(190, 12), Part::Button(1));
        assert_eq!(s.hit(190, 11), Part::Body); // above the box
        assert_eq!(s.hit(190, 36), Part::Body); // below it
        assert_eq!(s.hit(157, 24), Part::Body);
        // the cover's ring: x 4..26, rows 13..35
        assert_eq!(s.hit(15, 24), Part::Cover);
        assert_eq!(s.hit(3, 24), Part::Body);
        assert_eq!(s.hit(15, 12), Part::Body);
        assert_eq!(s.hit(26, 24), Part::Body); // text column
    }
}
