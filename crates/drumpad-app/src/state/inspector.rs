//! Pad inspector: editable pad settings, validation, layers, and preview.
//!
//! This is the UI shape of a pad's settings. `drumpad-model` owns the durable
//! `Pad`; the composition layer projects it into [`PadSettings`] on load and
//! writes it back on save. Keeping a separate UI type here means the inspector's
//! validation rules ("midi note 0..=127", "no overlapping velocity layers",
//! "choke group 1..=8") are testable now and the model crate can adopt them
//! verbatim.

use std::fmt;

use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

use super::keymap::{BindingConflict, KeyboardProfile, key_label};
use super::pad::PadId;

/// Lowest pad volume the inspector accepts, in dB.
pub const MIN_VOLUME_DB: f32 = -60.0;
/// Highest pad volume the inspector accepts, in dB.
pub const MAX_VOLUME_DB: f32 = 12.0;
/// Highest number of velocity layers on one pad.
pub const MAX_LAYERS: usize = 8;
/// Highest choke group number.
pub const MAX_CHOKE_GROUP: u8 = 8;

/// How a pad responds to a press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    /// Play to the end regardless of key release.
    OneShot,
    /// Stop with a short fade on key release.
    Gate,
    /// Loop the loop region while held.
    Loop,
}

impl PlaybackMode {
    /// All modes in menu order.
    pub const ALL: [Self; 3] = [Self::OneShot, Self::Gate, Self::Loop];

    /// Menu label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::OneShot => "One-shot",
            Self::Gate => "Gate",
            Self::Loop => "Loop",
        }
    }
}

/// One velocity layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerSpec {
    /// Lowest MIDI velocity in this layer, `1..=127`.
    pub velocity_min: u8,
    /// Highest MIDI velocity in this layer, `1..=127`.
    pub velocity_max: u8,
    /// Sample file name for the layer.
    pub sample: Option<String>,
    /// Layer gain, added to the pad gain.
    pub gain_db: f32,
    /// Layer tuning in semitones.
    pub tune_semitones: f32,
}

impl LayerSpec {
    /// A full-range layer for a sample.
    pub fn full_range(sample: Option<String>) -> Self {
        Self {
            velocity_min: 1,
            velocity_max: 127,
            sample,
            gain_db: 0.0,
            tune_semitones: 0.0,
        }
    }

    /// Whether `velocity` falls inside the layer.
    pub fn contains(&self, velocity: u8) -> bool {
        (self.velocity_min..=self.velocity_max).contains(&velocity)
    }
}

/// Every editable setting of one pad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadSettings {
    /// Sample slot shown in the inspector.
    pub sample: Option<String>,
    /// Velocity layers, ordered by `velocity_min`.
    pub layers: Vec<LayerSpec>,
    /// Playback mode.
    pub mode: PlaybackMode,
    /// Pad volume in dB.
    pub volume_db: f32,
    /// Equal-power pan, `-1.0` left to `1.0` right.
    pub pan: f32,
    /// Pad tuning in semitones.
    pub tune_semitones: f32,
    /// Choke group, `None` for no choking.
    pub choke_group: Option<u8>,
    /// MIDI note, `None` for unmapped.
    pub midi_note: Option<u8>,
    /// Keyboard key, `None` for unmapped.
    pub key: Option<KeyCode>,
}

impl Default for PadSettings {
    fn default() -> Self {
        Self {
            sample: None,
            layers: Vec::new(),
            mode: PlaybackMode::OneShot,
            volume_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            choke_group: None,
            midi_note: None,
            key: None,
        }
    }
}

impl PadSettings {
    /// Settings for a pad with one full-range sample.
    pub fn with_sample(sample: impl Into<String>) -> Self {
        let sample = sample.into();
        Self {
            sample: Some(sample.clone()),
            layers: vec![LayerSpec::full_range(Some(sample))],
            ..Self::default()
        }
    }

