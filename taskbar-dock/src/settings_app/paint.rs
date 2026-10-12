//! Double-buffer compatible GDI painting for the settings application.

use crate::settings_app::{
    layout,
    model::{Page, Row, WidgetRow},
};
use std::collections::HashMap;
use taskbar_dock::theme::{self, Rgb, Theme};
use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;

thread_local! {
    static ACTIVE_FONTS: std::cell::Cell<([HFONT; 5], [i32; 5])> =
        const { std::cell::Cell::new(([0; 5], [0; 5])) };
}

/// The settings app's five shared GDI fonts.
pub struct Fonts {
    handles: [HFONT; 5],
    sizes: [i32; 5],
}

impl Fonts {
    /// Builds label, bold label, help, heading, and small fonts for a theme and scale.
    pub fn new(theme: &Theme, scale: f32) -> Self {
        let sizes = [
            theme::ROW.px_at(scale),
            theme::TITLE_SMALL.px_at(scale),
            theme::CAPTION.px_at(scale),
            theme::TITLE.px_at(scale),
            theme::FOOTNOTE.px_at(scale),
        ];
        let family: Vec<u16> = theme.font.text.encode_utf16().chain(Some(0)).collect();
        let handles = std::array::from_fn(|index| unsafe {
            CreateFontW(
                -sizes[index],
                0,
                0,
                0,
                if index == 1 || index == 3 { 600 } else { 400 },
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                DEFAULT_PITCH as u32,
                family.as_ptr(),
            )
        });
        Self { handles, sizes }
    }

    /// Measures wrapped text with the exact shared help or small font.
    pub fn measure_height(&self, text: &str, width: i32, small: bool) -> i32 {
        if text.is_empty() || width <= 0 {
            return 0;
        }
        unsafe {
            let dc = GetDC(0);
            let font = self.handles[if small { 4 } else { 2 }];
            let old = SelectObject(dc, font);
            let wide: Vec<u16> = text.encode_utf16().collect();
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: 0,
            };
            DrawTextW(
                dc,
                wide.as_ptr(),
                wide.len() as i32,
                &mut rect,
                DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
            );
            SelectObject(dc, old);
            ReleaseDC(0, dc);
            (rect.bottom - rect.top).max(self.sizes[if small { 4 } else { 2 }])
        }
    }

    /// Measures one string with the cached label font.
    pub fn measure_width(&self, text: &str) -> i32 {
        if text.is_empty() {
            return 0;
        }
        unsafe {
            let dc = GetDC(0);
            let old = SelectObject(dc, self.handles[0]);
            let wide: Vec<u16> = text.encode_utf16().collect();
            let mut size: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut size);
            SelectObject(dc, old);
            ReleaseDC(0, dc);
            size.cx
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in self.handles {
            if font != 0 {
                unsafe {
                    DeleteObject(font);
                }
            }
        }
    }
}

/// Data required to paint one settings window frame.
pub struct Frame<'a> {
    /// Current page.
    pub page: &'a Page,
    /// Current color and font theme.
    pub theme: &'a Theme,
    /// Fonts built once for the current theme and DPI scale.
    pub fonts: &'a Fonts,
    /// Display scaling factor.
    pub scale: f32,
    /// Navigation labels and selected entry.
    pub nav: &'a [(String, Page)],
    /// Widget overview rows.
    pub widgets: &'a [WidgetRow],
    /// In-flight process actions by widget name.
    pub busy_actions: &'a HashMap<String, String>,
    /// Current settings rows.
    pub rows: &'a [Row],
    /// Active text editor and its caret/selection state.
    pub edit: Option<(usize, &'a taskbar_dock::text_edit::TextEdit)>,
    /// Card heights measured with the current GDI fonts.
    pub card_heights: &'a [i32],
    /// Error block heights measured with the current GDI fonts.
    pub error_heights: &'a [i32],
    /// Error explaining why the selected widget has no settings rows.
    pub page_error: Option<&'a str>,
    /// Current top-right save badge.
    pub badge: Option<&'a str>,
    /// File error banner text, when present.
    pub banner: Option<&'a str>,
    /// Current scroll offset.
    pub scroll: i32,
    /// Focused control row, if any.
    pub focus: Option<usize>,
    /// Row whose Reset button has keyboard focus.
    pub focus_reset: Option<usize>,
    /// Focused navigation entry, if any.
    pub focus_nav: Option<usize>,
    /// Focused home widget and control kind (show, primary, restart).
    pub focus_home: Option<(usize, u8)>,
    /// Open dropdown row and highlighted option.
    pub dropdown: Option<(usize, usize)>,
    /// Whether a secret is temporarily revealed while its eye is held.
    pub reveal_secret: bool,
    /// Pointer position used for hover highlights.
    pub hover: Option<(i32, i32)>,
    /// Client height used to keep dropdown lists within the window.
    pub height: i32,
}

