//! Pure control selection and keyboard focus rules for the settings application.
use crate::settings_app::model::{self, Control, KeyTarget, Row};
use taskbar_dock::describe::Kind;

/// The supported named keyboard operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Move focus forward.
    Tab,
    /// Move focus backward.
    ShiftTab,
    /// Activate or commit.
    Enter,
    /// Cancel or close.
    Escape,
    /// Toggle a switch.
    Space,
    /// Move to the previous option or navigation entry.
    Up,
    /// Move to the next option or navigation entry.
    Down,
    /// Move to the previous segment or lower slider step.
    Left,
    /// Move to the next segment or higher slider step.
    Right,
    /// Move to the first option or scroll start.
    Home,
    /// Move to the last option or scroll end.
    End,
    /// Begin text editing.
    F2,
    /// Scroll one page upward.
    PageUp,
    /// Scroll one page downward.
    PageDown,
}

/// Focusable item kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusKind {
    /// Navigation entry.
    Navigation,
    /// Main control for a row.
    Control,
    /// Reset button.
    Reset,
    /// Widget action button.
    Action,
    /// Developer page button.
    DeveloperAction,
}

/// A focus target identified in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    /// Kind of focus target.
    pub kind: FocusKind,
    /// Index within navigation or row controls.
    pub index: usize,
}

/// Selects an appropriate UI control for a description kind.
pub fn control(kind: &Kind) -> Control {
    model::control_for(kind)
}

/// Builds focus order from navigation, controls, reset buttons, and status actions.
pub fn focus_order(nav_count: usize, rows: &[Row], action_count: usize) -> Vec<Focus> {
    let mut focus = (0..nav_count)
        .map(|index| Focus {
            kind: FocusKind::Navigation,
            index,
        })
        .collect::<Vec<_>>();
    for (index, row) in rows.iter().enumerate() {
        focus.push(Focus {
            kind: FocusKind::Control,
            index,
        });
        if row.reset_visible() {
            focus.push(Focus {
                kind: FocusKind::Reset,
                index,
            });
        }
    }
    focus.extend((0..action_count).map(|index| Focus {
        kind: FocusKind::Action,
        index,
    }));
    focus
}

/// Builds Developer focus order after nav and advanced row controls.
pub fn developer_focus_order(nav_count: usize, rows: &[Row], action_count: usize) -> Vec<Focus> {
    let mut focus = focus_order(nav_count, rows, 0);
    focus.extend((0..action_count).map(|index| Focus {
        kind: FocusKind::DeveloperAction,
        index,
    }));
    focus
}

/// Returns the next focus target, wrapping in either direction.
pub fn next_focus(order: &[Focus], current: Option<Focus>, reverse: bool) -> Option<Focus> {
    if order.is_empty() {
        return None;
    }
    let current_index = current.and_then(|focus| order.iter().position(|item| *item == focus));
    let next = match (current_index, reverse) {
        (Some(0), true) | (None, true) => order.len() - 1,
        (Some(index), true) => index - 1,
        (Some(index), false) if index + 1 == order.len() => 0,
        (Some(index), false) => index + 1,
        (None, false) => 0,
    };
    order.get(next).copied()
}

/// Resolves keyboard precedence before assigning a key operation.
pub fn target(editing: bool, dropdown_open: bool) -> KeyTarget {
    model::key_target(editing, dropdown_open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use taskbar_dock::describe::Setting;

    /// Checks control selection for every description family and recursive wrapper.
    #[test]
    fn selects_controls_for_kinds() {
        assert_eq!(control(&Kind::Switch), Control::Switch);
        assert_eq!(
            control(&Kind::Choice {
                options: vec![(json!(1), "one".into()), (json!(2), "two".into())],
                other: false
            }),
            Control::Segments
        );
        assert_eq!(
            control(&Kind::Choice {
                options: vec![(json!(1), "one".into()), (json!(2), "two".into())],
                other: true
            }),
            Control::Dropdown
        );
        assert_eq!(
            control(&Kind::Choice {
                options: vec![
                    (json!(1), "one".into()),
                    (json!(2), "two".into()),
                    (json!(3), "three".into()),
                    (json!(4), "four".into())
                ],
                other: true
            }),
            Control::Dropdown
        );
        assert_eq!(
            control(&Kind::Number {
                min: 0.0,
                max: 1.0,
                step: 0.1,
                integer: false,
                unit: String::new()
            }),
            Control::Slider
        );
        assert_eq!(
            control(&Kind::Optional(Box::new(Kind::Text {
                placeholder: String::new(),
                empty_ok: true
            }))),
            Control::Text
        );
        assert_eq!(
            control(&Kind::Auto {
                word: "auto".into(),
                label: "Auto".into(),
                inner: Box::new(Kind::Number {
                    min: 0.0,
                    max: 1.0,
                    step: 0.1,
                    integer: false,
                    unit: String::new()
                })
            }),
            Control::Auto
        );
    }

    /// Checks focus order, wraparound, reverse traversal, and reset visibility.
    #[test]
    fn focus_order_wraps_and_reverses_with_visible_resets() {
        let mut row = Row::new(
            "w",
            Setting::new("x", "X", Kind::Switch),
            json!(true),
            json!(false),
        );
        row.enabled = true;
        let focus = focus_order(2, &[row], 2);
        assert_eq!(
            focus.iter().map(|f| f.kind).collect::<Vec<_>>(),
            [
                FocusKind::Navigation,
                FocusKind::Navigation,
                FocusKind::Control,
                FocusKind::Reset,
                FocusKind::Action,
                FocusKind::Action
            ]
        );
        assert_eq!(next_focus(&focus, None, false), focus.first().copied());
        assert_eq!(
            next_focus(&focus, focus.last().copied(), false),
            focus.first().copied()
        );
        assert_eq!(
            next_focus(&focus, focus.first().copied(), true),
            focus.last().copied()
        );
        for pair in focus.windows(2) {
            assert_eq!(next_focus(&focus, Some(pair[0]), false), Some(pair[1]));
            assert_eq!(next_focus(&focus, Some(pair[1]), true), Some(pair[0]));
        }
        let unchanged = Row::new(
            "w",
            Setting::new("same", "Same", Kind::Switch),
            json!(false),
            json!(false),
        );
        let order = focus_order(1, &[unchanged], 0);
        assert_eq!(order.len(), 2);
        assert_eq!(order[1].kind, FocusKind::Control);
    }

    #[test]
    fn developer_actions_follow_advanced_rows_in_order() {
        let row = Row::new(
            "w",
            Setting::new("x", "X", Kind::Switch),
            json!(true),
            json!(false),
        );
        let order = developer_focus_order(2, &[row], 4);
        assert_eq!(order[0].kind, FocusKind::Navigation);
        assert_eq!(order[2].kind, FocusKind::Control);
        assert_eq!(order[3].kind, FocusKind::Reset);
        assert_eq!(
            order[4..]
                .iter()
                .map(|focus| (focus.kind, focus.index))
                .collect::<Vec<_>>(),
            (0..4)
                .map(|index| (FocusKind::DeveloperAction, index))
                .collect::<Vec<_>>()
        );
    }

    /// Checks the text, dropdown, and page precedence table.
    #[test]
    fn keyboard_precedence_table() {
        assert_eq!(target(true, true), KeyTarget::Text);
        assert_eq!(target(true, false), KeyTarget::Text);
        assert_eq!(target(false, true), KeyTarget::Dropdown);
        assert_eq!(target(false, false), KeyTarget::Page);
    }
}
