//! UI intents and their dispatch into [`AppState`].
//!
//! Every widget emits an [`Action`]; [`dispatch`] applies it and returns the
//! [`Effect`]s the composition layer must perform (engine, kit service, disk).
//! Nothing here touches egui or winit, so a full interaction sequence can be
//! replayed in a unit test.

use std::path::PathBuf;

use crate::state::dragdrop::DropError;
use crate::state::editor::{EditParams, EditSession};
use crate::state::keymap::{ProfileId, TransportAction};
use crate::state::kit::{KitError, KitId, KitRequest};
use crate::state::navigation::View;
use crate::state::notifications::{DialogAction, DialogIntent};
use crate::state::pad::PadId;
use crate::state::performance::TransportEffect;
use crate::state::prefs::UiPreferences;
use crate::state::{AppState, Effect};

/// One user intent.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Trigger a pad.
    TriggerPad {
        /// Pad to play.
        pad: PadId,
        /// MIDI-style velocity `1..=127`.
        velocity: u8,
    },
    /// Release a gate-mode pad.
    ReleasePad(PadId),
    /// Run a transport action (F1-F5 or a transport button).
    Transport(TransportAction),
    /// Switch view.
    Navigate(View),
    /// Leave performance mode and restore the previous view.
    ExitPerformance,
    /// Select a pad for the inspector.
    SelectPad(PadId),
    /// Move the pad selection by grid steps.
    MoveSelection {
        /// Row delta.
        d_row: isize,
        /// Column delta.
        d_col: isize,
    },
    /// Switch keyboard profile.
    SetProfile(ProfileId),
    /// Change the pad surface size (4x4 or 3x3), live.
    SetGridSize(crate::state::prefs::GridSize),
    /// Search the kit list.
    SetKitSearch(String),
    /// Create a kit.
    CreateKit(String),
    /// Create a new kit from every supported audio file in a folder.
    CreateKitFromFolder {
        /// New kit name.
        name: String,
        /// Folder to scan.
        folder: PathBuf,
    },
    /// Duplicate a kit.
    DuplicateKit(KitId),
    /// Rename a kit.
    RenameKit {
        /// Kit to rename.
        kit: KitId,
        /// New name.
        name: String,
    },
    /// Delete a kit.
    DeleteKit(KitId),
    /// Load a kit.
    LoadKit(KitId),
    /// Load the next kit in set order.
    NextKit,
    /// Load the previous kit in set order.
    PreviousKit,
    /// Confirm the parked unsaved-changes action.
    ConfirmPendingKit,
    /// Cancel the parked unsaved-changes action.
    CancelPendingKit,
    /// Save the active kit.
    SaveActiveKit,
    /// Accept the inspector draft.
    CommitInspector,
    /// Discard inspector changes.
    RevertInspector,
    /// Reset the inspector draft to the empty-pad defaults.
    ResetInspector,
    /// Audition a pad with its current settings.
    PreviewPad {
        /// Pad to audition.
        pad: PadId,
        /// Audition velocity.
        velocity: u8,
    },
    /// Undo the last sample edit.
    UndoEdit,
    /// Redo the last undone sample edit.
    RedoEdit,
    /// Reset sample edits to the original.
    ResetEdit,
    /// Commit a sample parameter change.
    CommitEdit {
        /// History label.
        label: String,
        /// New parameters.
        params: Box<EditParams>,
    },
    /// Accept the settings draft and persist it.
    ApplySettings,
    /// Discard settings changes.
    RevertSettings,
    /// The wizard's Next button.
    WizardNext,
    /// The wizard's Back button.
    WizardBack,
    /// The wizard's Skip button.
    WizardSkip,
    /// The wizard's Finish button.
    WizardFinish,
    /// Reopen the wizard from Settings.
    WizardReopen,
    /// Close a reopened wizard.
    WizardClose,
    /// Begin a drag-and-drop preview.
    BeginDrop {
        /// Dropped paths.
        paths: Vec<PathBuf>,
        /// Pad under the cursor.
        target: Option<PadId>,
    },
    /// Assign the previewed drop.
    ConfirmDrop,
    /// Discard the previewed drop.
    CancelDrop,
    /// Undo the last confirmed drop.
    UndoDrop,
    /// Close the open dialog.
    DismissDialog,
    /// Choose a dialog action.
    ChooseDialog(DialogIntent),
    /// Escape: close a dialog or overlay, otherwise leave performance mode.
    Escape,
    /// Enter: confirm a drop, advance the wizard, or commit the inspector.
    Confirm,
    /// Report an error text from the shell.
    ReportError {
        /// Short title.
        title: String,
        /// Explanation.
        message: String,
    },
}