/// Paints a complete opaque settings client area into the supplied memory DC.
pub fn draw(dc: HDC, width: i32, height: i32, frame: &Frame<'_>) {
    ACTIVE_FONTS.with(|fonts| fonts.set((frame.fonts.handles, frame.fonts.sizes)));
    let theme = frame.theme;
    fill(
        dc,
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
        theme.color.bg,
    );
    let nav_width = layout::px(theme::app::NAV_WIDTH, frame.scale).min(width / 2);
    fill(
        dc,
        RECT {
            left: 0,
            top: 0,
            right: nav_width,
            bottom: height,
        },
        theme.color.input,
    );
    let mut y = layout::px(theme::app::CONTENT_PADDING, frame.scale);
    for (index, (label, page)) in frame.nav.iter().enumerate() {
        let h = layout::px(theme::app::NAV_ROW_HEIGHT, frame.scale);
        if matches!(page, Page::Developer) {
            y = height - h - layout::px(theme::app::CONTENT_PADDING, frame.scale);
        }
        if page == frame.page {
            fill(
                dc,
                RECT {
                    left: layout::px(theme::space::S, frame.scale),
                    top: y,
                    right: nav_width - layout::px(theme::space::S, frame.scale),
                    bottom: y + h,
                },
                theme.color.hover,
            );
        }
        text(
            dc,
            label,
            RECT {
                left: layout::px(theme::space::L, frame.scale),
                top: y,
                right: nav_width - layout::px(theme::space::L, frame.scale),
                bottom: y + h,
            },
            theme.color.fg,
            theme,
            theme::ROW.px_at(frame.scale),
            false,
        );
        if frame.focus_nav == Some(index) {
            outline(
                dc,
                RECT {
                    left: layout::px(theme::space::S, frame.scale),
                    top: y,
                    right: nav_width - layout::px(theme::space::S, frame.scale),
                    bottom: y + h,
                },
                theme.status.accent,
                frame.scale,
            );
        }
        if !matches!(page, Page::Developer) {
            y += h;
        }
    }
    let content_left = nav_width + layout::px(theme::app::CONTENT_PADDING, frame.scale);
    let content_width = width - content_left - layout::px(theme::app::CONTENT_PADDING, frame.scale);
    text(
        dc,
        page_title(frame.page, frame.nav),
        RECT {
            left: content_left,
            top: layout::px(theme::space::L, frame.scale),
            right: content_left + content_width,
            bottom: layout::px(theme::app::HEADER_HEIGHT, frame.scale),
        },
        theme.color.fg,
        theme,
        theme::TITLE.px_at(frame.scale),
        true,
    );
    if let Some(badge) = frame.badge {
        let badge_width = layout::px(theme::app::CONTROL_WIDTH, frame.scale);
        text(
            dc,
            badge,
            RECT {
                left: width - badge_width - layout::px(theme::space::L, frame.scale),
                top: layout::px(theme::space::L, frame.scale),
                right: width - layout::px(theme::space::L, frame.scale),
                bottom: layout::px(theme::app::HEADER_HEIGHT, frame.scale),
            },
            theme.color.dim,
            theme,
            theme::CAPTION.px_at(frame.scale),
            false,
        );
    }
    let mut cy = layout::px(theme::app::HEADER_HEIGHT, frame.scale);
    if let Some(banner) = frame.banner {
        let h = layout::px(theme::space::XL, frame.scale) * 2;
        fill(
            dc,
            RECT {
                left: content_left,
                top: cy,
                right: width - layout::px(theme::space::L, frame.scale),
                bottom: cy + h,
            },
            theme.status.danger_soft,
        );
        text(
            dc,
            banner,
            RECT {
                left: content_left + layout::px(theme::space::S, frame.scale),
                top: cy,
                right: width
                    - layout::px(
                        theme::app::NOTEPAD_BUTTON_WIDTH + theme::space::L * 2.0,
                        frame.scale,
                    ),
                bottom: cy + h,
            },
            theme.status.danger,
            theme,
            theme::ROW.px_at(frame.scale),
            false,
        );
        let button_width = layout::px(theme::app::NOTEPAD_BUTTON_WIDTH, frame.scale);
        let button = RECT {
            left: width - button_width - layout::px(theme::space::L, frame.scale),
            top: cy + layout::px(theme::space::S, frame.scale),
            right: width - layout::px(theme::space::L, frame.scale),
            bottom: cy + h - layout::px(theme::space::S, frame.scale),
        };
        fill(dc, button, theme.color.button);
        text(
            dc,
            "Open in Notepad",
            button,
            theme.color.fg,
            theme,
            theme::FOOTNOTE.px_at(frame.scale),
            false,
        );
        cy += h + layout::px(theme::space::S, frame.scale);
    }
    match frame.page {
        Page::Widgets => draw_widgets(
            dc,
            content_left,
            cy - frame.scroll,
            content_width,
            height,
            frame,
        ),
        Page::Dock | Page::Developer => text(
            dc,
            "This page comes in the next build.",
            RECT {
                left: content_left,
                top: cy,
                right: content_left + content_width,
                bottom: cy + layout::px(theme::space::XL, frame.scale),
            },
            theme.color.dim,
            theme,
            theme::BODY.px_at(frame.scale),
            false,
        ),
        Page::Widget(_) => draw_rows(
            dc,
            content_left,
            cy - frame.scroll,
            content_width,
            height,
            frame,
        ),
    }
}

