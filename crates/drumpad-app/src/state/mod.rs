//! Application state for the Drumpad UI.
//!
//! Ownership map:
//!
//! - [`pad::Pads`] — live pad visuals (flash, velocity ring, meters), selection.
//! - [`inspector::PadInspector`] plus `pad_settings` — the editable pad values.
//! - [`editor`] — non-destructive sample edit parameters and undo/redo.
//! - [`waveform`] — peaks and viewport math for the waveform widget.
//! - [`kit::KitBrowser`] — catalogue, CRUD rules, asynchronous load state.
//! - [`navigation::Navigation`] — which view is visible; nothing else moves.
//! - [`performance::Performance`] — metronome, panic, map overlay, transport flashes.
//! - [`notifications::Notifications`] — toasts and dialogs with concrete actions.
//! - [`dragdrop::DropController`] — preview, confirm, and undo for file drops.
//! - [`prefs::SettingsSession`] + [`wizard::Wizard`] — settings and first-run setup.
//! - [`keymap::KeyRouter`] — physical-key routing with auto-repeat suppression.
//!
//! No I/O, no engine handles, and no egui types live here. The shell
//! (`crate::app`) turns intents into [`Effect`]s, and the composition layer
//! performs them.

pub mod dragdrop;
pub mod editor;
pub mod inspector;
pub mod keymap;
pub mod kit;
pub mod navigation;
pub mod notifications;
pub mod pad;
pub mod performance;
pub mod prefs;
pub mod waveform;
pub mod wizard;

use std::time::Duration;

use dragdrop::DropController;
use editor::EditSession;
use inspector::{InspectorError, PadInspector, PadSettings};
use keymap::{KeyAction, KeyInput, KeyRouter, KeyboardProfile, ProfileId, TransportAction};
use kit::{KitBrowser, KitError, LoadStatus};
use navigation::Navigation;
use notifications::{DialogAction, DialogIntent, Notifications, Severity};
use pad::{PAD_COUNT, PadId, Pads};
use performance::{Performance, TransportEffect};
use prefs::{GridSize, SettingsSession, UiPreferences};
use waveform::{PeakSet, WaveformView};
use wizard::Wizard;

/// What the shell or composition layer should do after an intent is applied.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Nothing outside the UI state changed.
    None,
    /// Trigger a voice.
    Trigger {
        /// Pad to play.
        pad: PadId,
        /// Velocity `1..=127`.
        velocity: u8,
    },
    /// Release a gate-mode voice.
    Release {
        /// Pad to release.
        pad: PadId,
    },
    /// Stop every voice with a short fade.
    Panic,
    /// Start or stop the metronome.
    Metronome(bool),
    /// A kit operation for the model/kit service.
    Kit(kit::KitRequest),
    /// Duplicate a kit through the model/kit service.
    DuplicateKit(kit::KitId),
    /// Import a folder into a new kit.
    ImportKit {
        /// New kit name.
        name: String,
        /// Folder to scan.
        folder: std::path::PathBuf,
    },
    /// Persist preferences to `preferences.json`.
    PersistPreferences(Box<UiPreferences>),
    /// The appearance settings changed; re-apply the theme.
    PreferencesChanged,
    /// Open the log folder in Explorer.
    OpenLogFolder,
    /// Open the crash folder in Explorer.
    OpenCrashFolder,
    /// Run the missing-sample relink flow.
    RelinkSamples,
    /// A pad's settings were saved and must be written into the current kit.
    PadSettingsChanged(PadId),
    /// The waveform peaks changed and the widget must redraw.
    WaveformChanged,
}

/// A top-level screen plus the status bar contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// Active kit name, or `No kit`.
    pub kit: String,
    /// Load state summary.
    pub load: String,
    /// Active keyboard profile name.
    pub profile: String,
    /// Whether the metronome is on.
    pub metronome: bool,
    /// Selected pad number, one-based.
    pub pad: u8,
    /// Whether the active kit has unsaved changes.
    pub dirty: bool,
    /// Audio device and rate summary.
    pub audio: String,
}

/// UI-facing audio state, filled by the shell from the MVP runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStatus {
    /// Friendly device name, or a reason audio is unavailable.
    pub device: String,
    /// Engine/stream sample rate.
    pub sample_rate: u32,
    /// Negotiated buffer size in frames.
    pub buffer_frames: u32,
    /// Whether the stream is rendering.
    pub running: bool,
    /// Active voices from the engine meters.
    pub voices: u32,
    /// Fallback or error detail shown in the status bar.
    pub detail: Option<String>,
}

impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            device: "Audio not started".to_owned(),
            sample_rate: 48_000,
            buffer_frames: 128,
            running: false,
            voices: 0,
            detail: None,
        }
    }
}

impl AudioStatus {
    /// One-line status-bar summary.
    pub fn summary(&self) -> String {
        if self.running {
            let mut text = format!(
                "{} | {} Hz | {} frames | {} voice(s)",
                self.device, self.sample_rate, self.buffer_frames, self.voices
            );
            if let Some(detail) = &self.detail {
                text.push_str(" | ");
                text.push_str(detail);
            }
            text
        } else {
            match &self.detail {
                Some(detail) => format!("Audio unavailable: {detail}"),
                None => "Audio unavailable".to_owned(),
            }
        }
    }
}

/// The complete UI state.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Live pad visuals and selection.
    pub pads: Pads,
    /// Physical-key router.
    pub router: KeyRouter,
    /// Visible view.
    pub navigation: Navigation,
    /// Kit catalogue and load state.
    pub kits: KitBrowser,
    /// Performance-mode state.
    pub performance: Performance,
    /// Toasts and dialogs.
    pub notifications: Notifications,
    /// Drag-and-drop lifecycle.
    pub dragdrop: DropController,
    /// Settings screen session.
    pub settings: SettingsSession,
    /// First-run wizard.
    pub wizard: Wizard,
    /// Inspector session for the selected pad.
    pub inspector: PadInspector,
    /// Sample editing session for the selected pad, when a sample is loaded.
    pub editor: Option<EditSession>,
    /// Waveform viewport for the editor.
    pub waveform: WaveformView,
    /// Peaks for the editor waveform.
    pub peaks: Option<PeakSet>,
    /// Audio device status, filled from the MVP runtime by the shell.
    pub audio: AudioStatus,
    /// Frames rendered since launch, used by the UI smoke tests.
    pub frame: u64,
    /// Time since launch, used by the status bar and animations.
    pub elapsed: Duration,
    pad_settings: Vec<PadSettings>,
}

impl AppState {
    /// Build state from persisted preferences.
    pub fn new(preferences: UiPreferences) -> Self {
        let profile = profile_for(&preferences);
        let mut pads = Pads::new();
        pads.set_key_labels(&profile.pad_labels());
        pads.apply_grid(profile.grid);
        pads.select(PadId::new(0));

        let pad_settings = vec![PadSettings::default(); PAD_COUNT];
        let inspector = PadInspector::new(PadId::new(0), pad_settings[0].clone());

        Self {
            pads,
            router: KeyRouter::new(profile),
            navigation: Navigation::default(),
            kits: KitBrowser::with_factory_kit(),
            performance: Performance::new(),
            notifications: Notifications::new(),
            dragdrop: DropController::new(),
            settings: SettingsSession::new(preferences.clone()),
            wizard: Wizard::new(&preferences),
            inspector,
            editor: None,
            waveform: WaveformView::default(),
            peaks: None,
            audio: AudioStatus::default(),
            frame: 0,
            elapsed: Duration::ZERO,
            pad_settings,
        }
    }

    /// The active keyboard profile.
    pub fn profile(&self) -> &KeyboardProfile {
        self.router.profile()
    }

    /// The active profile id.
    pub fn profile_id(&self) -> ProfileId {
        self.profile().id
    }

    /// Current pad-surface size.
    pub fn grid_size(&self) -> GridSize {
        self.settings.draft().keyboard.grid_size
    }

    /// Apply a pad-surface size live and keep it in the settings draft so the
    /// next Apply persists it.
    pub fn set_grid_size(&mut self, grid: GridSize) {
        self.settings.draft_mut().keyboard.grid_size = grid;
        self.router.set_grid(grid);
        self.pads.apply_grid(grid);
        self.pads.set_key_labels(&self.profile().pad_labels());
        if self.pads.selected().index() >= grid.pad_count() {
            self.pads.select(PadId::new(0));
            self.inspector
                .select(PadId::new(0), self.pad_settings[0].clone());
        }
    }

    /// Switch profile. Returns `false` when the id is not a built-in profile.
    pub fn set_profile(&mut self, id: ProfileId) -> bool {
        let Some(base) = KeyboardProfile::builtin(id) else {
            return false;
        };
        let profile = base.with_grid(self.grid_size());
        self.pads.set_key_labels(&profile.pad_labels());
        self.pads.apply_grid(profile.grid);
        self.router.set_profile(profile);
        self.settings.draft_mut().keyboard.profile = id;
        true
    }

