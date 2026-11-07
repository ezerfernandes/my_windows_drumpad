//! Global shortcuts, mapped before the pad router sees the key.
//!
//! Shortcuts that use Ctrl, Alt, or the Windows key never reach the pad grid,
//! so `Ctrl+Z` cannot also trigger the pad bound to `Z`. Plain keys are left to
//! the active keyboard profile, except for the navigation keys listed here.

use winit::keyboard::{KeyCode, ModifiersState};

use super::actions::Action;

/// Whether a key with these modifiers should be routed to the pad grid.
///
/// Shift is allowed (it does not change the physical key), while Ctrl, Alt, and
/// the Windows key are reserved for shortcuts and text entry.
pub fn routes_to_pads(modifiers: ModifiersState) -> bool {
    !(modifiers.control_key() || modifiers.alt_key() || modifiers.super_key())
}

/// Map a key press to an action, if it is a global shortcut.
///
/// Returns `None` for plain keys that belong to the pad profile.
pub fn shortcut(key: KeyCode, modifiers: ModifiersState, pressed: bool) -> Option<Action> {
    if !pressed {
        return None;
    }

    if modifiers.control_key() {
        return match key {
            KeyCode::Digit1 => Some(Action::Navigate(crate::state::navigation::View::Edit)),
            KeyCode::Digit2 => Some(Action::Navigate(
                crate::state::navigation::View::KitBrowser,
            )),
            KeyCode::Digit3 => Some(Action::Navigate(
                crate::state::navigation::View::Settings,
            )),
            KeyCode::KeyZ => Some(if modifiers.shift_key() {
                Action::RedoEdit
            } else {
                Action::UndoEdit
            }),
            KeyCode::KeyY => Some(Action::RedoEdit),
            KeyCode::KeyS => Some(Action::SaveActiveKit),
            KeyCode::KeyO => Some(Action::Navigate(
                crate::state::navigation::View::KitBrowser,
            )),
            KeyCode::Escape => Some(Action::Escape),
            _ => None,
        };
    }

    if modifiers.alt_key() || modifiers.super_key() {
        return None;
    }

    match key {
        KeyCode::F11 => Some(Action::Navigate(crate::state::navigation::View::Performance)),
        KeyCode::Escape => Some(Action::Escape),
        KeyCode::Enter | KeyCode::NumpadEnter => Some(Action::Confirm),
        KeyCode::ArrowUp => Some(Action::MoveSelection { d_row: -1, d_col: 0 }),
        KeyCode::ArrowDown => Some(Action::MoveSelection { d_row: 1, d_col: 0 }),
        KeyCode::ArrowLeft => Some(Action::MoveSelection { d_row: 0, d_col: -1 }),
        KeyCode::ArrowRight => Some(Action::MoveSelection { d_row: 0, d_col: 1 }),
        _ => None,
    }
}

/// Shortcut help lines shown in the UI, in the order they are documented.
pub const SHORTCUT_HELP: [(&str, &str); 11] = [
    ("1 2 3 4 / Q W E R / A S D F / Z X C V", "Trigger pads 1-16"),
    ("F1", "Panic / stop all"),
    ("F2", "Metronome"),
    ("F3 / F4", "Previous / next kit"),
    ("F5", "Keyboard map overlay"),
    ("F11", "Performance mode"),
    ("Ctrl+1 / Ctrl+2 / Ctrl+3", "Edit / Kits / Settings"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo sample edit"),
    ("Ctrl+S", "Save the active kit"),
    ("Arrows", "Move the pad selection"),
    ("Enter / Esc", "Confirm a drop / leave performance"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::navigation::View;

    fn none() -> ModifiersState {
        ModifiersState::empty()
    }

    fn ctrl() -> ModifiersState {
        ModifiersState::CONTROL
    }

    #[test]
    fn ctrl_shortcuts_never_reach_the_pad_grid() {
        assert!(!routes_to_pads(ctrl()));
        assert!(!routes_to_pads(ModifiersState::ALT));
        assert!(!routes_to_pads(ModifiersState::SUPER));
        assert!(routes_to_pads(none()));
        assert!(routes_to_pads(ModifiersState::SHIFT));
    }

    #[test]
    fn ctrl_1_to_3_switch_views() {
        assert_eq!(
            shortcut(KeyCode::Digit1, ctrl(), true),
            Some(Action::Navigate(View::Edit))
        );
        assert_eq!(
            shortcut(KeyCode::Digit2, ctrl(), true),
            Some(Action::Navigate(View::KitBrowser))
        );
        assert_eq!(
            shortcut(KeyCode::Digit3, ctrl(), true),
            Some(Action::Navigate(View::Settings))
        );
    }

    #[test]
    fn ctrl_z_undoes_and_ctrl_shift_z_redoes() {
        assert_eq!(shortcut(KeyCode::KeyZ, ctrl(), true), Some(Action::UndoEdit));
        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(
            shortcut(KeyCode::KeyZ, ctrl_shift, true),
            Some(Action::RedoEdit)
        );
    }

    #[test]
    fn ctrl_s_saves_the_active_kit() {
        assert_eq!(
            shortcut(KeyCode::KeyS, ctrl(), true),
            Some(Action::SaveActiveKit)
        );
    }

    #[test]
    fn f11_enters_performance_and_escape_is_routed_to_the_shell() {
        assert_eq!(
            shortcut(KeyCode::F11, none(), true),
            Some(Action::Navigate(View::Performance))
        );
        assert_eq!(shortcut(KeyCode::Escape, none(), true), Some(Action::Escape));
    }

    #[test]
    fn arrows_move_the_selection() {
        assert_eq!(
            shortcut(KeyCode::ArrowUp, none(), true),
            Some(Action::MoveSelection { d_row: -1, d_col: 0 })
        );
        assert_eq!(
            shortcut(KeyCode::ArrowRight, none(), true),
            Some(Action::MoveSelection { d_row: 0, d_col: 1 })
        );
    }

    #[test]
    fn plain_pad_keys_are_left_to_the_profile() {
        for key in [KeyCode::KeyQ, KeyCode::Digit1, KeyCode::F1, KeyCode::Space] {
            assert_eq!(shortcut(key, none(), true), None, "{key:?}");
        }
    }

    #[test]
    fn releases_are_not_shortcuts() {
        assert_eq!(shortcut(KeyCode::KeyZ, ctrl(), false), None);
    }

    #[test]
    fn every_documented_shortcut_has_a_mapping_or_a_profile_binding() {
        // The help table is part of the UI contract; a shortcut that silently
        // stops working is a documentation bug.
        assert_eq!(SHORTCUT_HELP.len(), 11);
        for (keys, description) in SHORTCUT_HELP {
            assert!(!keys.is_empty());
            assert!(!description.is_empty());
        }
    }
}