/// Draws the widgets overview cards.
fn draw_widgets(dc: HDC, left: i32, mut y: i32, width: i32, height: i32, frame: &Frame<'_>) {
    let pad = layout::px(theme::app::CARD_PADDING, frame.scale);
    let row_h = layout::px(theme::app::NAV_ROW_HEIGHT * 2.0, frame.scale);
    for (index, widget) in frame.widgets.iter().enumerate() {
        if y + row_h < layout::px(theme::app::HEADER_HEIGHT, frame.scale) || y > height {
            y += row_h + layout::px(theme::app::CARD_GAP, frame.scale);
            continue;
        }
        fill(
            dc,
            RECT {
                left,
                top: y,
                right: left + width,
                bottom: y + row_h,
            },
            frame.theme.color.input,
        );
        let status = match widget.status.as_str() {
            "running" => frame.theme.status.ok,
            "not responding" => frame.theme.status.warn,
            "keeps crashing" => frame.theme.status.danger,
            _ => frame.theme.color.dim,
        };
        let switch_width = layout::px(theme::app::HOME_SWITCH_WIDTH, frame.scale);
        let action_width = layout::px(theme::app::HOME_ACTION_WIDTH, frame.scale);
        let button_y = y + row_h - pad - layout::px(theme::app::CONTROL_HEIGHT, frame.scale);
        let switch_rect = RECT {
            left: left + width - pad - switch_width - action_width * 2 - pad,
            top: button_y,
            right: left + width - action_width * 2 - pad * 2,
            bottom: button_y + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
        };
        text(
            dc,
            "Show on taskbar",
            RECT {
                right: switch_rect.right
                    - layout::px(theme::app::SWITCH_WIDTH, frame.scale)
                    - pad / 2,
                ..switch_rect
            },
            frame.theme.color.fg,
            frame.theme,
            theme::FOOTNOTE.px_at(frame.scale),
            false,
        );
        draw_switch(
            dc,
            RECT {
                left: switch_rect.right - layout::px(theme::app::SWITCH_WIDTH, frame.scale),
                top: switch_rect.top
                    + (switch_rect.bottom
                        - switch_rect.top
                        - layout::px(theme::app::SWITCH_HEIGHT, frame.scale))
                        / 2,
                right: switch_rect.right,
                bottom: switch_rect.top
                    + (switch_rect.bottom - switch_rect.top
                        + layout::px(theme::app::SWITCH_HEIGHT, frame.scale))
                        / 2,
            },
            widget.show_on_taskbar,
            frame,
        );
        if frame.focus_home == Some((index, 0)) {
            outline(dc, switch_rect, frame.theme.status.accent, frame.scale);
        }
        let action_left = left + width - action_width * 2 - pad;
        let running = widget.status == "running" || widget.status == "not responding";
        let action_rect = RECT {
            left: action_left,
            top: button_y,
            right: action_left + action_width,
            bottom: button_y + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
        };
        let busy = frame.busy_actions.get(&widget.name);
        round_fill(
            dc,
            action_rect,
            layout::px(theme::app::BUTTON_RADIUS, frame.scale),
            if busy.is_some() {
                frame.theme.color.dim
            } else if frame
                .hover
                .is_some_and(|(x, y)| contains(action_rect, x, y))
            {
                frame.theme.color.button_hover
            } else {
                frame.theme.color.button
            },
        );
        border(dc, action_rect, frame.theme.color.input_border, frame.scale);
        centred_text(
            dc,
            match busy.map(String::as_str) {
                Some("start") => "Starting\u{2026}",
                Some("quit") => "Quitting\u{2026}",
                Some("restart") => "Restarting\u{2026}",
                _ if running => "Quit",
                _ => "Start",
            },
            action_rect,
            frame.theme.color.fg,
            theme::FOOTNOTE.px_at(frame.scale),
        );
        if frame.focus_home == Some((index, 1)) {
            outline(dc, action_rect, frame.theme.status.accent, frame.scale);
        }
        if running {
            let restart = RECT {
                left: action_left + action_width + pad / 2,
                top: button_y,
                right: left + width - pad,
                bottom: button_y + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
            };
            round_fill(
                dc,
                restart,
                layout::px(theme::app::BUTTON_RADIUS, frame.scale),
                if busy.is_some() {
                    frame.theme.color.dim
                } else if frame.hover.is_some_and(|(x, y)| contains(restart, x, y)) {
                    frame.theme.color.button_hover
                } else {
                    frame.theme.color.button
                },
            );
            border(dc, restart, frame.theme.color.input_border, frame.scale);
            centred_text(
                dc,
                if busy.is_some_and(|action| action == "restart") {
                    "Restarting\u{2026}"
                } else {
                    "Restart"
                },
                restart,
                frame.theme.color.fg,
                theme::FOOTNOTE.px_at(frame.scale),
            );
            if frame.focus_home == Some((index, 2)) {
                outline(dc, restart, frame.theme.status.accent, frame.scale);
            }
        }
        let dot = layout::px(theme::app::STATUS_DOT, frame.scale);
        ellipse(
            dc,
            RECT {
                left: left + pad,
                // centred on the title line (the title's rect: y + pad / 2 .. y + pad + control)
                top: y
                    + (pad / 2 + pad + layout::px(theme::app::CONTROL_HEIGHT, frame.scale) - dot)
                        / 2,
                right: left + pad + dot,
                bottom: y
                    + (pad / 2 + pad + layout::px(theme::app::CONTROL_HEIGHT, frame.scale) + dot)
                        / 2,
            },
            status,
        );
        let title_left = left + pad + dot + pad / 2;
        let title_width = layout::px(
            widget.title.chars().count() as f32 * theme::TITLE_SMALL.px * 0.55 + theme::space::M,
            frame.scale,
        );
        let status_rect = RECT {
            left: title_left + title_width,
            top: y + pad / 2,
            right: left + width - pad,
            bottom: y + pad + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
        };
        text(
            dc,
            &widget.title,
            RECT {
                left: title_left,
                top: status_rect.top,
                right: title_left + title_width,
                bottom: status_rect.bottom,
            },
            frame.theme.color.fg,
            frame.theme,
            theme::TITLE_SMALL.px_at(frame.scale),
            true,
        );
        text(
            dc,
            &format!(
                "{}{}",
                status_label(&widget.status),
                widget
                    .reason
                    .as_deref()
                    .map_or(String::new(), |reason| format!("  {reason}"))
            ),
            status_rect,
            status,
            frame.theme,
            theme::CAPTION.px_at(frame.scale),
            false,
        );
        text(
            dc,
            &widget.description,
            RECT {
                left: left + pad,
                top: y + row_h / 2,
                right: left + width - pad,
                bottom: y + row_h - pad,
            },
            frame.theme.color.dim,
            frame.theme,
            theme::CAPTION.px_at(frame.scale),
            false,
        );
        if frame.focus == Some(index) {
            outline(
                dc,
                RECT {
                    left,
                    top: y,
                    right: left + width,
                    bottom: y + row_h,
                },
                frame.theme.status.accent,
                frame.scale,
            );
        }
        y += row_h + layout::px(theme::app::CARD_GAP, frame.scale);
    }
}