    /// Persisted settings for one pad.
    pub fn pad_settings(&self, pad: PadId) -> &PadSettings {
        &self.pad_settings[pad.index()]
    }

    /// Replace the persisted settings for one pad.
    pub fn set_pad_settings(&mut self, pad: PadId, settings: PadSettings) {
        self.pad_settings[pad.index()] = settings;
    }

    /// Select a pad for the inspector, keeping the draft of the previous pad.
    pub fn select_pad(&mut self, pad: PadId) {
        if self.pads.selected() == pad {
            return;
        }
        self.pads.select(pad);
        self.inspector
            .select(pad, self.pad_settings[pad.index()].clone());
    }

    /// Write the inspector draft back into the pad settings table.
    ///
    /// Returns the error from validation without mutating anything when the
    /// draft is invalid.
    pub fn commit_inspector(&mut self) -> Result<PadId, InspectorError> {
        let profile = self.profile().clone();
        self.inspector.apply(&profile)?;
        let pad = self.inspector.pad();
        let settings = self.inspector.saved().clone();
        self.pad_settings[pad.index()] = settings;
        Ok(pad)
    }

    /// Revert the inspector draft.
    pub fn revert_inspector(&mut self) {
        self.inspector.revert();
    }

    /// Preview trigger for a pad using its current (possibly unsaved) settings.
    pub fn preview_trigger(&self, pad: PadId, velocity: u8) -> Option<inspector::TriggerPreview> {
        if pad == self.inspector.pad() {
            self.inspector.preview(velocity)
        } else {
            self.pad_settings[pad.index()].preview(pad, velocity)
        }
    }

    /// Route a physical key event.
    pub fn handle_key(&mut self, input: KeyInput) -> Option<KeyAction> {
        let action = self.router.route(input);
        if let Some(KeyAction::Pad(pad)) = action
            && self.navigation.current().allows_edits()
        {
            self.select_pad(pad);
        }
        action
    }

    /// Apply a transport action to performance state and return its effect.
    pub fn transport(&mut self, action: TransportAction) -> TransportEffect {
        self.performance.trigger(action)
    }

    /// Advance every animation clock and the elapsed counter.
    pub fn tick(&mut self, dt: Duration) {
        self.pads.tick(dt);
        self.performance.tick(dt);
        self.notifications.tick(dt);
        self.elapsed += dt;
        self.frame = self.frame.saturating_add(1);
    }

    /// Whether any animation is running and the shell must keep repainting.
    pub fn is_animating(&self) -> bool {
        self.pads.is_animating()
            || self.performance.is_animating()
            || self.notifications.is_animating()
    }

    /// Status-bar contents.
    pub fn status(&self) -> StatusLine {
        let kit = self
            .kits
            .active()
            .and_then(|id| self.kits.get(id))
            .map_or_else(|| "No kit".to_owned(), |kit| kit.name.clone());
        let load = match self.kits.load_status() {
            LoadStatus::Idle => "Idle".to_owned(),
            LoadStatus::Loading { percent, .. } => format!("Loading kit {percent}%"),
            LoadStatus::Ready { .. } => "Ready".to_owned(),
            LoadStatus::Failed { message, .. } => format!("Load failed: {message}"),
        };
        StatusLine {
            kit,
            load,
            profile: self.profile().name.clone(),
            metronome: self.performance.metronome(),
            pad: self.pads.selected().number(),
            dirty: self.kits.has_unsaved_changes() || self.inspector.is_dirty(),
            audio: self.audio.summary(),
        }
    }

    /// Replace the audio status block from the shell.
    pub fn set_audio_status(&mut self, status: AudioStatus) {
        self.audio = status;
    }

    /// Project a loaded kit into the pad grid and the kit browser.
    ///
    /// The UI only needs display data here: pad names, the first sample name,
    /// and the MIDI hint. Runtime parameters are published to the engine by the
    /// audio runtime, not by the UI.
    pub fn apply_kit(&mut self, kit: &drumpad_model::kit::Kit) {
        self.kits.set_active_name(kit.metadata.name.clone());
        for pad in &kit.pads {
            let Some(id) = PadId::try_new(pad.id.index()) else {
                continue;
            };
            if let Some(name) = &pad.name {
                self.pads.set_name(id, name.clone());
            }
            if let Some(midi) = &pad.midi {
                self.pads.get_mut(id).midi_hint = Some(format!("N{}", midi.note.get()));
            }
            let sample = pad
                .layers
                .iter()
                .find_map(|layer| layer.sample.as_ref())
                .map(|sample| {
                    sample
                        .path
                        .as_str()
                        .rsplit('/')
                        .next()
                        .unwrap_or(sample.path.as_str())
                        .to_owned()
                });
            self.pads.set_sample(id, sample);
        }
    }

