//! First-run setup wizard.
//!
//! The wizard is a short, skippable sequence that leaves the app in a playable
//! state: a keyboard profile, an audio device, optionally a MIDI device, and a
//! sample folder. Every step can be skipped, and every skipped step can be
//! revisited from Settings, so the app never blocks on hardware that is not
//! plugged in.

use serde::{Deserialize, Serialize};

use super::keymap::KeyboardProfile;
use super::prefs::{AudioPrefs, MidiPrefs, UiPreferences};

/// One page of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStep {
    /// Welcome and what the wizard will ask.
    Welcome,
    /// Full keyboard or Finding one-hand keypad.
    Keyboard,
    /// Output device, sample rate, buffer.
    Audio,
    /// MIDI input device.
    Midi,
    /// Folder to import samples from.
    SampleFolder,
    /// Summary and finish.
    Done,
}

impl WizardStep {
    /// Steps in order.
    pub const ORDER: [Self; 6] = [
        Self::Welcome,
        Self::Keyboard,
        Self::Audio,
        Self::Midi,
        Self::SampleFolder,
        Self::Done,
    ];

    /// Page title.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome to Drumpad",
            Self::Keyboard => "Choose your keyboard",
            Self::Audio => "Choose your output",
            Self::Midi => "Connect a MIDI kit",
            Self::SampleFolder => "Find your samples",
            Self::Done => "You are ready to play",
        }
    }

    /// One-line explanation.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Welcome => {
                "Four quick choices, then the pads are live. You can change all of them later in Settings."
            }
            Self::Keyboard => {
                "The Finding 35-key one-hand keypad is the reference layout. A full keyboard uses the same 4x4 block."
            }
            Self::Audio => {
                "Pick the output that feeds your amp or the Nitro Max Aux In. Defaults work on most Windows machines."
            }
            Self::Midi => {
                "Select the Nitro Max port to play the pads from the kit. Skip it if you only use the keyboard today."
            }
            Self::SampleFolder => {
                "Where your kicks, snares, and loops live. Drag files onto pads at any time."
            }
            Self::Done => {
                "Press Finish. Pads trigger immediately; open Settings any time to revisit a step."
            }
        }
    }

    /// Whether the step can be skipped.
    pub const fn skippable(self) -> bool {
        !matches!(self, Self::Welcome | Self::Done)
    }

    fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|step| *step == self)
            .unwrap_or(0)
    }

    fn next(self) -> Self {
        Self::ORDER[(self.index() + 1).min(Self::ORDER.len() - 1)]
    }

    fn previous(self) -> Self {
        Self::ORDER[self.index().saturating_sub(1)]
    }
}

/// Wizard progress and the choices made so far.
#[derive(Debug, Clone)]
pub struct Wizard {
    step: WizardStep,
    completed: bool,
    skipped: Vec<WizardStep>,
    reopened: bool,
}

impl Wizard {
    /// Start at the welcome page unless setup is already done.
    pub fn new(preferences: &UiPreferences) -> Self {
        Self {
            step: WizardStep::Welcome,
            completed: preferences.wizard_completed,
            skipped: Vec::new(),
            reopened: false,
        }
    }

    /// Whether the wizard should be shown.
    pub fn is_active(&self) -> bool {
        !self.completed
    }

    /// The visible page.
    pub fn step(&self) -> WizardStep {
        self.step
    }

    /// Whether the wizard was reopened from Settings.
    pub fn is_reopened(&self) -> bool {
        self.reopened
    }

    /// Steps the user skipped.
    pub fn skipped(&self) -> &[WizardStep] {
        &self.skipped
    }

    /// Whether this step was skipped earlier.
    pub fn was_skipped(&self, step: WizardStep) -> bool {
        self.skipped.contains(&step)
    }

    /// Advance, refusing to finish until the Done page.
    pub fn next(&mut self, preferences: &mut UiPreferences) {
        if self.step == WizardStep::Done {
            self.finish(preferences);
            return;
        }
        self.step = self.step.next();
    }