/// Draws widget setting cards with status, current value, and reset visibility.
fn draw_rows(dc: HDC, left: i32, mut y: i32, width: i32, height: i32, frame: &Frame<'_>) {
    let start_y = y;
    if frame.rows.is_empty() {
        if let Some(error) = frame.page_error {
            text(
                dc,
                error,
                RECT {
                    left,
                    top: y,
                    right: left + width,
                    bottom: y + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
                },
                frame.theme.status.warn,
                frame.theme,
                theme::BODY.px_at(frame.scale),
                false,
            );
        }
        return;
    }
    let pad = layout::px(theme::app::CARD_PADDING, frame.scale);
    for (index, row) in frame.rows.iter().enumerate() {
        if let Some(group) = crate::settings_app::model::group_heading(frame.rows, index) {
            let group_height = layout::px(theme::app::GROUP_HEIGHT, frame.scale);
            text(
                dc,
                group,
                RECT {
                    left,
                    top: y,
                    right: left + width,
                    bottom: y + group_height,
                },
                frame.theme.color.dim,
                frame.theme,
                theme::CAPTION.px_at(frame.scale),
                true,
            );
            y += group_height;
        }
        let full_line = layout::full_line(row);
        let card_h = frame
            .card_heights
            .get(index)
            .copied()
            .unwrap_or_else(|| layout::card_height(row, width, frame.scale));
        let error_h = frame
            .error_heights
            .get(index)
            .copied()
            .unwrap_or_else(|| layout::error_height(row, width, frame.scale));
        if y + card_h < layout::px(theme::app::HEADER_HEIGHT, frame.scale) || y > height {
            y += card_h + layout::px(theme::app::CARD_GAP, frame.scale);
            continue;
        }
        fill(
            dc,
            RECT {
                left,
                top: y,
                right: left + width,
                bottom: y + card_h,
            },
            frame.theme.color.button,
        );
        let editing_text = frame
            .edit
            .filter(|(edit_index, _)| *edit_index == index)
            .map(|(_, edit)| edit.text());
        let val = value_text(row, frame.reveal_secret, editing_text);
        let local = layout::row_rects(
            width,
            y,
            card_h,
            layout::px(theme::app::CONTROL_WIDTH, frame.scale),
            full_line,
            row.reset_visible(),
            error_h,
            frame.scale,
        );
        let help_bottom = if full_line {
            local.control.top - pad / 2
        } else {
            y + card_h - pad - error_h
        };
        text(
            dc,
            &row.setting.label,
            RECT {
                left: left + pad,
                top: y + pad / 2,
                right: left + local.label.right,
                bottom: y + pad + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
            },
            frame.theme.color.fg,
            frame.theme,
            theme::TITLE_SMALL.px_at(frame.scale),
            true,
        );
        wrapped_text(
            dc,
            &row.setting.help,
            RECT {
                left: left + pad,
                top: y + pad + layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
                right: left + local.label.right,
                bottom: help_bottom,
            },
            frame.theme.color.dim,
            2,
        );
        let rect = RECT {
            left: left + local.control.left,
            top: local.control.top,
            right: left + local.control.right,
            bottom: local.control.bottom,
        };
        draw_control(dc, rect, row, &val, index, frame);
        if row.reset_visible() {
            let reset_rect = RECT {
                left: left + local.reset.unwrap().left,
                top: local.reset.unwrap().top,
                right: left + local.reset.unwrap().right,
                bottom: local.reset.unwrap().bottom,
            };
            text(
                dc,
                "Reset",
                reset_rect,
                frame.theme.status.accent,
                frame.theme,
                theme::FOOTNOTE.px_at(frame.scale),
                false,
            );
            if frame.focus_reset == Some(index) {
                outline(dc, reset_rect, frame.theme.status.accent, frame.scale);
            }
        }
        if let Some(error) = &row.error {
            wrapped_text(
                dc,
                error,
                RECT {
                    left: left + pad,
                    top: y + card_h - pad - error_h,
                    right: left + width - pad,
                    bottom: y + card_h - pad,
                },
                frame.theme.status.danger,
                4,
            );
        }
        if frame.focus == Some(index) {
            outline(dc, rect, frame.theme.status.accent, frame.scale);
        }
        y += card_h + layout::px(theme::app::CARD_GAP, frame.scale);
    }
    if let Some((index, selected)) = frame.dropdown {
        let mut top = start_y;
        for (row_index, row) in frame.rows.iter().enumerate() {
            if crate::settings_app::model::group_heading(frame.rows, row_index).is_some() {
                top += layout::px(theme::app::GROUP_HEIGHT, frame.scale);
            }
            let full_line = matches!(
                row.control(),
                crate::settings_app::model::Control::Slider
                    | crate::settings_app::model::Control::Text
                    | crate::settings_app::model::Control::Color
                    | crate::settings_app::model::Control::Secret
            );
            let card_h = frame
                .card_heights
                .get(row_index)
                .copied()
                .unwrap_or_else(|| layout::card_height(row, width, frame.scale));
            if row_index == index {
                if let taskbar_dock::describe::Kind::Choice { options, other } = &row.setting.kind {
                    let local = layout::row_rects(
                        width,
                        top,
                        card_h,
                        layout::px(theme::app::CONTROL_WIDTH, frame.scale),
                        full_line,
                        row.reset_visible(),
                        frame
                            .error_heights
                            .get(row_index)
                            .copied()
                            .unwrap_or_else(|| layout::error_height(row, width, frame.scale)),
                        frame.scale,
                    );
                    let trigger = layout::Rect {
                        left: left + local.control.left,
                        top: local.control.top,
                        right: left + local.control.right,
                        bottom: local.control.bottom,
                    };
                    let mut labels = options
                        .iter()
                        .map(|(_, label)| label.as_str())
                        .collect::<Vec<_>>();
                    if *other {
                        labels.push("Other\u{2026}");
                    }
                    let parts = layout::control_model(
                        row,
                        trigger,
                        frame.scale,
                        layout::Rect {
                            left,
                            top: layout::px(theme::app::HEADER_HEIGHT, frame.scale),
                            right: left + width,
                            bottom: height,
                        },
                        true,
                    );
                    let items = parts
                        .parts
                        .iter()
                        .filter_map(|(part, rect)| {
                            matches!(part, layout::ControlPart::DropdownItem(_)).then_some(*rect)
                        })
                        .collect::<Vec<_>>();
                    for (item_index, (item, label)) in items.iter().zip(labels).enumerate() {
                        let part = RECT {
                            left: item.left,
                            top: item.top,
                            right: item.right,
                            bottom: item.bottom,
                        };
                        fill(
                            dc,
                            part,
                            if item_index == selected
                                || frame.hover.is_some_and(|(x, y)| contains(part, x, y))
                            {
                                frame.theme.color.hover
                            } else {
                                frame.theme.color.input
                            },
                        );
                        text(
                            dc,
                            label,
                            RECT {
                                left: part.left
                                    + layout::px(theme::app::FIELD_PADDING, frame.scale),
                                ..part
                            },
                            frame.theme.color.fg,
                            frame.theme,
                            theme::ROW.px_at(frame.scale),
                            false,
                        );
                    }
                }
                break;
            }
            top += card_h + layout::px(theme::app::CARD_GAP, frame.scale);
        }
    }
}