    /// Check the ranges and cross-field rules.
    pub fn validate(&self) -> Result<(), InspectorError> {
        for (field, value) in [
            ("volume", self.volume_db),
            ("pan", self.pan),
            ("tune", self.tune_semitones),
        ] {
            if !value.is_finite() {
                return Err(InspectorError::NotFinite { field });
            }
        }
        if !(MIN_VOLUME_DB..=MAX_VOLUME_DB).contains(&self.volume_db) {
            return Err(InspectorError::Volume {
                value: self.volume_db,
            });
        }
        if !(-1.0..=1.0).contains(&self.pan) {
            return Err(InspectorError::Pan { value: self.pan });
        }
        if !(-24.0..=24.0).contains(&self.tune_semitones) {
            return Err(InspectorError::Tune {
                value: self.tune_semitones,
            });
        }
        if let Some(group) = self.choke_group
            && !(1..=MAX_CHOKE_GROUP).contains(&group)
        {
            return Err(InspectorError::ChokeGroup { value: group });
        }
        if let Some(note) = self.midi_note
            && note > 127
        {
            return Err(InspectorError::MidiNote { value: note });
        }
        if let Some(sample) = &self.sample
            && sample.trim().is_empty()
        {
            return Err(InspectorError::BlankSample);
        }
        if self.layers.len() > MAX_LAYERS {
            return Err(InspectorError::TooManyLayers {
                count: self.layers.len(),
            });
        }

        let mut sorted: Vec<&LayerSpec> = self.layers.iter().collect();
        sorted.sort_by_key(|layer| (layer.velocity_min, layer.velocity_max));
        for layer in &sorted {
            if layer.velocity_min == 0 || layer.velocity_max > 127 {
                return Err(InspectorError::LayerVelocity {
                    min: layer.velocity_min,
                    max: layer.velocity_max,
                });
            }
            if layer.velocity_min > layer.velocity_max {
                return Err(InspectorError::LayerInverted {
                    min: layer.velocity_min,
                    max: layer.velocity_max,
                });
            }
            if !layer.gain_db.is_finite() || !layer.tune_semitones.is_finite() {
                return Err(InspectorError::NotFinite { field: "layer" });
            }
        }
        for pair in sorted.windows(2) {
            if pair[0].velocity_max >= pair[1].velocity_min {
                return Err(InspectorError::LayerOverlap {
                    first_max: pair[0].velocity_max,
                    second_min: pair[1].velocity_min,
                });
            }
        }
        Ok(())
    }

    /// The layer that would play at `velocity`.
    pub fn layer_for(&self, velocity: u8) -> Option<&LayerSpec> {
        self.layers.iter().find(|layer| layer.contains(velocity))
    }

    /// A preview trigger for `velocity`, using the current settings.
    pub fn preview(&self, pad: PadId, velocity: u8) -> Option<TriggerPreview> {
        let layer = self.layer_for(velocity)?;
        Some(TriggerPreview {
            pad,
            velocity,
            gain_db: self.volume_db + layer.gain_db,
            tune_semitones: self.tune_semitones + layer.tune_semitones,
            sample: layer.sample.clone().or_else(|| self.sample.clone()),
        })
    }

    /// Reset every field to the empty-pad defaults.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Key label for the inspector's key field.
    pub fn key_label(&self) -> String {
        self.key
            .map(key_label)
            .unwrap_or_else(|| "Unmapped".to_owned())
    }
}

/// A preview trigger, ready for the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerPreview {
    /// Pad that would play.
    pub pad: PadId,
    /// Trigger velocity.
    pub velocity: u8,
    /// Combined pad and layer gain.
    pub gain_db: f32,
    /// Combined pad and layer tuning.
    pub tune_semitones: f32,
    /// Sample that would play.
    pub sample: Option<String>,
}

/// Inspector validation failure.
#[derive(Debug, Clone, PartialEq)]
pub enum InspectorError {
    /// Volume out of range.
    Volume {
        /// Requested dB.
        value: f32,
    },
    /// Pan out of range.
    Pan {
        /// Requested pan.
        value: f32,
    },
    /// Tune out of range.
    Tune {
        /// Requested semitones.
        value: f32,
    },
    /// Choke group outside 1..=8.
    ChokeGroup {
        /// Requested group.
        value: u8,
    },
    /// MIDI note outside 0..=127.
    MidiNote {
        /// Requested note.
        value: u8,
    },
    /// A numeric field is NaN or infinite.
    NotFinite {
        /// Field name.
        field: &'static str,
    },
    /// Sample name is whitespace.
    BlankSample,
    /// Too many layers.
    TooManyLayers {
        /// Requested count.
        count: usize,
    },
    /// Layer velocity outside 1..=127.
    LayerVelocity {
        /// Minimum.
        min: u8,
        /// Maximum.
        max: u8,
    },
    /// Layer min greater than max.
    LayerInverted {
        /// Minimum.
        min: u8,
        /// Maximum.
        max: u8,
    },
    /// Two layers cover the same velocity.
    LayerOverlap {
        /// End of the first layer.
        first_max: u8,
        /// Start of the second.
        second_min: u8,
    },
    /// The key is already used by another pad or by transport.
    KeyConflict(BindingConflict),
}