/// Apply an action, returning what the composition layer must do next.
pub fn dispatch(state: &mut AppState, action: Action) -> Vec<Effect> {
    match action {
        Action::TriggerPad { pad, velocity } => {
            state.pads.select(pad);
            state.pads.trigger(pad, velocity as f32 / 127.0);
            vec![Effect::Trigger { pad, velocity }]
        }
        Action::ReleasePad(pad) => vec![Effect::Release { pad }],
        Action::Transport(action) => dispatch_transport(state, action),
        Action::Navigate(view) => {
            if state.navigation.current() == view {
                return vec![];
            }
            let blocked = state.navigation.current() == View::Edit
                && state.kits.has_unsaved_changes()
                && view != View::Performance;
            if state.navigation.request_with_guard(view, blocked).is_none() && blocked {
                ask_unsaved_changes(state, DialogIntent::DiscardChanges);
            }
            vec![]
        }
        Action::ExitPerformance => {
            state.navigation.exit_performance();
            vec![]
        }
        Action::SelectPad(pad) => {
            state.select_pad(pad);
            vec![]
        }
        Action::MoveSelection { d_row, d_col } => {
            let grid = state.grid_size();
            state.pads.move_selection_in(grid, d_row, d_col);
            state.select_pad(state.pads.selected());
            vec![]
        }
        Action::SetGridSize(grid) => {
            state.set_grid_size(grid);
            vec![]
        }
        Action::SetProfile(id) => {
            if state.set_profile(id) {
                vec![]
            } else {
                state
                    .notifications
                    .warning(format!("Keyboard profile {id:?} is not installed."));
                vec![]
            }
        }
        Action::SetKitSearch(search) => {
            state.kits.set_search(search);
            vec![]
        }
        Action::CreateKit(name) => match state.kits.create(name) {
            Ok(request) => vec![Effect::Kit(request)],
            Err(error) => report_kit_error(state, &error),
        },
        Action::CreateKitFromFolder { name, folder } => {
            if name.trim().is_empty() {
                state
                    .notifications
                    .warning("Give the new kit a name first.");
                vec![]
            } else if !folder.is_dir() {
                state.notifications.warning(format!(
                    "{} is not a folder. Enter the path to a sample folder.",
                    folder.display()
                ));
                vec![]
            } else {
                vec![Effect::ImportKit { name, folder }]
            }
        }
        Action::DuplicateKit(kit) => vec![Effect::DuplicateKit(kit)],
        Action::RenameKit { kit, name } => match state.kits.rename(kit, name) {
            Ok(request) => vec![Effect::Kit(request)],
            Err(error) => report_kit_error(state, &error),
        },
        Action::DeleteKit(kit) => match state.kits.request_delete(kit) {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => {
                if state.kits.pending().is_some() {
                    ask_unsaved_changes(state, DialogIntent::DiscardChanges);
                }
                vec![]
            }
            Err(error) => report_kit_error(state, &error),
        },
        Action::LoadKit(kit) => match state.kits.request_switch(kit) {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => {
                if state.kits.pending().is_some() {
                    ask_unsaved_changes(state, DialogIntent::DiscardChanges);
                }
                vec![]
            }
            Err(error) => report_kit_error(state, &error),
        },
        Action::NextKit => match state.kits.next_kit() {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => vec![],
            Err(error) => report_kit_error(state, &error),
        },
        Action::PreviousKit => match state.kits.previous_kit() {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => vec![],
            Err(error) => report_kit_error(state, &error),
        },
        Action::ConfirmPendingKit => match state.kits.confirm_pending() {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => {
                state.notifications.close_dialog();
                vec![]
            }
            Err(error) => report_kit_error(state, &error),
        },
        Action::CancelPendingKit => {
            state.kits.cancel_pending();
            state.notifications.close_dialog();
            vec![]
        }
        Action::SaveActiveKit => match state.kits.active() {
            Some(kit) => {
                state.kits.set_dirty(false);
                state.notifications.success("Kit saved.");
                vec![Effect::Kit(KitRequest::Save(kit))]
            }
            None => vec![],
        },
        Action::CommitInspector => match state.commit_inspector() {
            Ok(pad) => {
                state
                    .notifications
                    .success(format!("Pad {} updated.", pad.number()));
                vec![Effect::PadSettingsChanged(pad)]
            }
            Err(error) => {
                state.report_inspector_error(&error);
                vec![]
            }
        },
        Action::RevertInspector => {
            state.revert_inspector();
            vec![]
        }
        Action::ResetInspector => {
            state.inspector.reset_draft();
            vec![]
        }
        Action::PreviewPad { pad, velocity } => {
            let preview = state.preview_trigger(pad, velocity);
            state.pads.trigger(pad, velocity as f32 / 127.0);
            match preview {
                Some(_) => vec![Effect::Trigger { pad, velocity }],
                None => {
                    state.notifications.warning(format!(
                        "Pad {} has no layer covering velocity {velocity}.",
                        pad.number()
                    ));
                    vec![]
                }
            }
        }
        Action::UndoEdit => match state.undo_edit() {
            Some(label) => {
                state.notifications.info(format!("Undid {label}"));
                vec![Effect::WaveformChanged]
            }
            None => vec![],
        },
        Action::RedoEdit => match state.redo_edit() {
            Some(label) => {
                state.notifications.info(format!("Redid {label}"));
                vec![Effect::WaveformChanged]
            }
            None => vec![],
        },
        Action::ResetEdit => {
            let Some(editor) = state.editor.as_mut() else {
                return vec![];
            };
            match editor.reset() {
                Ok(()) => vec![Effect::WaveformChanged],
                Err(error) => {
                    state.notifications.error(
                        "Reset failed",
                        error.to_string(),
                        DialogAction::new(DialogIntent::Dismiss),
                    );
                    vec![]
                }
            }
        }
        Action::CommitEdit { label, params } => {
            let Some(editor) = state.editor.as_mut() else {
                return vec![];
            };
            match editor.commit(label.clone(), *params) {
                Ok(()) => {
                    state.notifications.info(label);
                    vec![Effect::WaveformChanged]
                }
                Err(error) => {
                    state.notifications.error(
                        "Edit rejected",
                        error.to_string(),
                        DialogAction::labelled(DialogIntent::Dismiss, "Fix the value"),
                    );
                    vec![]
                }
            }
        }
        Action::ApplySettings => match state.settings.apply() {
            Ok(preferences) => {
                let preferences = preferences.clone();
                let reasons = state.settings.restart_reasons();
                state.notifications.success("Settings saved.");
                let mut effects = vec![Effect::PersistPreferences(Box::new(preferences))];
                if !reasons.is_empty() {
                    state
                        .notifications
                        .info(format!("Restart required for {} change(s).", reasons.len()));
                }
                // A new scale takes effect immediately in the next frame.
                effects.push(Effect::PreferencesChanged);
                effects
            }
            Err(error) => {
                state.notifications.error(
                    "Settings not applied",
                    error.to_string(),
                    DialogAction::labelled(DialogIntent::Dismiss, "Fix the value"),
                );
                vec![]
            }
        },
        Action::RevertSettings => {
            state.settings.revert();
            vec![]
        }
        Action::WizardNext => {
            let preferences = state.settings.draft().clone();
            let mut draft = preferences;
            state.wizard.next(&mut draft);
            state.settings.draft_mut().wizard_completed = draft.wizard_completed;
            finish_wizard_if_done(state)
        }
        Action::WizardBack => {
            state.wizard.back();
            vec![]
        }
        Action::WizardSkip => {
            let mut draft = state.settings.draft().clone();
            state.wizard.skip(&mut draft);
            *state.settings.draft_mut() = draft;
            finish_wizard_if_done(state)
        }
        Action::WizardFinish => {
            let mut draft = state.settings.draft().clone();
            state.wizard.finish(&mut draft);
            *state.settings.draft_mut() = draft;
            persist_wizard(state)
        }
        Action::WizardReopen => {
            state.wizard.reopen();
            vec![]
        }
        Action::WizardClose => {
            state.wizard.close();
            vec![]
        }
        Action::BeginDrop { paths, target } => {
            match state.dragdrop.begin(&paths, &state.pads, target) {
                Ok(preview) => {
                    let count = preview.targets.len();
                    state.notifications.info(format!(
                        "Drop {} file(s) onto the pads? Press Enter to assign.",
                        count
                    ));
                }
                Err(DropError::NoSupportedFiles) => {
                    state.notifications.warning(
                        "No supported audio files in this drop. Use WAV, AIFF, FLAC, MP3, or OGG.",
                    );
                }
                Err(error) => {
                    state.notifications.warning(error.to_string());
                }
            }
            vec![]
        }
        Action::ConfirmDrop => match state.dragdrop.confirm(&mut state.pads) {
            Some(undo) => {
                state.notifications.success(format!(
                    "Assigned {} pad(s). Ctrl+Z to undo.",
                    undo.assigned.len()
                ));
                vec![]
            }
            None => vec![],
        },
        Action::CancelDrop => {
            state.dragdrop.cancel();
            vec![]
        }
        Action::UndoDrop => {
            match state.dragdrop.undo_assignment(&mut state.pads) {
                Some(pads) => {
                    state
                        .notifications
                        .info(format!("Restored {} pad(s).", pads.len()));
                }
                None => {
                    state.notifications.warning("There is no drop to undo.");
                }
            }
            vec![]
        }
        Action::DismissDialog => {
            state.notifications.close_dialog();
            vec![]
        }
        Action::Escape => {
            if let Some(dialog) = state.notifications.dialog()
                && dialog.dismissible
            {
                state.notifications.close_dialog();
                state.kits.cancel_pending();
                state.navigation.cancel_pending();
                return vec![];
            }
            if state.performance.overlay_visible() {
                state.performance.set_overlay(false);
                return vec![];
            }
            if state.navigation.is_performance() {
                state.navigation.exit_performance();
            }
            vec![]
        }
        Action::Confirm => {
            if state.dragdrop.is_confirming() {
                return dispatch(state, Action::ConfirmDrop);
            }
            if state.wizard.is_active() {
                return dispatch(state, Action::WizardNext);
            }
            if state.inspector.is_dirty() {
                return dispatch(state, Action::CommitInspector);
            }
            vec![]
        }
        Action::ChooseDialog(intent) => choose_dialog(state, intent),
        Action::ReportError { title, message } => {
            state.notifications.error(
                title,
                message,
                DialogAction::new(DialogIntent::OpenLogFolder),
            );
            vec![]
        }
    }
}

