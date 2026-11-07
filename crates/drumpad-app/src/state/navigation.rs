//! View navigation and the state-preserving view stack.
//!
//! Only *which* view is visible lives here. Every other piece of state (pads,
//! selection, open kit, unsaved edits) is owned by [`super::AppState`] and is
//! never rebuilt when the user navigates, which is what makes "state survives
//! navigation" true by construction rather than by discipline.

use serde::{Deserialize, Serialize};

/// A top-level screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// Stage surface: large pads, transport row, no editing chrome.
    Performance,
    /// Pad grid plus inspector.
    Edit,
    /// Kit list and kit operations.
    KitBrowser,
    /// Audio, input, appearance, and diagnostics.
    Settings,
}

impl View {
    /// Views shown in the top bar, in tab order.
    pub const ALL: [Self; 4] = [
        Self::Performance,
        Self::Edit,
        Self::KitBrowser,
        Self::Settings,
    ];

    /// Tab label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Performance => "Performance",
            Self::Edit => "Edit",
            Self::KitBrowser => "Kits",
            Self::Settings => "Settings",
        }
    }

    /// Keyboard shortcut that jumps to this view.
    pub const fn shortcut(self) -> &'static str {
        match self {
            Self::Performance => "F11",
            Self::Edit => "Ctrl+1",
            Self::KitBrowser => "Ctrl+2",
            Self::Settings => "Ctrl+3",
        }
    }

    /// The Edit view is the only safe place to change pad settings.
    pub const fn allows_edits(self) -> bool {
        matches!(self, Self::Edit)
    }
}

/// Why the current view was entered. Performance mode is an overlay: leaving it
/// must return to the view the user came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    view: View,
    return_to: Option<View>,
}

/// The navigation stack.
#[derive(Debug, Clone)]
pub struct Navigation {
    current: Frame,
    /// Set when the user asks for Performance but editing has unsaved changes.
    pending_view: Option<View>,
}

impl Default for Navigation {
    fn default() -> Self {
        Self::new(View::Performance)
    }
}

impl Navigation {
    /// Start on `view`.
    pub fn new(view: View) -> Self {
        Self {
            current: Frame {
                view,
                return_to: None,
            },
            pending_view: None,
        }
    }

    /// The visible view.
    pub fn current(&self) -> View {
        self.current.view
    }

    /// Whether performance mode is active.
    pub fn is_performance(&self) -> bool {
        self.current.view == View::Performance
    }

    /// Navigate to `view`.
    ///
    /// Switching views never mutates pads, kits, or the editor. Entering
    /// Performance remembers where to return to; leaving it restores that view.
    pub fn go(&mut self, view: View) {
        if view == self.current.view {
            self.pending_view = None;
            return;
        }

        if view == View::Performance {
            self.current = Frame {
                view,
                return_to: Some(self.current.view),
            };
        } else {
            let returned = if self.current.view == View::Performance {
                self.current.return_to
            } else {
                None
            };
            self.current = Frame {
                view,
                return_to: returned,
            };
        }
        self.pending_view = None;
    }

    /// Leave performance mode and restore the previous view.
    ///
    /// Returns the view that became visible.
    pub fn exit_performance(&mut self) -> View {
        if !self.is_performance() {
            return self.current.view;
        }
        let view = self
            .current
            .return_to
            .unwrap_or(View::Edit);
        self.current = Frame {
            view,
            return_to: None,
        };
        self.pending_view = None;
        view
    }

    /// Ask for `view` but keep the current one when `blocked` is true.
    ///
    /// The kit browser and settings use this to run an unsaved-changes prompt
    /// before leaving the editor.
    pub fn request_with_guard(&mut self, view: View, blocked: bool) -> Option<View> {
        if blocked {
            self.pending_view = Some(view);
            None
        } else {
            self.go(view);
            Some(view)
        }
    }

    /// The view that was waiting behind a guard, if any.
    pub fn pending_view(&self) -> Option<View> {
        self.pending_view
    }