impl fmt::Display for InspectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Volume { value } => write!(
                formatter,
                "volume {value:.1} dB is outside {MIN_VOLUME_DB:.0}..={MAX_VOLUME_DB:.0} dB"
            ),
            Self::Pan { value } => write!(formatter, "pan {value:.2} is outside -1.0..=1.0"),
            Self::Tune { value } => {
                write!(formatter, "tune {value:.1} is outside -24..=24 semitones")
            }
            Self::ChokeGroup { value } => {
                write!(
                    formatter,
                    "choke group {value} is outside 1..={MAX_CHOKE_GROUP}"
                )
            }
            Self::MidiNote { value } => write!(formatter, "MIDI note {value} is outside 0..=127"),
            Self::NotFinite { field } => write!(formatter, "{field} must be a finite number"),
            Self::BlankSample => formatter.write_str("the sample name cannot be blank"),
            Self::TooManyLayers { count } => {
                write!(
                    formatter,
                    "{count} layers exceeds the maximum of {MAX_LAYERS}"
                )
            }
            Self::LayerVelocity { min, max } => {
                write!(
                    formatter,
                    "layer velocities {min}..={max} must stay inside 1..=127"
                )
            }
            Self::LayerInverted { min, max } => {
                write!(formatter, "layer minimum {min} is above its maximum {max}")
            }
            Self::LayerOverlap {
                first_max,
                second_min,
            } => write!(
                formatter,
                "layers overlap: {first_max} and {second_min} share a velocity"
            ),
            Self::KeyConflict(conflict) => write!(formatter, "{conflict}"),
        }
    }
}

impl std::error::Error for InspectorError {}

impl From<BindingConflict> for InspectorError {
    fn from(conflict: BindingConflict) -> Self {
        Self::KeyConflict(conflict)
    }
}

/// The inspector's editing session for one pad.
#[derive(Debug, Clone)]
pub struct PadInspector {
    pad: PadId,
    saved: PadSettings,
    draft: PadSettings,
    last_error: Option<InspectorError>,
}

impl PadInspector {
    /// Start editing `pad` from its current settings.
    pub fn new(pad: PadId, settings: PadSettings) -> Self {
        Self {
            pad,
            draft: settings.clone(),
            saved: settings,
            last_error: None,
        }
    }

    /// The pad being edited.
    pub fn pad(&self) -> PadId {
        self.pad
    }

    /// The persisted settings.
    pub fn saved(&self) -> &PadSettings {
        &self.saved
    }

    /// The editable draft.
    pub fn draft(&self) -> &PadSettings {
        &self.draft
    }

    /// Mutable draft access for widgets.
    pub fn draft_mut(&mut self) -> &mut PadSettings {
        &mut self.draft
    }

    /// Switch to another pad, dropping (not saving) the draft.
    pub fn select(&mut self, pad: PadId, settings: PadSettings) {
        self.pad = pad;
        self.saved = settings.clone();
        self.draft = settings;
        self.last_error = None;
    }

    /// Whether the draft differs from the saved settings.
    pub fn is_dirty(&self) -> bool {
        self.draft != self.saved
    }

    /// The last rejected value's reason.
    pub fn last_error(&self) -> Option<&InspectorError> {
        self.last_error.as_ref()
    }