    /// Go back one page.
    pub fn back(&mut self) {
        self.step = self.step.previous();
    }

    /// Skip the current page and advance, leaving its choice at the default.
    ///
    /// The welcome page cannot be skipped (there is nothing to skip).
    pub fn skip(&mut self, preferences: &mut UiPreferences) {
        if !self.step.skippable() {
            self.next(preferences);
            return;
        }
        if !self.skipped.contains(&self.step) {
            self.skipped.push(self.step);
        }
        self.step = self.step.next();
    }

    /// Finish setup. Idempotent.
    pub fn finish(&mut self, preferences: &mut UiPreferences) {
        preferences.wizard_completed = true;
        self.completed = true;
        self.step = WizardStep::Done;
    }

    /// Open the wizard again from Settings, keeping current choices.
    pub fn reopen(&mut self) {
        self.completed = false;
        self.reopened = true;
        self.skipped.clear();
        self.step = WizardStep::Welcome;
    }

    /// Close a reopened wizard without changing anything.
    pub fn close(&mut self) {
        self.completed = true;
        self.reopened = false;
    }

    /// Pick a keyboard profile by id.
    pub fn choose_profile(
        &mut self,
        preferences: &mut UiPreferences,
        id: super::keymap::ProfileId,
    ) -> bool {
        if KeyboardProfile::builtin(id).is_some() {
            preferences.keyboard.profile = id;
            true
        } else {
            false
        }
    }

    /// Set the audio choices used by the summary and the app.
    pub fn choose_audio(&mut self, preferences: &mut UiPreferences, audio: AudioPrefs) {
        preferences.audio = audio;
    }

    /// Set the MIDI choices.
    pub fn choose_midi(&mut self, preferences: &mut UiPreferences, midi: MidiPrefs) {
        preferences.midi = midi;
    }

    /// Set the sample folder.
    pub fn choose_sample_folder(
        &mut self,
        preferences: &mut UiPreferences,
        folder: std::path::PathBuf,
    ) {
        preferences.sample_folder = Some(folder);
    }