fn dispatch_transport(state: &mut AppState, action: TransportAction) -> Vec<Effect> {
    match state.transport(action) {
        TransportEffect::Panic => {
            state.performance.silence_metronome();
            vec![Effect::Panic, Effect::Metronome(false)]
        }
        TransportEffect::Metronome(on) => vec![Effect::Metronome(on)],
        TransportEffect::PreviousKit => match state.kits.previous_kit() {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => vec![],
            Err(error) => report_kit_error(state, &error),
        },
        TransportEffect::NextKit => match state.kits.next_kit() {
            Ok(Some(request)) => vec![Effect::Kit(request)],
            Ok(None) => vec![],
            Err(error) => report_kit_error(state, &error),
        },
        TransportEffect::Overlay(_) => vec![],
    }
}

fn report_kit_error(state: &mut AppState, error: &KitError) -> Vec<Effect> {
    state.report_kit_error(error);
    vec![]
}

fn ask_unsaved_changes(state: &mut AppState, continue_intent: DialogIntent) {
    let mut dialog = crate::state::notifications::Dialog::error(
        "Unsaved changes",
        "The current kit has edits that are not saved.",
        DialogAction::new(DialogIntent::SaveAndContinue),
    )
    .with_action(DialogAction::labelled(
        continue_intent,
        "Discard and continue",
    ))
    .with_action(DialogAction::new(DialogIntent::Cancel))
    .blocking();
    dialog.id = 0;
    state.notifications.show(dialog);
}

