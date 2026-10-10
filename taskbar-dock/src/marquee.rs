//! Scrolling text for a line too long for its box (e.g. a song title in a fixed-width panel),
//! the same in every widget:
//!
//! - Text that fits stays still.
//! - Longer text waits [`WAIT_S`] at the start, then scrolls left at [`SPEED`] px/s (at 100%) in a
//!   loop: its start comes round again [`GAP`] px after its end, and it keeps going until it's
//!   back where it began; then it waits again and repeats. No jump back.
//! - Soft fades at the box's edges where text is cut off ([`fade_edges`]), so letters aren't
//!   chopped.
//! - With Windows' animations off it doesn't move: the caller cuts it with "…" instead.
//!
//! Use: draw the line at `x - offset` and again at `x - offset + span`, both clipped to the box
//! (the second only while [`Marquee::span`] > 0), then call [`fade_edges`] with
//! [`Marquee::cut_left`] / [`Marquee::cut_right`]. Redraw ~30 times a second while
//! [`Marquee::moving`]; nothing to redraw while it waits.

/// Pause at the start, seconds.
pub const WAIT_S: f32 = 1.5;
/// Space between the end of the text and its start coming round again, px at 100%.
pub const GAP: f32 = 40.0;
/// Scrolling speed, px per second at 100%.
pub const SPEED: f32 = 30.0;
/// Width of the edge fades, px at 100%.
pub const FADE: f32 = 12.0;

/// Where a line of text is in its scroll at a moment.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Marquee {
    /// How far the text is moved left, px.
    pub offset: f32,
    /// How far it can go (text width − box width; 0 when it fits).
    pub overflow: f32,
    /// Distance from the text's start to its next copy (text width + gap); 0 when it fits.
    pub span: f32,
    /// Scrolling right now (not waiting, not still).
    pub moving: bool,
}

impl Marquee {
    /// Text `text_w` px wide in a box `box_w` px wide, `t` seconds after it was first shown,
    /// at display `scale`.
    pub fn at(text_w: f32, box_w: f32, t: f32, scale: f32) -> Marquee {
        let overflow = (text_w - box_w).max(0.0);
        if overflow <= 0.0 {
            return Marquee { offset: 0.0, overflow, span: 0.0, moving: false };
        }
        // one lap: the whole text and the gap go by, so it ends exactly where it started
        let span = text_w + GAP * scale;
        let travel = span / (SPEED * scale);
        let t = t.max(0.0) % (WAIT_S + travel);
        let moving = t > WAIT_S;
        let offset = if moving { (t - WAIT_S) / travel * span } else { 0.0 };
        Marquee { offset, overflow, span, moving }
    }

    /// Text is cut off on the left (scrolled away).
    pub fn cut_left(&self) -> bool {
        self.offset > 0.5
    }

    /// Text is cut off on the right (more to come): always, for text that doesn't fit.
    pub fn cut_right(&self) -> bool {
        self.overflow > 0.0
    }
}

/// Fades the pixels of `rect` (left, top, right, bottom) towards `bg` (0xRRGGBB) over `fade`
/// px at its left and/or right edge: for text drawn on an opaque background (a panel).
pub fn fade_edges(buf: &mut [u32], width: i32, rect: (i32, i32, i32, i32), fade: i32, left: bool, right: bool, bg: u32) {
    let (l, t, r, b) = rect;
    let height = buf.len() as i32 / width.max(1);
    if fade <= 0 {
        return;
    }
    for y in t.max(0)..b.min(height) {
        for i in 0..fade {
            // 0 at the very edge, nearly 1 at the fade's inner end
            let keep = (i as f32 + 0.5) / fade as f32;
            for (on, x) in [(left, l + i), (right, r - 1 - i)] {
                if !on || x < 0 || x >= width || x < l || x >= r {
                    continue;
                }
                let p = &mut buf[(y * width + x) as usize];
                let mix = |shift: u32| {
                    let (c, k) = (((*p >> shift) & 0xff) as f32, ((bg >> shift) & 0xff) as f32);
                    ((c * keep + k * (1.0 - keep)).round() as u32).min(255) << shift
                };
                *p = (*p & 0xff00_0000) | mix(16) | mix(8) | mix(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_fits_stays_still() {
        let m = Marquee::at(100.0, 120.0, 5.0, 1.0);
        assert_eq!(m.offset, 0.0);
        assert!(!m.moving && !m.cut_left() && !m.cut_right());
    }

    #[test]
    fn long_text_loops_round_to_where_it_began() {
        // 260 px of text + 40 px gap = a 300 px lap: 10 s at 30 px/s, after a 1.5 s wait
        let at = |t: f32| Marquee::at(260.0, 200.0, t, 1.0);
        assert_eq!(at(1.0).offset, 0.0); // waiting at the start
        assert!(!at(1.0).moving && at(1.0).cut_right() && !at(1.0).cut_left());
        assert_eq!(at(1.0).span, 300.0);
        assert!((at(6.5).offset - 150.0).abs() < 0.01 && at(6.5).moving); // half a lap
        assert!(at(6.5).cut_left());
        assert!(at(11.4).offset > 296.0 && at(11.4).moving); // nearly round
        // round: exactly where it began, waiting again, no jump
        assert_eq!(at(11.6).offset, 0.0);
        assert!(!at(11.6).moving);
        // at 150% it scrolls faster in px, so the same time on screen
        assert!((Marquee::at(350.0, 260.0, 2.5, 1.5).offset - 45.0).abs() < 0.01);
    }

    #[test]
    fn edges_fade_to_the_background() {
        let w = 40;
        let mut buf = vec![0xffff_ffffu32; (w * 2) as usize];
        fade_edges(&mut buf, w, (0, 0, w, 2), 10, true, false, 0x000000);
        assert!(buf[0] & 0xff < 20); // the left edge is nearly background
        assert!(buf[9] & 0xff > 230); // the fade's inner end is nearly text
        assert_eq!(buf[39], 0xffff_ffff); // no fade on the right
    }
}
