//! Pure pixel layout and hit testing for the settings application.

use crate::settings_app::model::{Control, Row};
use taskbar_dock::theme;
/// An integer rectangle in client pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge, exclusive.
    pub right: i32,
    /// Bottom edge, exclusive.
    pub bottom: i32,
}

impl Rect {
    /// Checks whether a point lies inside this rectangle.
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// A navigation, row, control, or action target returned by hit testing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Navigation entry index.
    Nav(usize),
    /// Content row index.
    Row(usize),
    /// Row reset action.
    Reset(usize),
    /// Interactive row control.
    Control(usize),
    /// Status action.
    Action(usize),
    /// Notepad button.
    Notepad,
}

/// Scaled rectangles for one card row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowRects {
    /// Full card rectangle.
    pub card: Rect,
    /// Setting label and help area.
    pub label: Rect,
    /// Interactive control area.
    pub control: Rect,
    /// Optional full-width second control line.
    pub second_line: Option<Rect>,
    /// Optional reset button.
    pub reset: Option<Rect>,
}

/// A named sub-rectangle inside a row control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlPart {
    /// Whole switch, dropdown, text box, or value label.
    Main,
    /// One button in a segmented choice.
    Segment(usize),
    /// One option in an open dropdown.
    DropdownItem(usize),
    /// The draggable numeric track.
    SliderTrack,
    /// The numeric value editor area.
    Value,
    /// Parsed color swatch.
    Swatch,
    /// Text entry area.
    Text,
    /// Secret reveal button.
    Eye,
    /// Automatic mode switch.
    AutoSwitch,
    /// Automatic mode's inner control.
    AutoInner,
}

/// The complete per-row control choice and its hit-test rectangles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlModel {
    /// Selected control kind for paint, keys, and reported state.
    pub control: Control,
    /// Ordered control sub-rectangles.
    pub parts: Vec<(ControlPart, Rect)>,
}

/// Chooses a row control and computes every sub-rectangle used by its UI paths.
pub fn control_model(
    row: &Row,
    rect: Rect,
    scale: f32,
    viewport: Rect,
    dropdown_open: bool,
) -> ControlModel {
    let control = row.control();
    let mut parts = Vec::new();
    match control {
        Control::Segments => {
            if let taskbar_dock::describe::Kind::Choice { options, .. } = &row.setting.kind {
                for index in 0..options.len() {
                    let segment_width = (rect.right - rect.left) / options.len().max(1) as i32;
                    let left = rect.left + segment_width * index as i32;
                    let right = if index + 1 == options.len() {
                        rect.right
                    } else {
                        left + segment_width
                    };
                    parts.push((
                        ControlPart::Segment(index),
                        Rect {
                            left,
                            right,
                            ..rect
                        },
                    ));
                }
            }
        }
        Control::Dropdown => {
            parts.push((ControlPart::Main, rect));
            if dropdown_open {
                if let taskbar_dock::describe::Kind::Choice { options, other } = &row.setting.kind {
                    let count = options.len() + usize::from(*other);
                    for (index, item) in dropdown_rects(
                        rect,
                        count,
                        px(taskbar_dock::theme::app::CONTROL_HEIGHT, scale),
                        viewport,
                    )
                    .into_iter()
                    .enumerate()
                    {
                        parts.push((ControlPart::DropdownItem(index), item));
                    }
                }
            }
        }
        Control::Slider => {
            let value_width = px(taskbar_dock::theme::app::SLIDER_VALUE_WIDTH, scale);
            parts.push((
                ControlPart::Value,
                Rect {
                    left: rect.right - value_width,
                    ..rect
                },
            ));
            let track_top = rect.top + (rect.bottom - rect.top) * 3 / 4;
            parts.push((
                ControlPart::SliderTrack,
                Rect {
                    left: rect.left,
                    top: track_top,
                    right: rect.right
                        - value_width
                        - px(taskbar_dock::theme::app::CONTROL_GAP, scale),
                    bottom: track_top + px(taskbar_dock::theme::app::SLIDER_TRACK_HEIGHT, scale),
                },
            ));
        }
        Control::Color => {
            let swatch_width = px(
                taskbar_dock::theme::app::CONTROL_HEIGHT - taskbar_dock::theme::space::S,
                scale,
            );
            parts.push((
                ControlPart::Text,
                Rect {
                    right: rect.right - swatch_width,
                    ..rect
                },
            ));
            parts.push((
                ControlPart::Swatch,
                Rect {
                    left: (rect.right - swatch_width).max(rect.left),
                    right: rect.right,
                    ..rect
                },
            ));
        }
        Control::Secret => {
            let eye_width = px(taskbar_dock::theme::app::CONTROL_HEIGHT, scale);
            parts.push((
                ControlPart::Text,
                Rect {
                    right: rect.right - eye_width,
                    ..rect
                },
            ));
            parts.push((
                ControlPart::Eye,
                Rect {
                    left: rect.right - eye_width,
                    ..rect
                },
            ));
        }
        Control::Auto => {
            let split = rect.left + (rect.right - rect.left) / 2;
            parts.push((
                ControlPart::AutoSwitch,
                Rect {
                    right: split,
                    ..rect
                },
            ));
            parts.push((
                ControlPart::AutoInner,
                Rect {
                    left: split,
                    ..rect
                },
            ));
        }
        Control::Switch | Control::Text => parts.push((ControlPart::Main, rect)),
    }
    ControlModel { control, parts }
}