fn choose_dialog(state: &mut AppState, intent: DialogIntent) -> Vec<Effect> {
    match intent {
        DialogIntent::Retry => {
            if let Some(request) = state.kits.retry() {
                state.notifications.close_dialog();
                vec![Effect::Kit(request)]
            } else {
                state.notifications.close_dialog();
                vec![]
            }
        }
        DialogIntent::OpenSettings => {
            state.notifications.close_dialog();
            dispatch(state, Action::Navigate(View::Settings))
        }
        DialogIntent::RelinkSamples => {
            state.notifications.close_dialog();
            vec![Effect::RelinkSamples]
        }
        DialogIntent::ReloadKit => {
            let kit = state.kits.active();
            state.notifications.close_dialog();
            match kit {
                Some(kit) => vec![Effect::Kit(KitRequest::Load(kit))],
                None => vec![],
            }
        }
        DialogIntent::OpenLogFolder => {
            state.notifications.close_dialog();
            vec![Effect::OpenLogFolder]
        }
        DialogIntent::OpenCrashFolder => {
            state.notifications.close_dialog();
            vec![Effect::OpenCrashFolder]
        }
        DialogIntent::DiscardChanges => {
            state.notifications.close_dialog();
            state.navigation.confirm_pending();
            match state.kits.confirm_pending() {
                Ok(Some(request)) => vec![Effect::Kit(request)],
                Ok(None) => vec![],
                Err(error) => report_kit_error(state, &error),
            }
        }
        DialogIntent::SaveAndContinue => {
            state.notifications.close_dialog();
            let Some(kit) = state.kits.active() else {
                return vec![];
            };
            state.kits.set_dirty(false);
            vec![Effect::Kit(KitRequest::Save(kit))]
        }
        DialogIntent::Cancel | DialogIntent::Dismiss => {
            state.notifications.close_dialog();
            state.kits.cancel_pending();
            state.navigation.cancel_pending();
            vec![]
        }
    }
}

fn finish_wizard_if_done(state: &mut AppState) -> Vec<Effect> {
    if state.wizard.is_active() {
        vec![]
    } else {
        persist_wizard(state)
    }
}

fn persist_wizard(state: &mut AppState) -> Vec<Effect> {
    let preferences = state.settings.draft().clone();
    state.notifications.success("Setup complete.");
    vec![Effect::PersistPreferences(Box::new(preferences))]
}

/// Open an edit session for the selected pad's sample.
pub fn open_editor_for_sample(state: &mut AppState, session: EditSession) {
    state.open_editor(session);
}

