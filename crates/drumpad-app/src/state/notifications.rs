//! Toasts, modal dialogs, and the recovery actions they offer.
//!
//! Every error surfaced by the app goes through [`Notifications::error`]. The
//! constructor requires at least one [`DialogAction`], so an error dialog with
//! no way forward does not type-check. Toasts are transient; dialogs block the
//! view until the user picks an action.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a toast stays on screen.
pub const TOAST_LIFETIME: Duration = Duration::from_secs(5);

/// Severity of a toast or dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational.
    Info,
    /// Something worked, but is worth noticing.
    Success,
    /// Something is degraded but the app can continue.
    Warning,
    /// Something failed.
    Error,
}

/// A concrete next step the user can take from a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogIntent {
    /// Retry the failed operation.
    Retry,
    /// Open the settings view on the relevant page.
    OpenSettings,
    /// Run the missing-sample relink flow.
    RelinkSamples,
    /// Reload the kit from disk.
    ReloadKit,
    /// Open the log directory in Explorer.
    OpenLogFolder,
    /// Open the crash directory in Explorer.
    OpenCrashFolder,
    /// Keep the current kit and discard unsaved changes.
    DiscardChanges,
    /// Save the current kit, then continue.
    SaveAndContinue,
    /// Cancel the action that raised the dialog.
    Cancel,
    /// Close the dialog and keep running.
    Dismiss,
}

impl DialogIntent {
    /// Default button label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Retry => "Retry",
            Self::OpenSettings => "Open settings",
            Self::RelinkSamples => "Relink samples",
            Self::ReloadKit => "Reload kit",
            Self::OpenLogFolder => "Open log folder",
            Self::OpenCrashFolder => "Open crash folder",
            Self::DiscardChanges => "Discard changes",
            Self::SaveAndContinue => "Save and continue",
            Self::Cancel => "Cancel",
            Self::Dismiss => "Dismiss",
        }
    }

    /// Whether choosing this intent closes the dialog without further work.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Dismiss | Self::Cancel | Self::DiscardChanges)
    }
}

/// One button in a dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogAction {
    /// What the button does.
    pub intent: DialogIntent,
    /// Button label, defaulting to [`DialogIntent::label`].
    pub label: String,
}

impl DialogAction {
    /// Build an action with the default label.
    pub fn new(intent: DialogIntent) -> Self {
        Self {
            intent,
            label: intent.label().to_owned(),
        }
    }

    /// Build an action with a custom label.
    pub fn labelled(intent: DialogIntent, label: impl Into<String>) -> Self {
        Self {
            intent,
            label: label.into(),
        }
    }
}

/// A blocking dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    /// Correlation id for the caller.
    pub id: u64,
    /// Severity controls colour and icon.
    pub severity: Severity,
    /// Short title.
    pub title: String,
    /// Explanation, which should name the failing object.
    pub message: String,
    /// Buttons, never empty.
    pub actions: Vec<DialogAction>,
    /// Whether the dialog may be closed with Escape. Errors generally can be,
    /// destructive confirmations cannot.
    pub dismissible: bool,
}

impl Dialog {
    /// An error dialog. At least one action is required by the type system.
    pub fn error(
        title: impl Into<String>,
        message: impl Into<String>,
        action: DialogAction,
    ) -> Self {
        Self {
            id: 0,
            severity: Severity::Error,
            title: title.into(),
            message: message.into(),
            actions: vec![action],
            dismissible: true,
        }
    }

    /// Add another action.
    pub fn with_action(mut self, action: DialogAction) -> Self {
        self.actions.push(action);
        self
    }

    /// Make the dialog non-dismissible (a confirmation that needs a decision).
    pub fn blocking(mut self) -> Self {
        self.dismissible = false;
        self
    }

    /// The primary action, drawn first and focused.
    pub fn primary(&self) -> &DialogAction {
        &self.actions[0]
    }

    /// Find an action by intent.
    pub fn action(&self, intent: DialogIntent) -> Option<&DialogAction> {
        self.actions.iter().find(|action| action.intent == intent)
    }
}

/// A transient message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// Correlation id.
    pub id: u64,
    /// Severity.
    pub severity: Severity,
    /// Message text.
    pub message: String,
    /// Time on screen.
    pub age: Duration,
}

impl Toast {
    /// Remaining fraction of the toast lifetime, `0..=1`.
    pub fn remaining(&self) -> f32 {
        let lifetime = TOAST_LIFETIME.as_secs_f32();
        if lifetime <= 0.0 {
            return 0.0;
        }
        (1.0 - self.age.as_secs_f32() / lifetime).clamp(0.0, 1.0)
    }
}

/// Toast and dialog queue.
#[derive(Debug, Clone, Default)]
pub struct Notifications {
    toasts: Vec<Toast>,
    dialog: Option<Dialog>,
    next_id: u64,
}

impl Notifications {
    /// Empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Show a transient toast.
    pub fn toast(&mut self, severity: Severity, message: impl Into<String>) -> u64 {
        let id = self.id();
        self.toasts.push(Toast {
            id,
            severity,
            message: message.into(),
            age: Duration::ZERO,
        });
        // Keep the on-screen queue short so it cannot cover the pad grid.
        const MAX_TOASTS: usize = 4;
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        id
    }

    /// Show an informational toast.
    pub fn info(&mut self, message: impl Into<String>) -> u64 {
        self.toast(Severity::Info, message)
    }

    /// Show a success toast.
    pub fn success(&mut self, message: impl Into<String>) -> u64 {
        self.toast(Severity::Success, message)
    }

    /// Show a warning toast.
    pub fn warning(&mut self, message: impl Into<String>) -> u64 {
        self.toast(Severity::Warning, message)
    }