/// Draws the per-row control selected from the described setting and saved value.
fn draw_control(dc: HDC, rect: RECT, row: &Row, value: &str, index: usize, frame: &Frame<'_>) {
    use crate::settings_app::model::Control;
    let theme = frame.theme;
    let control_model = layout::control_model(
        row,
        layout::Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        },
        frame.scale,
        layout::Rect {
            left: rect.left,
            top: layout::px(theme::app::HEADER_HEIGHT, frame.scale),
            right: rect.right,
            bottom: frame.height,
        },
        frame.dropdown.is_some_and(|(open, _)| open == index),
    );
    match control_model.control {
        Control::Segments => {
            fill(dc, rect, theme.color.input);
            if let taskbar_dock::describe::Kind::Choice { options, .. } = &row.setting.kind {
                for (option_index, (option, label)) in options.iter().enumerate() {
                    let segment = control_model
                        .parts
                        .iter()
                        .find_map(|(part, rect)| {
                            (*part == layout::ControlPart::Segment(option_index)).then_some(*rect)
                        })
                        .unwrap_or_default();
                    let part = RECT {
                        left: segment.left,
                        top: segment.top,
                        right: segment.right,
                        bottom: segment.bottom,
                    };
                    let hovered = frame.hover.is_some_and(|(x, y)| contains(part, x, y));
                    if option == &row.value {
                        fill(dc, part, theme.status.accent);
                    } else if hovered {
                        fill(dc, part, theme.color.button_hover);
                    }
                    centred_text(
                        dc,
                        label,
                        part,
                        if option == &row.value {
                            theme.status.on_accent
                        } else {
                            theme.color.fg
                        },
                        theme::FOOTNOTE.px_at(frame.scale),
                    );
                }
            }
            border(dc, rect, theme.color.input_border, frame.scale);
        }
        Control::Dropdown => {
            fill(dc, rect, theme.color.input);
            border(dc, rect, theme.color.input_border, frame.scale);
            text(
                dc,
                value,
                RECT {
                    left: rect.left + layout::px(theme::app::FIELD_PADDING, frame.scale),
                    right: rect.right - layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
                    ..rect
                },
                theme.color.fg,
                theme,
                theme::ROW.px_at(frame.scale),
                false,
            );
            text(
                dc,
                "⌄",
                RECT {
                    left: rect.right - layout::px(theme::app::CONTROL_HEIGHT, frame.scale),
                    ..rect
                },
                theme.color.dim,
                theme,
                theme::ROW.px_at(frame.scale),
                false,
            );
        }
        Control::Slider => {
            fill(dc, rect, theme.color.input);
            let mut label = value.to_string();
            if let taskbar_dock::describe::Kind::Number { min, max, unit, .. } = &row.setting.kind {
                let number = row
                    .draft
                    .as_deref()
                    .and_then(|value| value.parse::<f64>().ok())
                    .or_else(|| row.value.as_f64())
                    .unwrap_or(*min);
                let ratio = ((number - min) / (max - min).max(f64::EPSILON)).clamp(0.0, 1.0) as f32;
                let track_part = control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| {
                        (*part == layout::ControlPart::SliderTrack).then_some(*rect)
                    })
                    .unwrap_or_default();
                let track_y = track_part.top;
                let track = RECT {
                    left: track_part.left,
                    top: track_y,
                    right: track_part.right,
                    bottom: track_part.bottom,
                };
                fill(dc, track, theme.color.track);
                let filled_right =
                    track.left + ((track.right - track.left) as f32 * ratio).round() as i32;
                fill(
                    dc,
                    RECT {
                        right: filled_right,
                        ..track
                    },
                    theme.status.accent,
                );
                let knob = layout::px(theme::app::SLIDER_KNOB, frame.scale);
                ellipse(
                    dc,
                    RECT {
                        left: filled_right - knob / 2,
                        top: track.top + (track.bottom - track.top - knob) / 2,
                        right: filled_right + knob / 2,
                        bottom: track.top + (track.bottom - track.top + knob) / 2,
                    },
                    theme.status.accent,
                );
                label = format!("{} {}", value, unit).trim().to_string();
                let value_rect = control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| (*part == layout::ControlPart::Value).then_some(*rect))
                    .unwrap_or_default();
                text(
                    dc,
                    &label,
                    RECT {
                        left: value_rect.left,
                        top: value_rect.top,
                        right: value_rect.right,
                        bottom: value_rect.bottom,
                    },
                    theme.color.fg,
                    theme,
                    theme::ROW.px_at(frame.scale),
                    false,
                );
            }
            let _ = label;
        }
        Control::Switch => draw_switch(
            dc,
            RECT {
                left: rect.right - layout::px(theme::app::SWITCH_WIDTH, frame.scale),
                top: rect.top
                    + (rect.bottom - rect.top - layout::px(theme::app::SWITCH_HEIGHT, frame.scale))
                        / 2,
                right: rect.right,
                bottom: rect.top
                    + (rect.bottom - rect.top + layout::px(theme::app::SWITCH_HEIGHT, frame.scale))
                        / 2,
            },
            row.value.as_bool() == Some(true),
            frame,
        ),
        Control::Text | Control::Color | Control::Secret => {
            fill(dc, rect, theme.color.input);
            border(dc, rect, theme.color.input_border, frame.scale);
            let swatch = if row.control() == Control::Color {
                value
                    .trim_start_matches('#')
                    .parse::<u32>()
                    .ok()
                    .filter(|_| value.trim_start_matches('#').len() == 6)
                    .map(|rgb| {
                        taskbar_dock::theme::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
                    })
            } else {
                None
            };
            if let Some(color) = swatch {
                let swatch_rect = control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| {
                        (*part == layout::ControlPart::Swatch).then_some(*rect)
                    })
                    .unwrap_or_default();
                fill(
                    dc,
                    RECT {
                        left: swatch_rect.left + layout::px(theme::app::SWATCH_INSET, frame.scale),
                        top: swatch_rect.top + layout::px(theme::app::SWATCH_INSET, frame.scale),
                        right: swatch_rect.right,
                        bottom: swatch_rect.bottom
                            - layout::px(theme::app::SWATCH_INSET, frame.scale),
                    },
                    color,
                );
            }
            let value_rect = if row.control() == Control::Secret {
                let eye_rect = control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| (*part == layout::ControlPart::Eye).then_some(*rect))
                    .unwrap_or_default();
                let eye = RECT {
                    left: eye_rect.left,
                    top: eye_rect.top,
                    right: eye_rect.right,
                    bottom: eye_rect.bottom,
                };
                fill(dc, eye, theme.color.button);
                text(
                    dc,
                    "Eye",
                    eye,
                    theme.color.fg,
                    theme,
                    theme::FOOTNOTE.px_at(frame.scale),
                    false,
                );
                RECT {
                    right: eye.left,
                    ..rect
                }
            } else {
                control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| (*part == layout::ControlPart::Text).then_some(*rect))
                    .map(|rect| RECT {
                        left: rect.left,
                        top: rect.top,
                        right: rect.right,
                        bottom: rect.bottom,
                    })
                    .unwrap_or(rect)
            };
            let text_left = value_rect.left + layout::px(theme::app::FIELD_PADDING, frame.scale);
            let text_right = value_rect.right - layout::px(theme::app::FIELD_PADDING, frame.scale);
            let editing = frame
                .edit
                .is_some_and(|(edit_index, _)| edit_index == index);
            let shown_value = if value.is_empty() && !editing {
                text_placeholder(&row.setting.kind).unwrap_or("")
            } else {
                value
            };
            text(
                dc,
                shown_value,
                RECT {
                    left: text_left,
                    right: text_right,
                    ..value_rect
                },
                if value.is_empty() && !editing {
                    theme.color.dim
                } else {
                    theme.color.fg
                },
                theme,
                theme::ROW.px_at(frame.scale),
                false,
            );
        }
        Control::Auto => {
            if let taskbar_dock::describe::Kind::Auto { word, label, inner } = &row.setting.kind {
                let automatic = row.value.as_str() == Some(word);
                let switch_part = control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| {
                        (*part == layout::ControlPart::AutoSwitch).then_some(*rect)
                    })
                    .unwrap_or_default();
                let inner_part = control_model
                    .parts
                    .iter()
                    .find_map(|(part, rect)| {
                        (*part == layout::ControlPart::AutoInner).then_some(*rect)
                    })
                    .unwrap_or_default();
                let switch = RECT {
                    left: switch_part.left,
                    top: switch_part.top,
                    right: switch_part.right,
                    bottom: switch_part.bottom,
                };
                let inner_rect = RECT {
                    left: inner_part.left,
                    top: inner_part.top,
                    right: inner_part.right,
                    bottom: inner_part.bottom,
                };
                fill(dc, switch, theme.color.button);
                text(
                    dc,
                    if automatic { label } else { "Manual" },
                    switch,
                    theme.color.fg,
                    theme,
                    theme::FOOTNOTE.px_at(frame.scale),
                    false,
                );
                fill(
                    dc,
                    inner_rect,
                    if automatic {
                        theme.color.input
                    } else {
                        theme.color.button
                    },
                );
                let inner_value = if automatic {
                    "Automatic".to_string()
                } else {
                    row.value.to_string()
                };
                let _ = inner;
                text(
                    dc,
                    &inner_value,
                    inner_rect,
                    theme.color.fg,
                    theme,
                    theme::FOOTNOTE.px_at(frame.scale),
                    false,
                );
            }
        }
    }
    if let Some((edit_index, edit)) = frame.edit.filter(|(edit_index, _)| *edit_index == index) {
        let _ = edit_index;
        let text_rect = control_model
            .parts
            .iter()
            .find_map(|(part, rect)| (*part == layout::ControlPart::Text).then_some(*rect))
            .map_or(rect, |part| RECT {
                left: part.left + layout::px(theme::app::FIELD_PADDING, frame.scale),
                top: part.top,
                right: part.right - layout::px(theme::app::FIELD_PADDING, frame.scale),
                bottom: part.bottom,
            });
        draw_edit_state(
            dc,
            text_rect,
            edit,
            matches!(row.setting.kind, taskbar_dock::describe::Kind::Secret),
            theme,
            frame.scale,
        );
    }
}