    /// Report an inspector failure as a dialog with a next action.
    pub fn report_inspector_error(&mut self, error: &InspectorError) {
        self.notifications.error(
            "Pad setting rejected",
            error.to_string(),
            DialogAction::labelled(DialogIntent::Dismiss, "Fix the value"),
        );
    }

    /// Report a kit failure. Load failures offer Retry; rule violations are a
    /// toast because the user can fix them in place.
    pub fn report_kit_error(&mut self, error: &KitError) {
        match error {
            KitError::Busy => {
                self.notifications
                    .warning("A kit is already loading; wait for it to finish.");
            }
            other => {
                self.notifications
                    .toast(Severity::Warning, other.to_string());
            }
        }
    }

    /// Report a failed kit load with Retry and Dismiss.
    pub fn report_load_failure(&mut self, message: impl Into<String>) {
        self.notifications.error(
            "Kit could not be loaded",
            message,
            DialogAction::new(DialogIntent::Retry),
        );
        self.notifications.add_dialog_action(DialogAction::labelled(
            DialogIntent::Dismiss,
            "Keep current kit",
        ));
    }

    /// Report missing samples with a relink action.
    pub fn report_missing_samples(&mut self, detail: impl Into<String>) {
        self.notifications.error(
            "Samples are missing",
            detail,
            DialogAction::new(DialogIntent::RelinkSamples),
        );
        self.notifications.add_dialog_action(DialogAction::labelled(
            DialogIntent::Dismiss,
            "Play silent pads",
        ));
    }

    /// Load an edit session for the selected pad and build its peaks.
    pub fn open_editor(&mut self, session: EditSession) {
        self.waveform = WaveformView::default();
        self.peaks = None;
        self.editor = Some(session);
    }

    /// Publish peaks for the open edit session.
    pub fn set_peaks(&mut self, peaks: PeakSet) {
        self.peaks = Some(peaks);
    }

    /// Close the editor, keeping the pad settings untouched.
    pub fn close_editor(&mut self) {
        self.editor = None;
        self.peaks = None;
        self.waveform = WaveformView::default();
    }

    /// Undo the last edit, returning the label that was reverted.
    pub fn undo_edit(&mut self) -> Option<String> {
        self.editor.as_mut()?.undo()
    }

    /// Redo the last undone edit.
    pub fn redo_edit(&mut self) -> Option<String> {
        self.editor.as_mut()?.redo()
    }

    /// History labels for the UI, `(undo, redo)`.
    pub fn history_labels(&self) -> (Option<String>, Option<String>) {
        let Some(editor) = &self.editor else {
            return (None, None);
        };
        (
            editor.undo_label().map(str::to_owned),
            editor.redo_label().map(str::to_owned),
        )
    }

    /// Convenience for tests and the smoke harness: the undo stack length.
    pub fn undo_depth(&self) -> usize {
        let Some(editor) = &self.editor else {
            return 0;
        };
        // `EditSession` exposes availability, not depth; count by labels.
        let mut depth = 0;
        let mut probe = editor.clone();
        while probe.undo().is_some() {
            depth += 1;
        }
        depth
    }
}