/// Rectangles for one dropdown list, placed below or above its trigger.
pub fn dropdown_rects(trigger: Rect, count: usize, item_height: i32, viewport: Rect) -> Vec<Rect> {
    let count = count.min(64);
    let total = count as i32 * item_height;
    let top = if trigger.bottom + total <= viewport.bottom {
        trigger.bottom
    } else {
        (trigger.top - total).max(viewport.top)
    };
    (0..count)
        .map(|index| Rect {
            left: trigger.left,
            top: top + index as i32 * item_height,
            right: trigger.right,
            bottom: top + (index as i32 + 1) * item_height,
        })
        .collect()
}

/// Scales a theme size to whole client pixels.
pub fn px(size: f32, scale: f32) -> i32 {
    (size * scale).round() as i32
}

/// Reports whether a row control uses a separate full-width line.
pub fn full_line(row: &Row) -> bool {
    matches!(
        row.control(),
        Control::Slider | Control::Text | Control::Color | Control::Secret
    )
}

/// Measures a card's height from its wrapped help and error text.
pub fn card_height(row: &Row, width: i32, scale: f32) -> i32 {
    let pad = px(taskbar_dock::theme::app::CARD_PADDING, scale);
    let full_line = full_line(row);
    let mut label_width = width - 2 * pad;
    if !full_line {
        label_width -= px(
            taskbar_dock::theme::app::CONTROL_WIDTH + taskbar_dock::theme::app::LABEL_GAP,
            scale,
        );
        label_width -= px(
            taskbar_dock::theme::app::RESET_WIDTH + taskbar_dock::theme::app::CONTROL_GAP,
            scale,
        );
    }
    let help_lines = wrapped_lines(&row.setting.help, label_width, px(theme::CAPTION.px, scale));
    let error_height = error_height(row, width, scale);
    card_height_measured(
        row,
        scale,
        help_lines * px(theme::CAPTION.px, scale),
        error_height,
    )
}

/// Computes a card's height from measured help and error pixel heights.
pub fn card_height_measured(row: &Row, scale: f32, help_height: i32, error_height: i32) -> i32 {
    let pad = px(theme::app::CARD_PADDING, scale);
    // A control on its own line sits half a padding below the help text (paint's help_bottom).
    let (control_lines, gap) = if full_line(row) { (2, pad / 2) } else { (1, 0) };
    pad * 2
        + control_lines * px(theme::app::CONTROL_HEIGHT, scale)
        + help_height
        + gap
        + error_height
}

/// Measures the error block below a row control.
pub fn error_height(row: &Row, width: i32, scale: f32) -> i32 {
    row.error.as_deref().map_or(0, |error| {
        wrapped_lines(
            error,
            width - 2 * px(taskbar_dock::theme::app::CARD_PADDING, scale),
            px(theme::FOOTNOTE.px, scale),
        ) * px(theme::FOOTNOTE.px, scale)
    })
}

/// Estimates wrapped text lines using the theme font's average glyph width.
pub fn wrapped_lines(text: &str, width: i32, font_height: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let chars_per_line = (width.max(1) as f32 / (font_height.max(1) as f32 * 0.55))
        .floor()
        .max(1.0) as usize;
    ((text.chars().count() + chars_per_line - 1) / chars_per_line).max(1) as i32
}