/// Formats a row value without revealing secrets.
fn value_text(row: &Row, reveal_secret: bool, editing_text: Option<&str>) -> String {
    if matches!(row.setting.kind, taskbar_dock::describe::Kind::Secret) {
        return if reveal_secret {
            editing_text
                .unwrap_or_else(|| row.value.as_str().unwrap_or(""))
                .to_string()
        } else {
            "•".repeat(
                editing_text
                    .unwrap_or_else(|| row.value.as_str().unwrap_or(""))
                    .chars()
                    .count(),
            )
        };
    }
    if row.key == "startup" || row.setting.key == "show_on_taskbar" || row.setting.key == "compact"
    {
        return if row.value.as_bool() == Some(true) {
            "On".into()
        } else {
            "Off".into()
        };
    }
    if let taskbar_dock::describe::Kind::Choice { options, .. } = &row.setting.kind {
        if let Some((_, label)) = options.iter().find(|(value, _)| value == &row.value) {
            return label.clone();
        }
    }
    if row.value.is_null() {
        return String::new();
    }
    row.draft
        .clone()
        .or_else(|| row.value.as_str().map(str::to_string))
        .unwrap_or_else(|| row.value.to_string())
}

/// Returns the selected page's visible title.
fn page_title<'a>(page: &Page, nav: &'a [(String, Page)]) -> &'a str {
    match page {
        Page::Widget(_) => nav
            .iter()
            .find(|(_, target)| target == page)
            .map_or("Widget", |(title, _)| title),
        _ => match page {
            Page::Widgets => "Widgets",
            Page::Dock => "Dock",
            Page::Developer => "Developer",
            Page::Widget(_) => unreachable!(),
        },
    }
}