fn profile_for(preferences: &UiPreferences) -> KeyboardProfile {
    KeyboardProfile::builtin(preferences.keyboard.profile)
        .unwrap_or_else(KeyboardProfile::finding_keypad)
        .with_grid(preferences.keyboard.grid_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_state_is_on_the_performance_view_with_a_factory_kit() {
        let state = AppState::new(UiPreferences::default());
        assert_eq!(state.navigation.current(), navigation::View::Performance);
        assert_eq!(state.pads.selected(), PadId::new(0));
        assert_eq!(state.status().kit, "Default Kit");
        assert_eq!(state.status().load, "Ready");
        assert!(!state.status().dirty);
    }

    #[test]
    fn key_labels_come_from_the_active_profile() {
        let state = AppState::new(UiPreferences::default());
        assert_eq!(state.grid_size(), GridSize::ThreeByThree);
        assert_eq!(state.pads.get(PadId::new(0)).key_label, "Q");
        assert_eq!(state.pads.get(PadId::new(8)).key_label, "C");
        assert_eq!(
            state.pads.get(PadId::new(0)).key_label,
            GridSize::ThreeByThree.key_labels()[0]
        );
        assert_eq!(state.status().profile, "Finding 35-key keypad");
    }

    #[test]
    fn switching_to_4x4_relabels_to_the_number_row() {
        let mut state = AppState::new(UiPreferences::default());
        state.set_grid_size(GridSize::FourByFour);
        assert_eq!(state.pads.get(PadId::new(0)).key_label, "1");
        assert_eq!(state.pads.get(PadId::new(15)).key_label, "V");
        assert_eq!(state.router.profile().pad_count(), 16);
    }

    #[test]
    fn unknown_profiles_do_not_change_the_mapping() {
        let mut state = AppState::new(UiPreferences::default());
        assert!(!state.set_profile(ProfileId(99)));
        assert_eq!(state.profile_id(), ProfileId(2));
    }

    #[test]
    fn selecting_a_pad_swaps_the_inspector_document() {
        let mut state = AppState::new(UiPreferences::default());
        state.set_pad_settings(PadId::new(4), PadSettings::with_sample("tom.wav"));
        state.select_pad(PadId::new(4));
        assert_eq!(state.inspector.pad(), PadId::new(4));
        assert_eq!(state.inspector.draft().sample.as_deref(), Some("tom.wav"));
    }

    #[test]
    fn committing_the_inspector_persists_to_the_pad_table() {
        let mut state = AppState::new(UiPreferences::default());
        state.inspector.draft_mut().volume_db = -6.0;
        let pad = state.commit_inspector().expect("commit");
        assert_eq!(state.pad_settings(pad).volume_db, -6.0);
        assert!(!state.inspector.is_dirty());
    }

    #[test]
    fn an_invalid_commit_reports_an_error_and_changes_nothing() {
        let mut state = AppState::new(UiPreferences::default());
        state.inspector.draft_mut().pan = 9.0;
        let error = state.commit_inspector().expect_err("invalid");
        state.report_inspector_error(&error);
        assert_eq!(state.pad_settings(PadId::new(0)).pan, 0.0);
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(!dialog.actions.is_empty());
    }

    #[test]
    fn a_kit_load_failure_offers_retry_and_keep_current() {
        let mut state = AppState::new(UiPreferences::default());
        state.report_load_failure("kick.wav is missing");
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(dialog.action(DialogIntent::Retry).is_some());
        assert!(dialog.action(DialogIntent::Dismiss).is_some());
        assert!(dialog.message.contains("kick.wav"));
    }

    #[test]
    fn missing_samples_offer_a_relink_action() {
        let mut state = AppState::new(UiPreferences::default());
        state.report_missing_samples("3 files under D:\\Kits\\Rock");
        let dialog = state.notifications.dialog().expect("dialog");
        assert!(dialog.action(DialogIntent::RelinkSamples).is_some());
    }

    #[test]
    fn ticking_advances_animations_and_elapsed_time() {
        let mut state = AppState::new(UiPreferences::default());
        state.pads.trigger(PadId::new(0), 1.0);
        assert!(state.is_animating());
        state.tick(Duration::from_millis(400));
        assert_eq!(state.pads.get(PadId::new(0)).flash, 0.0);
        assert_eq!(state.frame, 1);
        assert_eq!(state.elapsed, Duration::from_millis(400));
        state.tick(Duration::from_secs(2));
        assert!(!state.is_animating());
    }

    #[test]
    fn editor_undo_and_redo_are_reachable_from_state() {
        use crate::state::editor::{EditParams, EditSession, SourceRef};
        let source = SourceRef {
            path: "kick.wav".into(),
            content_hash: "hash".to_owned(),
            sample_rate: 48_000,
            channels: 1,
            frames: 48_000,
        };
        let mut state = AppState::new(UiPreferences::default());
        let mut session = EditSession::new(source);
        let mut params = EditParams::default();
        params.gain_db = -3.0;
        session.commit("Gain", params).expect("commit");
        state.open_editor(session);
        assert_eq!(state.history_labels().0.as_deref(), Some("Gain"));
        assert_eq!(state.undo_edit().as_deref(), Some("Gain"));
        assert_eq!(state.history_labels().1.as_deref(), Some("Gain"));
        assert_eq!(state.redo_edit().as_deref(), Some("Gain"));
        assert_eq!(state.undo_depth(), 1);
    }
}