    /// Validate and accept the draft. Also checks the key against the active
    /// profile so the inspector cannot create a duplicate binding.
    pub fn apply(&mut self, profile: &KeyboardProfile) -> Result<&PadSettings, InspectorError> {
        if let Some(key) = self.draft.key {
            if let Some(other) = profile.pad_for(key)
                && other != self.pad
            {
                let error = InspectorError::KeyConflict(BindingConflict::Pad { key, other });
                self.last_error = Some(error.clone());
                return Err(error);
            }
            if let Some(action) = profile
                .transport_keys
                .iter()
                .position(|candidate| *candidate == key)
            {
                let error = InspectorError::KeyConflict(BindingConflict::Transport {
                    key,
                    action: super::keymap::TransportAction::ALL[action],
                });
                self.last_error = Some(error.clone());
                return Err(error);
            }
        }
        match self.draft.validate() {
            Ok(()) => {
                self.saved = self.draft.clone();
                self.last_error = None;
                Ok(&self.saved)
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Discard draft changes.
    pub fn revert(&mut self) {
        self.draft = self.saved.clone();
        self.last_error = None;
    }

    /// Reset the draft to the empty-pad defaults (not yet applied).
    pub fn reset_draft(&mut self) {
        self.draft.reset();
    }

    /// Preview the draft at a velocity, using the draft rather than the saved
    /// settings so auditioning reflects unsaved changes.
    pub fn preview(&self, velocity: u8) -> Option<TriggerPreview> {
        self.draft.preview(self.pad, velocity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::keymap::KeyboardProfile;

    fn profile() -> KeyboardProfile {
        KeyboardProfile::finding_keypad()
    }

    #[test]
    fn a_new_inspector_is_clean() {
        let inspector = PadInspector::new(PadId::new(0), PadSettings::default());
        assert!(!inspector.is_dirty());
        assert_eq!(inspector.last_error(), None);
        assert_eq!(inspector.preview(100), None, "no layer, no preview");
    }

    #[test]
    fn every_pad_setting_is_editable_and_applies() {
        let mut inspector = PadInspector::new(PadId::new(2), PadSettings::default());
        {
            let draft = inspector.draft_mut();
            draft.sample = Some("snare.wav".to_owned());
            draft.mode = PlaybackMode::Gate;
            draft.volume_db = -3.0;
            draft.pan = 0.4;
            draft.tune_semitones = 2.0;
            draft.choke_group = Some(3);
            draft.midi_note = Some(38);
            draft.layers = vec![LayerSpec::full_range(Some("snare.wav".to_owned()))];
        }
        let applied = inspector.apply(&profile()).expect("apply").clone();
        assert_eq!(applied.mode, PlaybackMode::Gate);
        assert_eq!(applied.volume_db, -3.0);
        assert_eq!(applied.choke_group, Some(3));
        assert_eq!(applied.midi_note, Some(38));
        assert!(!inspector.is_dirty());
    }

    #[test]
    fn out_of_range_values_are_blocked_with_a_reason() {
        let cases: Vec<(fn(&mut PadSettings), &str)> = vec![
            (|draft| draft.volume_db = 40.0, "volume"),
            (|draft| draft.pan = 2.0, "pan"),
            (|draft| draft.tune_semitones = 40.0, "tune"),
            (|draft| draft.midi_note = Some(200), "MIDI note"),
            (|draft| draft.choke_group = Some(9), "choke group"),
        ];
        for (mutate, expected) in cases {
            let mut inspector = PadInspector::new(PadId::new(0), PadSettings::default());
            mutate(inspector.draft_mut());
            let error = inspector.apply(&profile()).expect_err("invalid");
            assert!(
                error.to_string().contains(expected),
                "{error} should mention {expected}"
            );
        }
    }

    #[test]
    fn a_rejected_apply_keeps_the_saved_settings() {
        let mut inspector = PadInspector::new(PadId::new(0), PadSettings::default());
        inspector.draft_mut().volume_db = 100.0;
        assert!(inspector.apply(&profile()).is_err());
        assert_eq!(inspector.saved(), &PadSettings::default());
        assert!(inspector.is_dirty());
    }

    #[test]
    fn velocity_layers_must_not_overlap() {
        let mut settings = PadSettings::default();
        settings.layers = vec![
            LayerSpec {
                velocity_min: 1,
                velocity_max: 64,
                ..LayerSpec::full_range(None)
            },
            LayerSpec {
                velocity_min: 64,
                velocity_max: 127,
                ..LayerSpec::full_range(None)
            },
        ];
        assert!(matches!(
            settings.validate(),
            Err(InspectorError::LayerOverlap { .. })
        ));
    }

    #[test]
    fn layers_must_be_ordered_and_inside_the_velocity_range() {
        let mut settings = PadSettings::default();
        settings.layers = vec![LayerSpec {
            velocity_min: 90,
            velocity_max: 20,
            ..LayerSpec::full_range(None)
        }];
        assert!(matches!(
            settings.validate(),
            Err(InspectorError::LayerInverted { .. })
        ));

        settings.layers[0].velocity_min = 0;
        settings.layers[0].velocity_max = 127;
        assert!(matches!(
            settings.validate(),
            Err(InspectorError::LayerVelocity { .. })
        ));
    }

    #[test]
    fn the_layer_for_a_velocity_is_selected_by_range() {
        let mut settings = PadSettings::default();
        settings.layers = vec![
            LayerSpec {
                velocity_min: 1,
                velocity_max: 63,
                sample: Some("soft.wav".to_owned()),
                ..LayerSpec::full_range(None)
            },
            LayerSpec {
                velocity_min: 64,
                velocity_max: 127,
                sample: Some("hard.wav".to_owned()),
                ..LayerSpec::full_range(None)
            },
        ];
        assert_eq!(
            settings
                .layer_for(1)
                .and_then(|layer| layer.sample.as_deref()),
            Some("soft.wav")
        );
        assert_eq!(
            settings
                .layer_for(127)
                .and_then(|layer| layer.sample.as_deref()),
            Some("hard.wav")
        );
        assert_eq!(
            settings
                .layer_for(70)
                .and_then(|layer| layer.sample.as_deref()),
            Some("hard.wav")
        );
    }

    #[test]
    fn preview_uses_the_current_draft_values() {
        let mut inspector = PadInspector::new(PadId::new(5), PadSettings::default());
        inspector.draft_mut().volume_db = -6.0;
        inspector.draft_mut().tune_semitones = 1.0;
        inspector.draft_mut().sample = Some("tom.wav".to_owned());
        inspector.draft_mut().layers = vec![LayerSpec {
            gain_db: 3.0,
            tune_semitones: 2.0,
            sample: Some("tom-layer.wav".to_owned()),
            ..LayerSpec::full_range(None)
        }];
        let preview = inspector.preview(100).expect("preview");
        assert_eq!(preview.gain_db, -3.0);
        assert_eq!(preview.tune_semitones, 3.0);
        assert_eq!(preview.sample.as_deref(), Some("tom-layer.wav"));
        assert_eq!(preview.pad, PadId::new(5));
    }

    #[test]
    fn the_inspector_blocks_a_key_already_used_by_another_pad() {
        let mut inspector = PadInspector::new(PadId::new(0), PadSettings::default());
        inspector.draft_mut().key = Some(KeyCode::KeyV);
        let error = inspector.apply(&profile()).expect_err("conflict");
        assert!(matches!(
            error,
            InspectorError::KeyConflict(BindingConflict::Pad { other, .. }) if other == PadId::new(15)
        ));
    }

    #[test]
    fn the_inspector_blocks_a_transport_key() {
        let mut inspector = PadInspector::new(PadId::new(0), PadSettings::default());
        inspector.draft_mut().key = Some(KeyCode::F2);
        assert!(matches!(
            inspector.apply(&profile()).expect_err("conflict"),
            InspectorError::KeyConflict(BindingConflict::Transport { .. })
        ));
    }

    #[test]
    fn a_pad_may_keep_its_own_key() {
        let mut inspector = PadInspector::new(PadId::new(0), PadSettings::default());
        inspector.draft_mut().key = Some(KeyCode::Digit1);
        assert!(inspector.apply(&profile()).is_ok());
    }

    #[test]
    fn revert_restores_the_saved_settings() {
        let mut inspector = PadInspector::new(PadId::new(0), PadSettings::default());
        inspector.draft_mut().volume_db = -12.0;
        inspector.revert();
        assert!(!inspector.is_dirty());
        assert_eq!(inspector.draft().volume_db, 0.0);
    }

    #[test]
    fn reset_clears_the_draft_and_can_be_applied() {
        let mut inspector = PadInspector::new(PadId::new(0), PadSettings::with_sample("kick.wav"));
        inspector.reset_draft();
        inspector.apply(&profile()).expect("apply reset");
        assert_eq!(inspector.saved(), &PadSettings::default());
        assert_eq!(inspector.saved().sample, None);
    }

    #[test]
    fn pad_settings_round_trip_through_json_for_kit_persistence() {
        let settings = PadSettings::with_sample("kick.wav");
        let json = serde_json::to_string(&settings).expect("serialize");
        let decoded: PadSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn a_blank_sample_name_is_rejected() {
        let mut settings = PadSettings::default();
        settings.sample = Some("   ".to_owned());
        assert_eq!(settings.validate(), Err(InspectorError::BlankSample));
    }
}