/// Returns the capitalized status label shown on the Widgets page.
fn status_label(status: &str) -> &'static str {
    match status {
        "running" => "Running",
        "stopped" => "Stopped",
        "not responding" => "Not responding",
        "keeps crashing" => "Keeps crashing",
        _ => "Stopped",
    }
}

/// Returns a text or optional text control's placeholder.
fn text_placeholder(kind: &taskbar_dock::describe::Kind) -> Option<&str> {
    match kind {
        taskbar_dock::describe::Kind::Text { placeholder, .. } => Some(placeholder),
        taskbar_dock::describe::Kind::Optional(inner) => text_placeholder(inner),
        _ => None,
    }
}

/// Fills a rectangle with an owned brush.
fn fill(dc: HDC, rect: RECT, color: Rgb) {
    unsafe {
        let brush = CreateSolidBrush(color.colorref());
        FillRect(dc, &rect, brush);
        DeleteObject(brush);
    }
}

/// Draws a switch using the menu panel's track, knob and proportions.
fn draw_switch(dc: HDC, rect: RECT, on: bool, frame: &Frame<'_>) {
    let height = rect.bottom - rect.top;
    let width = rect.right - rect.left;
    let radius = height;
    let track = if on {
        frame.theme.status.accent
    } else {
        frame.theme.color.knob
    };
    round_fill(dc, rect, radius, track);
    if !on {
        let inset = layout::px(theme::app::SWITCH_BORDER, frame.scale);
        round_fill(
            dc,
            RECT {
                left: rect.left + inset,
                top: rect.top + inset,
                right: rect.right - inset,
                bottom: rect.bottom - inset,
            },
            (height - 2 * inset).max(1),
            frame.theme.color.bg,
        );
    }
    let diameter = ((height as f32 * if on { 0.6 } else { 0.5 }).round() as i32)
        .min(height - 2 * layout::px(theme::app::SWITCH_BORDER, frame.scale));
    let cy = rect.top + height / 2;
    let cx = if on {
        rect.right - height / 2
    } else {
        rect.left + height / 2
    };
    let _ = width;
    ellipse(
        dc,
        RECT {
            left: cx - diameter / 2,
            top: cy - diameter / 2,
            right: cx + diameter / 2,
            bottom: cy + diameter / 2,
        },
        if on {
            frame.theme.color.bg
        } else {
            frame.theme.color.knob
        },
    );
}

/// Fills a rounded rectangle with a temporary GDI brush.
fn round_fill(dc: HDC, rect: RECT, radius: i32, color: Rgb) {
    unsafe {
        let brush = CreateSolidBrush(color.colorref());
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(brush);
    }
}