    /// Show an error dialog with a concrete next action.
    pub fn error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        action: DialogAction,
    ) -> u64 {
        let mut dialog = Dialog::error(title, message, action);
        dialog.id = self.id();
        let id = dialog.id;
        self.dialog = Some(dialog);
        id
    }

    /// Show a prepared dialog.
    pub fn show(&mut self, mut dialog: Dialog) -> u64 {
        dialog.id = self.id();
        let id = dialog.id;
        self.dialog = Some(dialog);
        id
    }

    /// The current dialog.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Add an action to the open dialog, if any.
    pub fn add_dialog_action(&mut self, action: DialogAction) {
        if let Some(dialog) = &mut self.dialog
            && !dialog.actions.iter().any(|existing| existing.intent == action.intent)
        {
            dialog.actions.push(action);
        }
    }

    /// Close the dialog. Escape and the close button both land here; callers
    /// must check [`Dialog::dismissible`] before calling it for Escape.
    pub fn close_dialog(&mut self) {
        self.dialog = None;
    }

    /// The current toasts.
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Advance toast ages and drop expired ones.
    pub fn tick(&mut self, dt: Duration) {
        for toast in &mut self.toasts {
            toast.age += dt;
        }
        self.toasts.retain(|toast| toast.age < TOAST_LIFETIME);
    }

    /// Whether a toast animation needs another frame.
    pub fn is_animating(&self) -> bool {
        !self.toasts.is_empty()
    }

    /// Dismiss one toast.
    pub fn dismiss_toast(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_dialog_has_at_least_one_concrete_action() {
        let dialog = Dialog::error("Audio device lost", "The output device was unplugged.", DialogAction::new(DialogIntent::OpenSettings));
        assert!(!dialog.actions.is_empty());
        assert_eq!(dialog.primary().intent, DialogIntent::OpenSettings);
        assert!(!dialog.primary().label.is_empty());
    }

    #[test]
    fn a_dialog_can_offer_a_primary_and_a_secondary_path() {
        let dialog = Dialog::error(
            "Missing samples",
            "kick.wav could not be found.",
            DialogAction::new(DialogIntent::RelinkSamples),
        )
        .with_action(DialogAction::labelled(DialogIntent::Dismiss, "Play anyway"));

        assert_eq!(dialog.actions.len(), 2);
        assert!(dialog.action(DialogIntent::RelinkSamples).is_some());
        assert_eq!(
            dialog.action(DialogIntent::Dismiss).expect("dismiss").label,
            "Play anyway"
        );
    }

    #[test]
    fn destructive_prompts_can_refuse_escape() {
        let dialog = Dialog::error(
            "Discard changes?",
            "The kit has unsaved edits.",
            DialogAction::new(DialogIntent::SaveAndContinue),
        )
        .with_action(DialogAction::new(DialogIntent::DiscardChanges))
        .with_action(DialogAction::new(DialogIntent::Cancel))
        .blocking();
        assert!(!dialog.dismissible);
    }

    #[test]
    fn the_error_helper_assigns_an_id_and_replaces_the_open_dialog() {
        let mut notifications = Notifications::new();
        let first = notifications.error(
            "First",
            "one",
            DialogAction::new(DialogIntent::Dismiss),
        );
        let second = notifications.error(
            "Second",
            "two",
            DialogAction::new(DialogIntent::Retry),
        );
        assert_ne!(first, second);
        assert_eq!(notifications.dialog().expect("dialog").id, second);
        assert_eq!(notifications.dialog().expect("dialog").title, "Second");
    }

    #[test]
    fn adding_a_duplicate_intent_is_a_no_op() {
        let mut notifications = Notifications::new();
        notifications.error("T", "m", DialogAction::new(DialogIntent::Retry));
        notifications.add_dialog_action(DialogAction::new(DialogIntent::Retry));
        assert_eq!(notifications.dialog().expect("dialog").actions.len(), 1);
        notifications.add_dialog_action(DialogAction::new(DialogIntent::OpenSettings));
        assert_eq!(notifications.dialog().expect("dialog").actions.len(), 2);
    }

    #[test]
    fn toasts_expire_after_their_lifetime() {
        let mut notifications = Notifications::new();
        notifications.info("Imported kick.wav");
        assert_eq!(notifications.toasts().len(), 1);
        assert!(notifications.is_animating());
        notifications.tick(TOAST_LIFETIME / 2);
        assert!(notifications.toasts()[0].remaining() < 1.0);
        notifications.tick(TOAST_LIFETIME);
        assert!(notifications.toasts().is_empty());
        assert!(!notifications.is_animating());
    }

    #[test]
    fn the_toast_queue_is_bounded() {
        let mut notifications = Notifications::new();
        for index in 0..10 {
            notifications.info(format!("toast {index}"));
        }
        assert_eq!(notifications.toasts().len(), 4);
        assert_eq!(notifications.toasts().last().expect("last").message, "toast 9");
    }

    #[test]
    fn a_toast_can_be_dismissed_by_id() {
        let mut notifications = Notifications::new();
        let first = notifications.info("first");
        let second = notifications.info("second");
        notifications.dismiss_toast(first);
        assert_eq!(notifications.toasts().len(), 1);
        assert_eq!(notifications.toasts()[0].id, second);
    }

    #[test]
    fn terminal_intents_are_the_ones_that_close_without_follow_up() {
        for intent in [DialogIntent::Dismiss, DialogIntent::Cancel, DialogIntent::DiscardChanges] {
            assert!(intent.is_terminal(), "{intent:?}");
        }
        for intent in [DialogIntent::Retry, DialogIntent::OpenSettings, DialogIntent::RelinkSamples] {
            assert!(!intent.is_terminal(), "{intent:?}");
        }
    }
}