/// Places a card's controls on the right or on a second full-width line.
pub fn row_rects(
    width: i32,
    y: i32,
    card_height: i32,
    control_width: i32,
    full_line: bool,
    reset_visible: bool,
    error_height: i32,
    scale: f32,
) -> RowRects {
    let pad = px(taskbar_dock::theme::app::CARD_PADDING, scale);
    let right = width - pad;
    let reset_width = px(taskbar_dock::theme::app::RESET_WIDTH, scale);
    let control_width = control_width.min((width - 2 * pad).max(1));
    let control_height = px(taskbar_dock::theme::app::CONTROL_HEIGHT, scale);
    let reset_y = if full_line {
        y + card_height - pad - error_height - control_height
    } else {
        y + pad
    };
    let reset = Rect {
        left: right - reset_width,
        top: reset_y,
        right,
        bottom: reset_y + control_height,
    };
    let reset_gap = reset_width + px(taskbar_dock::theme::app::CONTROL_GAP, scale);
    let control = if full_line {
        Rect {
            left: pad,
            top: y + card_height
                - pad
                - error_height
                - px(taskbar_dock::theme::app::CONTROL_HEIGHT, scale),
            right: right - reset_gap,
            bottom: y + card_height - pad - error_height,
        }
    } else {
        Rect {
            left: right - reset_gap - control_width,
            top: y + pad,
            right: right - reset_gap,
            bottom: y + pad + px(taskbar_dock::theme::app::CONTROL_HEIGHT, scale),
        }
    };
    let label_right = if full_line {
        width - pad
    } else {
        control.left - px(taskbar_dock::theme::app::LABEL_GAP, scale)
    };
    RowRects {
        card: Rect {
            left: 0,
            top: y,
            right: width,
            bottom: y + card_height,
        },
        label: Rect {
            left: pad,
            top: y + pad,
            right: label_right,
            bottom: y + card_height - pad,
        },
        control,
        second_line: full_line.then_some(control),
        reset: reset_visible.then_some(reset),
    }
}

/// Clamps the page scroll position to the content's valid range.
pub fn clamp_scroll(scroll: i32, content: i32, viewport: i32) -> i32 {
    scroll.clamp(0, (content - viewport).max(0))
}