/// Fills an ellipse with a temporary GDI brush.
fn ellipse(dc: HDC, rect: RECT, color: Rgb) {
    unsafe {
        let brush = CreateSolidBrush(color.colorref());
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        Ellipse(dc, rect.left, rect.top, rect.right, rect.bottom);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(brush);
    }
}

/// Draws a one-pixel themed border around a rectangle.
fn border(dc: HDC, rect: RECT, color: Rgb, scale: f32) {
    unsafe {
        let pen = CreatePen(
            PS_SOLID,
            layout::px(theme::app::FIELD_BORDER, scale),
            color.colorref(),
        );
        let old_pen = SelectObject(dc, pen);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Rectangle(dc, rect.left, rect.top, rect.right, rect.bottom);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        DeleteObject(pen);
    }
}

/// Tests whether a point lies inside a GDI rectangle.
fn contains(rect: RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

/// Draws a themed single-line label.
fn text(dc: HDC, text: &str, rect: RECT, color: Rgb, _theme: &Theme, size: i32, bold: bool) {
    aligned_text(dc, text, rect, color, size, bold, DT_LEFT);
}

/// Draws a themed single-line label centred in its rectangle.
fn centred_text(dc: HDC, text: &str, rect: RECT, color: Rgb, size: i32) {
    aligned_text(dc, text, rect, color, size, false, DT_CENTER);
}

/// Draws a single-line label with the cached font for its size and the given alignment.
fn aligned_text(
    dc: HDC,
    text: &str,
    rect: RECT,
    color: Rgb,
    size: i32,
    bold: bool,
    align: DRAW_TEXT_FORMAT,
) {
    // An empty Vec's pointer is dangling; DrawTextW reads it even with a length of 0.
    if text.is_empty() {
        return;
    }
    unsafe {
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color.colorref());
        let sizes = ACTIVE_FONTS.with(|fonts| fonts.get().1);
        let slot = if bold && size == sizes[3] {
            3
        } else if bold {
            1
        } else if size == sizes[2] {
            2
        } else if size == sizes[4] {
            4
        } else {
            0
        };
        let font = ACTIVE_FONTS.with(|fonts| fonts.get().0[slot]);
        let old = SelectObject(dc, font);
        let wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut { rect },
            align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        SelectObject(dc, old);
    }
}

/// Draws help or error text wrapped with the matching cached font.
fn wrapped_text(dc: HDC, text: &str, rect: RECT, color: Rgb, font_slot: usize) {
    if text.is_empty() || rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    unsafe {
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color.colorref());
        let font = ACTIVE_FONTS.with(|fonts| fonts.get().0[font_slot]);
        let old = SelectObject(dc, font);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = rect;
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
        );
        SelectObject(dc, old);
    }
}

/// Draws the active editor's selection and caret over its displayed text.
fn draw_edit_state(
    dc: HDC,
    rect: RECT,
    edit: &taskbar_dock::text_edit::TextEdit,
    secret: bool,
    theme: &Theme,
    scale: f32,
) {
    let prefix_width = |count: usize| {
        let prefix = if secret {
            "•".repeat(count)
        } else {
            edit.text().chars().take(count).collect::<String>()
        };
        let wide: Vec<u16> = prefix.encode_utf16().collect();
        let mut size: SIZE = unsafe { std::mem::zeroed() };
        if !wide.is_empty() {
            unsafe {
                let font = ACTIVE_FONTS.with(|fonts| fonts.get().0[0]);
                let old = SelectObject(dc, font);
                GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut size);
                SelectObject(dc, old);
            }
        }
        size.cx
    };
    if let Some((start, end)) = edit.selection_range() {
        let left = rect.left + prefix_width(start);
        let right = rect.left + prefix_width(end);
        fill(
            dc,
            RECT {
                left,
                top: rect.top + layout::px(theme::space::XS, scale),
                right,
                bottom: rect.bottom - layout::px(theme::space::XS, scale),
            },
            theme.status.accent,
        );
        let selected = if secret {
            "•".repeat(end - start)
        } else {
            edit.text()
                .chars()
                .skip(start)
                .take(end - start)
                .collect::<String>()
        };
        text(
            dc,
            &selected,
            RECT {
                left,
                right,
                ..rect
            },
            theme.status.on_accent,
            theme,
            theme::ROW.px_at(scale),
            false,
        );
    }
    let x = rect.left + prefix_width(edit.caret());
    unsafe {
        let pen = CreatePen(
            PS_SOLID,
            layout::px(theme::app::FOCUS_RING, scale),
            theme.color.fg.colorref(),
        );
        let old = SelectObject(dc, pen);
        MoveToEx(
            dc,
            x,
            rect.top + layout::px(theme::space::XS, scale),
            std::ptr::null_mut(),
        );
        LineTo(dc, x, rect.bottom - layout::px(theme::space::XS, scale));
        SelectObject(dc, old);
        DeleteObject(pen);
    }
}

/// Draws a focus outline from the live accent theme.
fn outline(dc: HDC, rect: RECT, color: Rgb, scale: f32) {
    unsafe {
        let pen = CreatePen(
            PS_SOLID,
            layout::px(theme::app::FOCUS_RING, scale),
            color.colorref(),
        );
        let old = SelectObject(dc, pen);
        let brush = GetStockObject(NULL_BRUSH as i32);
        let old_brush = SelectObject(dc, brush);
        Rectangle(dc, rect.left, rect.top, rect.right, rect.bottom);
        SelectObject(dc, old_brush);
        SelectObject(dc, old);
        DeleteObject(pen);
    }
}