/// Persist preferences through the same path the action layer uses, for the
/// shell to call from its effect runner.
pub fn preferences_path(config_dir: &std::path::Path) -> PathBuf {
    UiPreferences::path_in(config_dir)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;
    use crate::state::keymap::TransportAction;
    use crate::state::kit::KitRequest;
    use crate::state::prefs::UiPreferences;

    fn state() -> AppState {
        AppState::new(UiPreferences::default())
    }

    #[test]
    fn triggering_a_pad_flashes_it_and_asks_the_engine_to_play() {
        let mut state = state();
        let effects = dispatch(
            &mut state,
            Action::TriggerPad {
                pad: PadId::new(3),
                velocity: 100,
            },
        );
        assert_eq!(
            effects,
            vec![Effect::Trigger {
                pad: PadId::new(3),
                velocity: 100
            }]
        );
        assert_eq!(state.pads.get(PadId::new(3)).flash, 1.0);
        assert_eq!(state.pads.selected(), PadId::new(3));
    }

    #[test]
    fn f1_panics_and_silences_the_metronome() {
        let mut state = state();
        dispatch(&mut state, Action::Transport(TransportAction::Metronome));
        assert!(state.performance.metronome());
        let effects = dispatch(&mut state, Action::Transport(TransportAction::Panic));
        assert!(effects.contains(&Effect::Panic));
        assert!(effects.contains(&Effect::Metronome(false)));
        assert!(!state.performance.metronome());
        assert_eq!(state.performance.panic_requests(), 1);
    }

    #[test]
    fn a_pad_click_does_not_need_a_live_model_to_feedback() {
        let mut state = state();
        dispatch(&mut state, Action::SelectPad(PadId::new(6)));
        assert_eq!(state.pads.selected(), PadId::new(6));
        assert_eq!(state.inspector.pad(), PadId::new(6));
    }

    #[test]
    fn kit_create_surfaces_a_request_and_a_duplicate_name_is_a_toast() {
        let mut state = state();
        let effects = dispatch(&mut state, Action::CreateKit("Live".to_owned()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Kit(KitRequest::Create(_))]
        ));
        assert_eq!(state.kits.len(), 2);

        let effects = dispatch(&mut state, Action::CreateKit("live".to_owned()));
        assert!(effects.is_empty());
        assert!(!state.notifications.toasts().is_empty());
        assert_eq!(state.kits.len(), 2);
    }

    #[test]
    fn switching_kits_with_unsaved_changes_asks_before_discarding() {
        let mut state = state();
        dispatch(&mut state, Action::CreateKit("Two".to_owned()));
        let second = state
            .kits
            .kits()
            .iter()
            .find(|kit| kit.name == "Two")
            .expect("two")
            .id;
        state.kits.set_dirty(true);
        let effects = dispatch(&mut state, Action::LoadKit(second));
        assert!(effects.is_empty());
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(dialog.action(DialogIntent::DiscardChanges).is_some());
        assert!(!dialog.dismissible);
    }

    #[test]
    fn discarding_from_the_prompt_starts_the_kit_load() {
        let mut state = state();
        dispatch(&mut state, Action::CreateKit("Two".to_owned()));
        let second = state
            .kits
            .kits()
            .iter()
            .find(|kit| kit.name == "Two")
            .expect("two")
            .id;
        state.kits.set_dirty(true);
        dispatch(&mut state, Action::LoadKit(second));
        let effects = dispatch(
            &mut state,
            Action::ChooseDialog(DialogIntent::DiscardChanges),
        );
        assert_eq!(effects, vec![Effect::Kit(KitRequest::Load(second))]);
        assert!(state.notifications.dialog().is_none());
    }

    #[test]
    fn settings_apply_persists_and_reports_a_restart() {
        let mut state = state();
        state.settings.draft_mut().audio.sample_rate = 96_000;
        let effects = dispatch(&mut state, Action::ApplySettings);
        assert!(matches!(
            effects.first(),
            Some(Effect::PersistPreferences(_))
        ));
        assert!(effects.contains(&Effect::PreferencesChanged));
        assert!(
            state
                .notifications
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("saved"))
        );
    }

    #[test]
    fn invalid_settings_are_blocked_with_a_dialog() {
        let mut state = state();
        state.settings.draft_mut().appearance.scale = 4.0;
        let effects = dispatch(&mut state, Action::ApplySettings);
        assert!(effects.is_empty());
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(dialog.message.contains("UI scale"));
        assert!(dialog.action(DialogIntent::Dismiss).is_some());
    }

    #[test]
    fn the_wizard_finishes_and_persists() {
        let mut state = state();
        while state.wizard.is_active() {
            dispatch(&mut state, Action::WizardSkip);
        }
        assert!(state.settings.draft().wizard_completed);
    }

    #[test]
    fn a_drop_previews_then_assigns_and_undoes() {
        let mut state = state();
        dispatch(
            &mut state,
            Action::BeginDrop {
                paths: vec![PathBuf::from("kick.wav")],
                target: Some(PadId::new(2)),
            },
        );
        assert!(state.dragdrop.is_confirming());
        dispatch(&mut state, Action::ConfirmDrop);
        assert_eq!(
            state.pads.get(PadId::new(2)).sample.as_deref(),
            Some("kick.wav")
        );
        dispatch(&mut state, Action::UndoDrop);
        assert_eq!(state.pads.get(PadId::new(2)).sample, None);
    }

    #[test]
    fn an_unsupported_drop_is_refused_with_a_toast_and_no_pad_change() {
        let mut state = state();
        dispatch(
            &mut state,
            Action::BeginDrop {
                paths: vec![PathBuf::from("notes.txt")],
                target: Some(PadId::new(0)),
            },
        );
        assert!(!state.dragdrop.is_confirming());
        assert!(
            state
                .notifications
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("No supported"))
        );
        assert!(state.pads.iter().all(|pad| pad.sample.is_none()));
    }

    #[test]
    fn transport_next_kit_asks_the_kit_browser() {
        let mut state = state();
        dispatch(&mut state, Action::CreateKit("Two".to_owned()));
        let effects = dispatch(&mut state, Action::Transport(TransportAction::NextKit));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Kit(KitRequest::Load(_))]
        ));
    }

    #[test]
    fn create_kit_from_folder_emits_an_import_effect() {
        let temp = tempfile::tempdir().expect("temp");
        let mut state = state();
        let effects = dispatch(
            &mut state,
            Action::CreateKitFromFolder {
                name: "Imported".to_owned(),
                folder: temp.path().to_path_buf(),
            },
        );
        assert_eq!(
            effects,
            vec![Effect::ImportKit {
                name: "Imported".to_owned(),
                folder: temp.path().to_path_buf(),
            }]
        );
    }

    #[test]
    fn create_kit_from_folder_rejects_a_missing_folder_or_name() {
        let mut state = state();
        let effects = dispatch(
            &mut state,
            Action::CreateKitFromFolder {
                name: "Imported".to_owned(),
                folder: PathBuf::from("Z:\\definitely-missing"),
            },
        );
        assert!(effects.is_empty());
        assert!(!state.notifications.toasts().is_empty());

        let temp = tempfile::tempdir().expect("temp");
        let effects = dispatch(
            &mut state,
            Action::CreateKitFromFolder {
                name: "  ".to_owned(),
                folder: temp.path().to_path_buf(),
            },
        );
        assert!(effects.is_empty());
    }

    #[test]
    fn the_grid_size_action_switches_the_surface_live() {
        let mut state = state();
        assert_eq!(
            state.grid_size(),
            crate::state::prefs::GridSize::ThreeByThree
        );
        assert_eq!(state.pads.get(PadId::new(0)).key_label, "1");
        assert_eq!(state.pads.get(PadId::new(3)).key_label, "Q");

        dispatch(
            &mut state,
            Action::SetGridSize(crate::state::prefs::GridSize::FourByFour),
        );
        assert_eq!(state.pads.get(PadId::new(0)).key_label, "1");
        assert_eq!(state.pads.get(PadId::new(3)).key_label, "4");
        assert_eq!(state.router.profile().pad_count(), 16);

        dispatch(
            &mut state,
            Action::SetGridSize(crate::state::prefs::GridSize::ThreeByThree),
        );
        assert_eq!(state.pads.get(PadId::new(3)).key_label, "Q");
        assert_eq!(state.router.profile().pad_count(), 9);
    }
    #[test]
    fn selecting_a_profile_relabels_every_pad() {
        let mut state = state();
        dispatch(&mut state, Action::SetProfile(ProfileId(1)));
        assert_eq!(state.pads.get(PadId::new(0)).key_label, "1");
        assert_eq!(state.profile().name, "Full keyboard");
    }

    #[test]
    fn undo_and_redo_round_trip_through_actions() {
        use crate::state::editor::{EditParams, EditSession, SourceRef};
        let mut state = state();
        let session = EditSession::new(SourceRef {
            path: "kick.wav".into(),
            content_hash: "h".to_owned(),
            sample_rate: 48_000,
            channels: 1,
            frames: 48_000,
        });
        state.open_editor(session);
        let mut params = EditParams::default();
        params.gain_db = -3.0;
        let effects = dispatch(
            &mut state,
            Action::CommitEdit {
                label: "Gain".to_owned(),
                params: Box::new(params),
            },
        );
        assert!(effects.contains(&Effect::WaveformChanged));
        assert!(dispatch(&mut state, Action::UndoEdit).contains(&Effect::WaveformChanged));
        assert!(dispatch(&mut state, Action::RedoEdit).contains(&Effect::WaveformChanged));
    }

    #[test]
    fn an_error_dialog_offers_a_log_folder_action() {
        let mut state = state();
        dispatch(
            &mut state,
            Action::ReportError {
                title: "Unexpected error".to_owned(),
                message: "boom".to_owned(),
            },
        );
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(dialog.action(DialogIntent::OpenLogFolder).is_some());
        let effects = dispatch(
            &mut state,
            Action::ChooseDialog(DialogIntent::OpenLogFolder),
        );
        assert_eq!(effects, vec![Effect::OpenLogFolder]);
    }

    #[test]
    fn pad_release_and_pad_preview_route_to_the_engine() {
        let mut state = state();
        assert_eq!(
            dispatch(&mut state, Action::ReleasePad(PadId::new(4))),
            vec![Effect::Release { pad: PadId::new(4) }]
        );

        // A pad with no layers cannot be auditioned.
        let effects = dispatch(
            &mut state,
            Action::PreviewPad {
                pad: PadId::new(1),
                velocity: 100,
            },
        );
        assert!(effects.is_empty());
        assert!(
            state
                .notifications
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("no layer"))
        );

        // Give the selected pad a layer and audition it.
        state.inspector.draft_mut().layers = vec![crate::state::inspector::LayerSpec::full_range(
            Some("snare.wav".to_owned()),
        )];
        let effects = dispatch(
            &mut state,
            Action::PreviewPad {
                pad: PadId::new(0),
                velocity: 100,
            },
        );
        assert!(matches!(
            effects.as_slice(),
            [Effect::Trigger {
                velocity: 100,
                ..
            }] if matches!(effects.first(), Some(Effect::Trigger { pad, .. }) if pad.index() == 0)
        ));
    }

    #[test]
    fn navigation_guards_and_performance_exit_preserve_state() {
        let mut state = state();
        dispatch(&mut state, Action::Navigate(View::Settings));
        assert_eq!(state.navigation.current(), View::Settings);
        dispatch(&mut state, Action::Navigate(View::Performance));
        assert!(state.navigation.is_performance());
        dispatch(&mut state, Action::ExitPerformance);
        assert_eq!(state.navigation.current(), View::Settings);

        // A dirty editor parks the navigation behind the unsaved prompt.
        state.navigation.go(View::Edit);
        state.kits.set_dirty(true);
        dispatch(&mut state, Action::Navigate(View::KitBrowser));
        assert_eq!(state.navigation.current(), View::Edit);
        assert_eq!(state.navigation.pending_view(), Some(View::KitBrowser));
        dispatch(
            &mut state,
            Action::ChooseDialog(DialogIntent::DiscardChanges),
        );
        assert_eq!(state.navigation.current(), View::KitBrowser);
    }

    #[test]
    fn kit_search_rename_and_delete_emit_durable_requests() {
        let mut state = state();
        dispatch(&mut state, Action::SetKitSearch("drum".to_owned()));
        assert_eq!(state.kits.search(), "drum");

        dispatch(&mut state, Action::CreateKit("Rock".to_owned()));
        let rock = state
            .kits
            .kits()
            .iter()
            .find(|kit| kit.name == "Rock")
            .expect("rock")
            .id;
        let effects = dispatch(
            &mut state,
            Action::RenameKit {
                kit: rock,
                name: "Metal".to_owned(),
            },
        );
        assert_eq!(effects, vec![Effect::Kit(KitRequest::Rename(rock))]);

        let metal = state
            .kits
            .kits()
            .iter()
            .find(|kit| kit.name == "Metal")
            .expect("metal")
            .id;
        let effects = dispatch(&mut state, Action::DeleteKit(metal));
        assert_eq!(effects, vec![Effect::Kit(KitRequest::Delete(metal))]);
        assert!(!state.kits.kits().iter().any(|kit| kit.name == "Metal"));
    }

    #[test]
    fn deleting_the_active_dirty_kit_asks_before_discarding() {
        let mut state = state();
        dispatch(&mut state, Action::CreateKit("Two".to_owned()));
        let two = state
            .kits
            .kits()
            .iter()
            .find(|kit| kit.name == "Two")
            .expect("two")
            .id;
        dispatch(&mut state, Action::LoadKit(two));
        state.kits.finish_load(two, Ok(()));
        state.kits.set_dirty(true);

        let effects = dispatch(&mut state, Action::DeleteKit(two));
        assert!(effects.is_empty());
        assert!(state.notifications.dialog().is_some());
        assert_eq!(
            state.kits.pending(),
            Some(crate::state::kit::PendingAction::Delete(two))
        );

        let effects = dispatch(
            &mut state,
            Action::ChooseDialog(DialogIntent::DiscardChanges),
        );
        assert_eq!(effects, vec![Effect::Kit(KitRequest::Delete(two))]);
    }

    #[test]
    fn next_and_previous_kit_requests_wrap_the_catalogue() {
        let mut state = state();
        dispatch(&mut state, Action::CreateKit("Two".to_owned()));
        let effects = dispatch(&mut state, Action::NextKit);
        let loaded = match effects.as_slice() {
            [Effect::Kit(KitRequest::Load(id))] => *id,
            other => panic!("expected a kit load, got {other:?}"),
        };
        state.kits.finish_load(loaded, Ok(()));

        let effects = dispatch(&mut state, Action::PreviousKit);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Kit(KitRequest::Load(_))]
        ));
    }

    #[test]
    fn save_confirm_and_cancel_pending_kit_actions() {
        let mut state = state();
        let effects = dispatch(&mut state, Action::SaveActiveKit);
        assert_eq!(
            effects,
            vec![Effect::Kit(KitRequest::Save(
                state.kits.active().expect("active")
            ))]
        );
        assert!(!state.kits.has_unsaved_changes());

        let first = state.kits.active().expect("active");
        dispatch(&mut state, Action::CreateKit("Two".to_owned()));
        let two = state
            .kits
            .kits()
            .iter()
            .find(|kit| kit.name == "Two")
            .expect("two")
            .id;
        let effects = dispatch(&mut state, Action::LoadKit(two));
        assert_eq!(effects, vec![Effect::Kit(KitRequest::Load(two))]);
        state.kits.finish_load(two, Ok(()));

        state.kits.set_dirty(true);
        dispatch(&mut state, Action::LoadKit(first));
        assert!(state.kits.pending().is_some());
        dispatch(&mut state, Action::CancelPendingKit);
        assert_eq!(state.kits.pending(), None);

        state.kits.set_dirty(true);
        dispatch(&mut state, Action::LoadKit(first));
        let effects = dispatch(&mut state, Action::ConfirmPendingKit);
        assert_eq!(effects, vec![Effect::Kit(KitRequest::Load(first))]);
    }

    #[test]
    fn inspector_revert_reset_and_settings_revert_restore_drafts() {
        let mut state = state();
        state.inspector.draft_mut().volume_db = -12.0;
        dispatch(&mut state, Action::RevertInspector);
        assert_eq!(state.inspector.draft().volume_db, 0.0);

        state.inspector.draft_mut().sample = Some("kick.wav".to_owned());
        dispatch(&mut state, Action::ResetInspector);
        assert_eq!(state.inspector.draft().sample, None);

        state.settings.draft_mut().audio.sample_rate = 96_000;
        dispatch(&mut state, Action::RevertSettings);
        assert_eq!(state.settings.draft().audio.sample_rate, 48_000);
    }

    #[test]
    fn editor_actions_without_a_session_are_no_ops() {
        let mut state = state();
        assert!(dispatch(&mut state, Action::UndoEdit).is_empty());
        assert!(dispatch(&mut state, Action::RedoEdit).is_empty());
        assert!(dispatch(&mut state, Action::ResetEdit).is_empty());
        assert!(
            dispatch(
                &mut state,
                Action::CommitEdit {
                    label: "Gain".to_owned(),
                    params: Box::new(crate::state::editor::EditParams::default()),
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn wizard_back_reopen_close_and_finish() {
        let mut state = state();
        dispatch(&mut state, Action::WizardNext);
        assert_eq!(
            state.wizard.step(),
            crate::state::wizard::WizardStep::Keyboard
        );
        dispatch(&mut state, Action::WizardBack);
        assert_eq!(
            state.wizard.step(),
            crate::state::wizard::WizardStep::Welcome
        );
        dispatch(&mut state, Action::WizardSkip);
        assert_eq!(
            state.wizard.step(),
            crate::state::wizard::WizardStep::Keyboard
        );
        dispatch(&mut state, Action::WizardReopen);
        assert!(state.wizard.is_reopened());
        dispatch(&mut state, Action::WizardClose);
        assert!(!state.wizard.is_active());

        state.wizard.reopen();
        let effects = dispatch(&mut state, Action::WizardFinish);
        assert!(matches!(
            effects.as_slice(),
            [Effect::PersistPreferences(_)]
        ));
        assert!(!state.wizard.is_active());
        assert!(state.settings.draft().wizard_completed);
    }

    #[test]
    fn drop_confirm_cancel_and_undo_preserve_pad_state() {
        let mut state = state();
        let temp = tempfile::tempdir().expect("temp");
        dispatch(
            &mut state,
            Action::BeginDrop {
                paths: vec![temp.path().join("kick.wav")],
                target: Some(PadId::new(5)),
            },
        );
        assert!(state.dragdrop.is_confirming());
        dispatch(&mut state, Action::CancelDrop);
        assert!(!state.dragdrop.is_confirming());
        assert_eq!(state.pads.get(PadId::new(5)).sample, None);

        dispatch(
            &mut state,
            Action::BeginDrop {
                paths: vec![temp.path().join("kick.wav")],
                target: Some(PadId::new(5)),
            },
        );
        dispatch(&mut state, Action::ConfirmDrop);
        assert_eq!(
            state.pads.get(PadId::new(5)).sample.as_deref(),
            Some("kick.wav")
        );
        dispatch(&mut state, Action::UndoDrop);
        assert_eq!(state.pads.get(PadId::new(5)).sample, None);
        dispatch(&mut state, Action::UndoDrop);
        assert!(
            state
                .notifications
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("no drop"))
        );
    }

    #[test]
    fn escape_and_enter_follow_the_dialog_overlay_performance_precedence() {
        let mut state = state();

        // Dialog first.
        state.notifications.error(
            "T",
            "m",
            crate::state::notifications::DialogAction::new(DialogIntent::Dismiss),
        );
        dispatch(&mut state, Action::Escape);
        assert!(state.notifications.dialog().is_none());

        // Then the map overlay.
        state.performance.set_overlay(true);
        dispatch(&mut state, Action::Escape);
        assert!(!state.performance.overlay_visible());

        // Then performance mode itself.
        state.navigation.go(View::Performance);
        dispatch(&mut state, Action::Escape);
        assert!(!state.navigation.is_performance());

        // Enter confirms a pending drop.
        let temp = tempfile::tempdir().expect("temp");
        dispatch(
            &mut state,
            Action::BeginDrop {
                paths: vec![temp.path().join("snare.wav")],
                target: Some(PadId::new(1)),
            },
        );
        dispatch(&mut state, Action::Confirm);
        assert!(!state.dragdrop.is_confirming());
        assert_eq!(
            state.pads.get(PadId::new(1)).sample.as_deref(),
            Some("snare.wav")
        );
    }

    #[test]
    fn dialog_intents_open_settings_relink_reload_and_crash_folder() {
        for (intent, expected) in [
            (DialogIntent::OpenSettings, None),
            (DialogIntent::RelinkSamples, Some(Effect::RelinkSamples)),
            (DialogIntent::OpenCrashFolder, Some(Effect::OpenCrashFolder)),
        ] {
            let mut state = state();
            state.notifications.error(
                "T",
                "m",
                crate::state::notifications::DialogAction::new(intent),
            );
            let effects = dispatch(&mut state, Action::ChooseDialog(intent));
            if let Some(expected) = expected {
                assert_eq!(effects, vec![expected]);
            } else {
                assert_eq!(state.navigation.current(), View::Settings);
            }
            assert!(state.notifications.dialog().is_none());
        }

        let mut state = state();
        state.kits.finish_load(
            state.kits.active().expect("active"),
            Err("missing".to_owned()),
        );
        state.notifications.error(
            "T",
            "m",
            crate::state::notifications::DialogAction::new(DialogIntent::Retry),
        );
        let effects = dispatch(&mut state, Action::ChooseDialog(DialogIntent::Retry));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Kit(KitRequest::Load(_))]
        ));
    }

    #[test]
    fn a_kit_load_failure_is_surfaced_as_a_retryable_dialog() {
        let mut state = state();
        state.report_load_failure("kick.wav is missing");
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(dialog.action(DialogIntent::Retry).is_some());
        assert!(dialog.action(DialogIntent::Dismiss).is_some());
        // Retry parks the request in the failed load state.
        let active = state.kits.active().expect("active");
        state
            .kits
            .finish_load(active, Err("kick.wav is missing".to_owned()));
        let effects = dispatch(&mut state, Action::ChooseDialog(DialogIntent::Retry));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Kit(KitRequest::Load(_))]
        ));
    }
}