/// Finds the first hit rectangle containing a point.
pub fn hit_test(x: i32, y: i32, items: &[(Rect, Hit)]) -> Option<Hit> {
    items
        .iter()
        .find(|(rect, _)| rect.contains(x, y))
        .map(|(_, hit)| hit.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use taskbar_dock::describe::{Kind, Setting};

    /// Checks scaled layout and full-line controls at 100 and 150 percent.
    #[test]
    fn layouts_scale_and_move_wide_controls_below() {
        assert_eq!(px(taskbar_dock::theme::app::CARD_PADDING, 1.0), 16);
        assert_eq!(px(taskbar_dock::theme::app::CARD_PADDING, 1.5), 24);
        let rects = row_rects(600, 0, 150, 360, true, true, 0, 1.0);
        assert!(rects.second_line.unwrap().top > rects.label.top);
        let inline = row_rects(600, 0, 100, 180, false, false, 0, 1.5);
        assert!(inline.control.left > inline.label.left);
        let full = row_rects(600, 0, 150, 360, true, false, 0, 1.0);
        assert!(full.control.right - full.control.left > 300);
        assert!(full.reset.is_none());
    }

    /// Checks Reset visibility never moves a card's control column.
    #[test]
    fn reset_column_is_reserved_for_every_card() {
        let visible = row_rects(600, 0, 120, 180, false, true, 0, 1.0);
        let hidden = row_rects(600, 0, 120, 180, false, false, 0, 1.0);
        assert_eq!(visible.control, hidden.control);
        assert_eq!(visible.control.right, hidden.control.right);
    }

    /// Checks measured wrapped help lines directly determine card height.
    #[test]
    fn measured_help_lines_set_card_height() {
        let row = Row::new(
            "w",
            Setting::new(
                "text",
                "Text",
                Kind::Text {
                    placeholder: String::new(),
                    empty_ok: true,
                },
            ),
            json!(""),
            json!(""),
        );
        let one = card_height_measured(&row, 1.0, 16, 0);
        let three = card_height_measured(&row, 1.0, 48, 0);
        assert_eq!(three - one, 32);
    }

    /// Checks scroll clamping and hit testing for representative controls.
    #[test]
    fn clamps_scroll_and_hits_nav_rows_controls_and_buttons() {
        assert_eq!(clamp_scroll(-3, 500, 300), 0);
        assert_eq!(clamp_scroll(400, 500, 300), 200);
        let items = [
            (
                Rect {
                    left: 0,
                    top: 0,
                    right: 40,
                    bottom: 40,
                },
                Hit::Nav(0),
            ),
            (
                Rect {
                    left: 40,
                    top: 0,
                    right: 80,
                    bottom: 40,
                },
                Hit::Row(1),
            ),
            (
                Rect {
                    left: 80,
                    top: 0,
                    right: 120,
                    bottom: 40,
                },
                Hit::Control(1),
            ),
            (
                Rect {
                    left: 120,
                    top: 0,
                    right: 160,
                    bottom: 40,
                },
                Hit::Reset(1),
            ),
        ];
        assert_eq!(hit_test(10, 10, &items), Some(Hit::Nav(0)));
        assert_eq!(hit_test(50, 10, &items), Some(Hit::Row(1)));
        assert_eq!(hit_test(90, 10, &items), Some(Hit::Control(1)));
        assert_eq!(hit_test(130, 10, &items), Some(Hit::Reset(1)));
    }

    /// Places every dropdown item within the viewport, preferring below then above.
    #[test]
    fn dropdown_list_flips_above_when_needed() {
        let trigger = Rect {
            left: 20,
            top: 80,
            right: 180,
            bottom: 112,
        };
        let below = dropdown_rects(
            trigger,
            3,
            24,
            Rect {
                left: 0,
                top: 0,
                right: 200,
                bottom: 200,
            },
        );
        assert_eq!(below.first().unwrap().top, 112);
        let above = dropdown_rects(
            trigger,
            3,
            24,
            Rect {
                left: 0,
                top: 0,
                right: 200,
                bottom: 140,
            },
        );
        assert_eq!(above.first().unwrap().top, 8);
        assert!(above.last().unwrap().bottom <= trigger.top);
    }

    /// Builds and hit-tests the shared model for switches, choices, sliders, text, and secrets.
    #[test]
    fn control_model_exposes_every_interactive_subrect() {
        let bounds = Rect {
            left: 20,
            top: 40,
            right: 320,
            bottom: 80,
        };
        let viewport = Rect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        let switch = Row::new(
            "w",
            Setting::new("s", "Switch", Kind::Switch),
            json!(false),
            json!(false),
        );
        assert!(matches!(
            control_model(&switch, bounds, 1.0, viewport, false).parts[0].0,
            ControlPart::Main
        ));
        let segments = Row::new(
            "w",
            Setting::new(
                "c",
                "Choice",
                Kind::Choice {
                    options: vec![
                        (json!("a"), "A".into()),
                        (json!("b"), "B".into()),
                        (json!("c"), "C".into()),
                    ],
                    other: false,
                },
            ),
            json!("a"),
            json!("a"),
        );
        let segment_model = control_model(&segments, bounds, 1.0, viewport, false);
        assert_eq!(
            segment_model
                .parts
                .iter()
                .filter(|(part, _)| matches!(part, ControlPart::Segment(_)))
                .count(),
            3
        );
        assert_eq!(
            hit_test(25, 50, &[(segment_model.parts[0].1, Hit::Control(0))]),
            Some(Hit::Control(0))
        );
        let dropdown = Row::new(
            "w",
            Setting::new(
                "d",
                "Dropdown",
                Kind::Choice {
                    options: vec![
                        (json!("a"), "A".into()),
                        (json!("b"), "B".into()),
                        (json!("c"), "C".into()),
                        (json!("d"), "D".into()),
                    ],
                    other: true,
                },
            ),
            json!("a"),
            json!("a"),
        );
        let open = control_model(&dropdown, bounds, 1.5, viewport, true);
        assert_eq!(
            open.parts
                .iter()
                .filter(|(part, _)| matches!(part, ControlPart::DropdownItem(_)))
                .count(),
            5
        );
        let slider = Row::new(
            "w",
            Setting::new(
                "n",
                "Number",
                Kind::Number {
                    min: 0.0,
                    max: 1.0,
                    step: 0.1,
                    integer: false,
                    unit: String::new(),
                },
            ),
            json!(0.5),
            json!(0.0),
        );
        assert!(control_model(&slider, bounds, 1.0, viewport, false)
            .parts
            .iter()
            .any(|(part, _)| *part == ControlPart::SliderTrack));
        let secret = Row::new(
            "w",
            Setting::new("secret", "Secret", Kind::Secret),
            json!("x"),
            json!(""),
        );
        let eye = control_model(&secret, bounds, 1.0, viewport, false)
            .parts
            .iter()
            .find(|(part, _)| *part == ControlPart::Eye)
            .unwrap()
            .1;
        assert_eq!(
            hit_test(eye.left + 1, eye.top + 1, &[(eye, Hit::Control(1))]),
            Some(Hit::Control(1))
        );
        let reset = row_rects(500, 0, 120, 180, false, true, 0, 1.5)
            .reset
            .unwrap();
        assert_eq!(
            hit_test(reset.left + 1, reset.top + 1, &[(reset, Hit::Reset(1))]),
            Some(Hit::Reset(1))
        );
    }

    /// Hit-tests control sub-rectangles at the base and high-DPI scales.
    #[test]
    fn switch_segment_slider_dropdown_and_swatch_hits_scale() {
        let viewport = Rect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 500,
        };
        for scale in [1.0, 1.5] {
            let bounds = Rect {
                left: 20,
                top: 40,
                right: 320,
                bottom: 80,
            };
            let switch = Row::new(
                "w",
                Setting::new("s", "Switch", Kind::Switch),
                json!(true),
                json!(false),
            );
            let switch_part = control_model(&switch, bounds, scale, viewport, false).parts[0].1;
            assert!(switch_part.contains(switch_part.left, switch_part.top));

            let choice = Row::new(
                "w",
                Setting::new(
                    "c",
                    "Choice",
                    Kind::Choice {
                        options: vec![(json!("a"), "A".into()), (json!("b"), "B".into())],
                        other: false,
                    },
                ),
                json!("a"),
                json!("a"),
            );
            let segments = control_model(&choice, bounds, scale, viewport, false);
            for (_, part) in segments.parts {
                assert!(part.right > part.left);
                assert!(part.contains(part.left, part.top));
            }

            let slider = Row::new(
                "w",
                Setting::new(
                    "n",
                    "Number",
                    Kind::Number {
                        min: 0.0,
                        max: 10.0,
                        step: 1.0,
                        integer: true,
                        unit: "min".into(),
                    },
                ),
                json!(5),
                json!(0),
            );
            let slider_parts = control_model(&slider, bounds, scale, viewport, false).parts;
            let track = slider_parts
                .iter()
                .find(|(part, _)| *part == ControlPart::SliderTrack)
                .unwrap()
                .1;
            let value = slider_parts
                .iter()
                .find(|(part, _)| *part == ControlPart::Value)
                .unwrap()
                .1;
            assert!(track.right < value.left);
            assert!(track.contains(track.left, track.top));
            assert!(value.contains(value.left, value.top));

            let dropdown = Row::new(
                "w",
                Setting::new(
                    "d",
                    "Dropdown",
                    Kind::Choice {
                        options: vec![
                            (json!("a"), "A".into()),
                            (json!("b"), "B".into()),
                            (json!("c"), "C".into()),
                            (json!("d"), "D".into()),
                        ],
                        other: false,
                    },
                ),
                json!("a"),
                json!("a"),
            );
            let dropdown_parts = control_model(&dropdown, bounds, scale, viewport, true).parts;
            assert!(dropdown_parts.iter().any(|(part, rect)| {
                matches!(part, ControlPart::DropdownItem(_)) && rect.contains(rect.left, rect.top)
            }));

            let color = Row::new(
                "w",
                Setting::new(
                    "color",
                    "Color",
                    Kind::Color {
                        hash: true,
                        empty_ok: true,
                    },
                ),
                json!("#112233"),
                json!(""),
            );
            let color_parts = control_model(&color, bounds, scale, viewport, false).parts;
            let swatch = color_parts
                .iter()
                .find(|(part, _)| *part == ControlPart::Swatch)
                .unwrap()
                .1;
            let text = color_parts
                .iter()
                .find(|(part, _)| *part == ControlPart::Text)
                .unwrap()
                .1;
            assert_eq!(swatch.right, bounds.right);
            assert!(text.right <= swatch.left);
        }
    }
}