    /// Confirm the pending navigation.
    pub fn confirm_pending(&mut self) -> Option<View> {
        let view = self.pending_view.take()?;
        self.go(view);
        Some(view)
    }

    /// Discard the pending navigation.
    pub fn cancel_pending(&mut self) {
        self.pending_view = None;
    }

    /// Cycle to the next view in tab order (Ctrl+Tab).
    pub fn next_view(&mut self) {
        let index = View::ALL
            .iter()
            .position(|view| *view == self.current.view)
            .unwrap_or(0);
        let next = View::ALL[(index + 1) % View::ALL.len()];
        self.go(next);
    }

    /// Cycle to the previous view in tab order.
    pub fn previous_view(&mut self) {
        let index = View::ALL
            .iter()
            .position(|view| *view == self.current.view)
            .unwrap_or(0);
        let previous = View::ALL[(index + View::ALL.len() - 1) % View::ALL.len()];
        self.go(previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_views_preserves_the_navigation_identity() {
        let mut navigation = Navigation::new(View::Edit);
        assert_eq!(navigation.current(), View::Edit);
        navigation.go(View::Settings);
        assert_eq!(navigation.current(), View::Settings);
        navigation.go(View::Performance);
        assert!(navigation.is_performance());
    }

    #[test]
    fn leaving_performance_restores_the_previous_view() {
        let mut navigation = Navigation::new(View::KitBrowser);
        navigation.go(View::Performance);
        assert_eq!(navigation.exit_performance(), View::KitBrowser);
        assert_eq!(navigation.current(), View::KitBrowser);
    }

    #[test]
    fn performance_first_launch_falls_back_to_edit() {
        let mut navigation = Navigation::new(View::Performance);
        assert_eq!(navigation.exit_performance(), View::Edit);
    }

    #[test]
    fn exiting_performance_when_not_in_performance_is_a_no_op() {
        let mut navigation = Navigation::new(View::Settings);
        assert_eq!(navigation.exit_performance(), View::Settings);
    }

    #[test]
    fn a_guard_blocks_navigation_until_confirmed() {
        let mut navigation = Navigation::new(View::Edit);
        assert_eq!(navigation.request_with_guard(View::KitBrowser, true), None);
        assert_eq!(navigation.current(), View::Edit);
        assert_eq!(navigation.pending_view(), Some(View::KitBrowser));

        assert_eq!(navigation.confirm_pending(), Some(View::KitBrowser));
        assert_eq!(navigation.current(), View::KitBrowser);
        assert_eq!(navigation.pending_view(), None);
    }

    #[test]
    fn cancelling_a_guarded_navigation_keeps_the_editor() {
        let mut navigation = Navigation::new(View::Edit);
        navigation.request_with_guard(View::Settings, true);
        navigation.cancel_pending();
        assert_eq!(navigation.current(), View::Edit);
        assert_eq!(navigation.pending_view(), None);
    }

    #[test]
    fn an_unguarded_request_switches_immediately() {
        let mut navigation = Navigation::new(View::Edit);
        assert_eq!(
            navigation.request_with_guard(View::Settings, false),
            Some(View::Settings)
        );
        assert_eq!(navigation.current(), View::Settings);
    }

    #[test]
    fn tab_cycling_wraps_in_both_directions() {
        let mut navigation = Navigation::new(View::Performance);
        navigation.next_view();
        assert_eq!(navigation.current(), View::Edit);
        navigation.previous_view();
        assert_eq!(navigation.current(), View::Performance);
        navigation.previous_view();
        assert_eq!(navigation.current(), View::Settings);
    }

    #[test]
    fn only_the_edit_view_allows_edits() {
        assert!(View::Edit.allows_edits());
        for view in [View::Performance, View::KitBrowser, View::Settings] {
            assert!(!view.allows_edits(), "{view:?} must not allow edits");
        }
    }

    #[test]
    fn every_view_documents_a_shortcut() {
        for view in View::ALL {
            assert!(!view.shortcut().is_empty());
            assert!(!view.label().is_empty());
        }
    }
}