    /// Human summary lines for the Done page.
    pub fn summary(&self, preferences: &UiPreferences) -> Vec<String> {
        let profile = KeyboardProfile::builtin(preferences.keyboard.profile)
            .map(|profile| profile.name)
            .unwrap_or_else(|| "Full keyboard".to_owned());
        let audio = match &preferences.audio.device {
            Some(device) => format!("{} @ {} Hz", device, preferences.audio.sample_rate),
            None => format!("System default @ {} Hz", preferences.audio.sample_rate),
        };
        let midi = preferences
            .midi
            .device
            .clone()
            .unwrap_or_else(|| "No MIDI device (keyboard only)".to_owned());
        let folder = preferences
            .sample_folder
            .as_ref()
            .map(|folder| folder.display().to_string())
            .unwrap_or_else(|| "Defaults to Documents\\Drumpad\\Samples".to_owned());
        vec![
            format!("Keyboard: {profile}"),
            format!("Output: {audio}"),
            format!("MIDI: {midi}"),
            format!("Samples: {folder}"),
        ]
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;
    use crate::state::keymap::ProfileId;

    #[test]
    fn a_clean_install_starts_at_the_welcome_page() {
        let preferences = UiPreferences::default();
        let wizard = Wizard::new(&preferences);
        assert!(wizard.is_active());
        assert_eq!(wizard.step(), WizardStep::Welcome);
    }

    #[test]
    fn a_completed_install_does_not_show_the_wizard() {
        let mut preferences = UiPreferences::default();
        preferences.wizard_completed = true;
        assert!(!Wizard::new(&preferences).is_active());
    }

    #[test]
    fn every_step_advances_in_order_and_finishes_on_done() {
        let mut preferences = UiPreferences::default();
        let mut wizard = Wizard::new(&preferences);
        for expected in [
            WizardStep::Keyboard,
            WizardStep::Audio,
            WizardStep::Midi,
            WizardStep::SampleFolder,
            WizardStep::Done,
        ] {
            wizard.next(&mut preferences);
            assert_eq!(wizard.step(), expected);
        }
        assert!(wizard.is_active(), "Done is a page, not a finish");
        wizard.next(&mut preferences);
        assert!(!wizard.is_active());
        assert!(preferences.wizard_completed);
    }

    #[test]
    fn back_stops_at_the_first_page() {
        let mut preferences = UiPreferences::default();
        let mut wizard = Wizard::new(&preferences);
        wizard.back();
        assert_eq!(wizard.step(), WizardStep::Welcome);
        wizard.next(&mut preferences);
        wizard.back();
        assert_eq!(wizard.step(), WizardStep::Welcome);
    }

    #[test]
    fn every_optional_step_can_be_skipped_and_the_wizard_still_completes() {
        let mut preferences = UiPreferences::default();
        let mut wizard = Wizard::new(&preferences);
        while wizard.step() != WizardStep::Done {
            let step = wizard.step();
            if step.skippable() {
                wizard.skip(&mut preferences);
                assert!(
                    wizard.was_skipped(step),
                    "{step:?} should be recorded as skipped"
                );
            } else {
                wizard.next(&mut preferences);
            }
        }
        wizard.next(&mut preferences);
        assert!(!wizard.is_active());
        assert!(preferences.wizard_completed);
        assert_eq!(wizard.skipped().len(), 4);
    }

    #[test]
    fn skipping_everything_leaves_valid_defaults() {
        let mut preferences = UiPreferences::default();
        let mut wizard = Wizard::new(&preferences);
        while wizard.is_active() {
            wizard.skip(&mut preferences);
        }
        assert_eq!(preferences.validate(), Ok(()));
        assert_eq!(preferences.keyboard.profile, ProfileId(2));
    }

    #[test]
    fn the_welcome_and_done_pages_are_not_skippable() {
        assert!(!WizardStep::Welcome.skippable());
        assert!(!WizardStep::Done.skippable());
        for step in [
            WizardStep::Keyboard,
            WizardStep::Audio,
            WizardStep::Midi,
            WizardStep::SampleFolder,
        ] {
            assert!(step.skippable(), "{step:?}");
        }
    }

    #[test]
    fn choosing_the_finding_profile_sets_the_active_mapping() {
        let mut preferences = UiPreferences::default();
        let mut wizard = Wizard::new(&preferences);
        assert!(wizard.choose_profile(&mut preferences, ProfileId(2)));
        assert_eq!(preferences.keyboard.profile, ProfileId(2));
        assert!(!wizard.choose_profile(&mut preferences, ProfileId(99)));
        assert_eq!(preferences.keyboard.profile, ProfileId(2));
    }

    #[test]
    fn the_summary_names_every_choice() {
        let mut preferences = UiPreferences::default();
        preferences.audio.device = Some("Speakers".to_owned());
        preferences.midi.device = Some("Nitro Max".to_owned());
        preferences.sample_folder = Some(std::path::PathBuf::from("D:\\Samples"));
        let wizard = Wizard::new(&preferences);
        let summary = wizard.summary(&preferences).join("\n");
        assert!(summary.contains("Finding 35-key keypad"));
        assert!(summary.contains("Speakers @ 48000 Hz"));
        assert!(summary.contains("Nitro Max"));
        assert!(summary.contains("D:\\Samples"));
    }

    #[test]
    fn the_wizard_can_be_reopened_from_settings_and_closed_again() {
        let mut preferences = UiPreferences::default();
        preferences.wizard_completed = true;
        let mut wizard = Wizard::new(&preferences);
        assert!(!wizard.is_active());
        wizard.reopen();
        assert!(wizard.is_active());
        assert!(wizard.is_reopened());
        assert_eq!(wizard.step(), WizardStep::Welcome);
        wizard.close();
        assert!(!wizard.is_active());
        assert!(!wizard.is_reopened());
    }

    #[test]
    fn each_step_has_a_title_and_description() {
        for step in WizardStep::ORDER {
            assert!(!step.title().is_empty());
            assert!(!step.description().is_empty());
        }
    }
}
