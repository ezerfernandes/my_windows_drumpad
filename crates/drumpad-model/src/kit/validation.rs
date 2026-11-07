//! Kit invariant validation.
//!
//! Validation collects every issue instead of failing on the first one so the
//! UI and import/repair flows can present a complete, actionable report.

use std::fmt;

use thiserror::Error;

use super::model::{
    CURRENT_FORMAT_VERSION, Kit, MAX_LAYERS_PER_PAD, PAD_COUNT, PadId, PlaybackMode, VelocityRange,
};

/// Whether an issue blocks a kit from loading.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Severity {
    /// Recoverable or informational; the kit can still load.
    Warning,
    /// The kit violates an invariant and must not be used as-is.
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Warning => f.write_str("warning"),
            Self::Error => f.write_str("error"),
        }
    }
}

/// Stable machine-readable code for a validation issue.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum ValidationCode {
    /// The document was written by a different schema version.
    UnsupportedFormatVersion,
    /// The kit does not contain exactly 16 pads.
    WrongPadCount,
    /// Two pads share an id.
    DuplicatePadId,
    /// A pad id is outside the 0..=15 range.
    PadIdOutOfRange,
    /// The kit name is empty.
    EmptyKitName,
    /// The kit name is too long.
    KitNameTooLong,
    /// An optional text field is too long.
    TextTooLong,
    /// There are too many tags.
    TooManyTags,
    /// A pad has too many layers.
    TooManyLayers,
    /// Two layers in one pad share an id.
    DuplicateLayerId,
    /// A velocity bound is outside 1..=127.
    VelocityOutOfRange,
    /// A velocity range is inverted.
    VelocityRangeInverted,
    /// A gain, pan, or tune value is not finite.
    NonFiniteValue,
    /// A gain value is outside its permitted range.
    GainOutOfRange,
    /// A pan value is outside -1..=1.
    PanOutOfRange,
    /// A tuning value is outside its permitted range.
    TuneOutOfRange,
    /// Two pads use the same physical key.
    DuplicateKeyMapping,
    /// Two pads use the same MIDI note and channel.
    DuplicateMidiMapping,
    /// Two sample references share a path but not a content hash.
    SamplePathCollision,
    /// A sample reference has no content hash, so it cannot be verified.
    MissingSampleHash,
    /// A loop range is empty or inverted.
    InvalidLoopRange,
    /// A loop range is configured but the pad is not in loop mode.
    LoopWithoutLoopMode,
    /// A pad has no layer that can play.
    SilentPad,
}

impl ValidationCode {
    /// Stable text form used in logs and error payloads.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedFormatVersion => "unsupported_format_version",
            Self::WrongPadCount => "wrong_pad_count",
            Self::DuplicatePadId => "duplicate_pad_id",
            Self::PadIdOutOfRange => "pad_id_out_of_range",
            Self::EmptyKitName => "empty_kit_name",
            Self::KitNameTooLong => "kit_name_too_long",
            Self::TextTooLong => "text_too_long",
            Self::TooManyTags => "too_many_tags",
            Self::TooManyLayers => "too_many_layers",
            Self::DuplicateLayerId => "duplicate_layer_id",
            Self::VelocityOutOfRange => "velocity_out_of_range",
            Self::VelocityRangeInverted => "velocity_range_inverted",
            Self::NonFiniteValue => "non_finite_value",
            Self::GainOutOfRange => "gain_out_of_range",
            Self::PanOutOfRange => "pan_out_of_range",
            Self::TuneOutOfRange => "tune_out_of_range",
            Self::DuplicateKeyMapping => "duplicate_key_mapping",
            Self::DuplicateMidiMapping => "duplicate_midi_mapping",
            Self::SamplePathCollision => "sample_path_collision",
            Self::MissingSampleHash => "missing_sample_hash",
            Self::InvalidLoopRange => "invalid_loop_range",
            Self::LoopWithoutLoopMode => "loop_without_loop_mode",
            Self::SilentPad => "silent_pad",
        }
    }
}

impl fmt::Display for ValidationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One invariant violation with a JSON-pointer-like location.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidationIssue {
    /// Severity of the issue.
    pub severity: Severity,
    /// Machine-readable code.
    pub code: ValidationCode,
    /// Location inside the document, for example `pads[3].layers[0].velocity.min`.
    pub path: String,
    /// Human-readable explanation.
    pub message: String,
}

impl ValidationIssue {
    fn error(code: ValidationCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            path: path.into(),
            message: message.into(),
        }
    }

    fn warning(code: ValidationCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] at {}: {}",
            self.severity, self.code, self.path, self.message
        )
    }
}

/// A full validation result, including warnings.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ValidationReport {
    /// All issues, ordered by discovery.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// True when no error-severity issue is present.
    pub fn is_valid(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    /// Error-severity issues.
    pub fn errors(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
    }

    /// Warning-severity issues.
    pub fn warnings(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
    }

    /// Compact one-line descriptions for logs and dialogs.
    pub fn messages(&self) -> Vec<String> {
        self.issues.iter().map(ToString::to_string).collect()
    }
}

/// Error returned by [`Kit::validate`] when any hard invariant fails.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
#[error("kit failed validation with {} error(s): {}", .issues.len(), format_issues(.issues))]
pub struct ValidationError {
    /// The error-severity issues, in document order.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationError {
    /// Build from a report if it contains errors.
    pub fn from_report(report: ValidationReport) -> Result<(), Self> {
        let issues: Vec<_> = report
            .issues
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(Self { issues })
        }
    }
}

fn format_issues(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .take(4)
        .map(|issue| format!("{} at {}: {}", issue.code, issue.path, issue.message))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Validate a kit and return every issue found.
pub fn validate_kit_report(kit: &Kit) -> ValidationReport {
    let mut issues = Vec::new();

    if kit.format_version != CURRENT_FORMAT_VERSION {
        issues.push(ValidationIssue::error(
            ValidationCode::UnsupportedFormatVersion,
            "format_version",
            format!(
                "expected {CURRENT_FORMAT_VERSION}, found {}",
                kit.format_version
            ),
        ));
    }

    let name = kit.metadata.name.trim();
    if name.is_empty() {
        issues.push(ValidationIssue::error(
            ValidationCode::EmptyKitName,
            "metadata.name",
            "kit name must not be empty",
        ));
    } else if name.chars().count() > 128 {
        issues.push(ValidationIssue::error(
            ValidationCode::KitNameTooLong,
            "metadata.name",
            "kit name must be 128 characters or fewer",
        ));
    }
    if let Some(author) = &kit.metadata.author
        && author.chars().count() > 128
    {
        issues.push(ValidationIssue::error(
            ValidationCode::TextTooLong,
            "metadata.author",
            "author must be 128 characters or fewer",
        ));
    }
    if let Some(description) = &kit.metadata.description
        && description.chars().count() > 4096
    {
        issues.push(ValidationIssue::error(
            ValidationCode::TextTooLong,
            "metadata.description",
            "description must be 4096 characters or fewer",
        ));
    }
    if kit.metadata.tags.len() > 32 {
        issues.push(ValidationIssue::error(
            ValidationCode::TooManyTags,
            "metadata.tags",
            "a kit may have at most 32 tags",
        ));
    }
    for (index, tag) in kit.metadata.tags.iter().enumerate() {
        if tag.chars().count() > 32 {
            issues.push(ValidationIssue::error(
                ValidationCode::TextTooLong,
                format!("metadata.tags[{index}]"),
                "tags must be 32 characters or fewer",
            ));
        }
    }

    if kit.pads.len() != PAD_COUNT {
        issues.push(ValidationIssue::error(
            ValidationCode::WrongPadCount,
            "pads",
            format!("a v1 kit must have exactly {PAD_COUNT} pads"),
        ));
    }

    let mut seen_pad_ids = std::collections::BTreeMap::new();
    for (index, pad) in kit.pads.iter().enumerate() {
        let base = format!("pads[{index}]");
        if let Some(previous) = seen_pad_ids.insert(pad.id, index) {
            issues.push(ValidationIssue::error(
                ValidationCode::DuplicatePadId,
                format!("{base}.id"),
                format!("pad id {} is also used by pads[{previous}]", pad.id),
            ));
        }
        // `PadId` has a public field by contract, so an out-of-range value can
        // be constructed without going through `PadId::new`. Treat it as a
        // validation error rather than letting it reach the engine.
        if pad.id.get() > PadId::MAX {
            issues.push(ValidationIssue::error(
                ValidationCode::PadIdOutOfRange,
                format!("{base}.id"),
                format!("pad id {} is outside 0..={}", pad.id.get(), PadId::MAX),
            ));
        }
        if let Some(pad_name) = &pad.name
            && pad_name.chars().count() > 64
        {
            issues.push(ValidationIssue::error(
                ValidationCode::TextTooLong,
                format!("{base}.name"),
                "pad name must be 64 characters or fewer",
            ));
        }
        if pad.layers.len() > MAX_LAYERS_PER_PAD {
            issues.push(ValidationIssue::error(
                ValidationCode::TooManyLayers,
                format!("{base}.layers"),
                format!("a pad may have at most {MAX_LAYERS_PER_PAD} layers"),
            ));
        }

        validate_float(
            &mut issues,
            pad.gain_db,
            format!("{base}.gain_db"),
            -60.0,
            24.0,
            ValidationCode::GainOutOfRange,
        );
        validate_float(
            &mut issues,
            pad.pan,
            format!("{base}.pan"),
            -1.0,
            1.0,
            ValidationCode::PanOutOfRange,
        );
        validate_float(
            &mut issues,
            pad.tune_semitones,
            format!("{base}.tune_semitones"),
            -24.0,
            24.0,
            ValidationCode::TuneOutOfRange,
        );

        if pad.layers.is_empty() {
            issues.push(ValidationIssue::warning(
                ValidationCode::SilentPad,
                base.clone(),
                "pad has no layers and will not make sound",
            ));
        }

        let mut seen_layer_ids = std::collections::BTreeSet::new();
        for (layer_index, layer) in pad.layers.iter().enumerate() {
            let layer_base = format!("{base}.layers[{layer_index}]");
            if !seen_layer_ids.insert(layer.id) {
                issues.push(ValidationIssue::error(
                    ValidationCode::DuplicateLayerId,
                    format!("{layer_base}.id"),
                    format!("layer id {} is duplicated inside this pad", layer.id),
                ));
            }
            validate_velocity(
                &mut issues,
                layer.velocity,
                format!("{layer_base}.velocity"),
            );
            validate_float(
                &mut issues,
                layer.gain_db,
                format!("{layer_base}.gain_db"),
                -60.0,
                24.0,
                ValidationCode::GainOutOfRange,
            );
            validate_float(
                &mut issues,
                layer.tune_cents,
                format!("{layer_base}.tune_cents"),
                -2400.0,
                2400.0,
                ValidationCode::TuneOutOfRange,
            );
            if let Some(loop_points) = layer.loop_points {
                if loop_points.end_frame <= loop_points.start_frame {
                    issues.push(ValidationIssue::error(
                        ValidationCode::InvalidLoopRange,
                        format!("{layer_base}.loop_points"),
                        "loop end frame must be greater than the start frame",
                    ));
                }
                if pad.playback != PlaybackMode::Loop {
                    issues.push(ValidationIssue::warning(
                        ValidationCode::LoopWithoutLoopMode,
                        format!("{layer_base}.loop_points"),
                        "loop points are ignored because the pad is not in loop mode",
                    ));
                }
            }
            if let Some(sample) = &layer.sample {
                if sample.hash.is_none() {
                    issues.push(ValidationIssue::warning(
                        ValidationCode::MissingSampleHash,
                        format!("{layer_base}.sample.hash"),
                        format!("{} has no content hash and cannot be verified", sample.path),
                    ));
                }
                if let Some(original) = &sample.original_name
                    && original.chars().count() > 255
                {
                    issues.push(ValidationIssue::error(
                        ValidationCode::TextTooLong,
                        format!("{layer_base}.sample.original_name"),
                        "original name must be 255 characters or fewer",
                    ));
                }
            }
        }
    }

    // Keyboard and MIDI mappings must be unambiguous across the kit.
    let mut keys = std::collections::BTreeMap::new();
    let mut midi = std::collections::BTreeMap::new();
    for (index, pad) in kit.pads.iter().enumerate() {
        if let Some(mapping) = pad.key
            && let Some(previous) = keys.insert(mapping.key, index)
        {
            issues.push(ValidationIssue::error(
                ValidationCode::DuplicateKeyMapping,
                format!("pads[{index}].key"),
                format!(
                    "physical key {} is also mapped by pads[{previous}]",
                    mapping.key.raw()
                ),
            ));
        }
        if let Some(mapping) = pad.midi
            && let Some(previous) = midi.insert((mapping.note, mapping.channel), index)
        {
            issues.push(ValidationIssue::error(
                ValidationCode::DuplicateMidiMapping,
                format!("pads[{index}].midi"),
                format!(
                    "MIDI note {} on channel {} is also mapped by pads[{previous}]",
                    mapping.note.get(),
                    mapping.channel.get()
                ),
            ));
        }
    }

    // Case-insensitive path collisions matter on Windows.
    let mut paths: std::collections::BTreeMap<String, Option<super::ContentHash>> =
        std::collections::BTreeMap::new();
    for (pad_index, pad) in kit.pads.iter().enumerate() {
        for (layer_index, layer) in pad.layers.iter().enumerate() {
            let Some(sample) = &layer.sample else {
                continue;
            };
            let key = sample.path.as_str().to_ascii_lowercase();
            match paths.get(&key) {
                Some(existing) if *existing != sample.hash => {
                    issues.push(ValidationIssue::error(
                        ValidationCode::SamplePathCollision,
                        format!("pads[{pad_index}].layers[{layer_index}].sample.path"),
                        format!(
                            "{} is referenced with different content hashes",
                            sample.path
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    paths.insert(key, sample.hash);
                }
            }
        }
    }

    ValidationReport { issues }
}

fn validate_velocity(issues: &mut Vec<ValidationIssue>, range: VelocityRange, path: String) {
    if !(1..=127).contains(&range.min) {
        issues.push(ValidationIssue::error(
            ValidationCode::VelocityOutOfRange,
            format!("{path}.min"),
            format!("velocity minimum {} is outside 1..=127", range.min),
        ));
    }
    if !(1..=127).contains(&range.max) {
        issues.push(ValidationIssue::error(
            ValidationCode::VelocityOutOfRange,
            format!("{path}.max"),
            format!("velocity maximum {} is outside 1..=127", range.max),
        ));
    }
    if range.min > range.max {
        issues.push(ValidationIssue::error(
            ValidationCode::VelocityRangeInverted,
            path,
            format!(
                "velocity minimum {} is greater than maximum {}",
                range.min, range.max
            ),
        ));
    }
}

fn validate_float(
    issues: &mut Vec<ValidationIssue>,
    value: f32,
    path: String,
    min: f32,
    max: f32,
    range_code: ValidationCode,
) {
    if !value.is_finite() {
        issues.push(ValidationIssue::error(
            ValidationCode::NonFiniteValue,
            path,
            "value must be finite",
        ));
    } else if value < min || value > max {
        issues.push(ValidationIssue::error(
            range_code,
            path,
            format!("value {value} is outside {min}..={max}"),
        ));
    }
}

/// Validate a kit, returning only a success/failure plus error issues.
pub fn validate_kit(kit: &Kit) -> Result<(), ValidationError> {
    ValidationError::from_report(validate_kit_report(kit))
}

#[cfg(test)]
mod tests {
    use super::super::model::{Layer, PadId, SampleLoop, SampleRef};
    use super::super::{ContentHash, Kit, Pad, RelativeSamplePath};
    use super::*;

    fn silent_kit() -> Kit {
        Kit::silent_default()
    }

    #[test]
    fn silent_default_is_valid_with_warnings() {
        let kit = silent_kit();
        assert!(kit.validate().is_ok());
        let report = validate_kit_report(&kit);
        assert!(report.is_valid());
        assert!(
            report
                .warnings()
                .any(|issue| issue.code == ValidationCode::SilentPad)
        );
    }

    #[test]
    fn duplicate_pad_ids_are_rejected() {
        let mut kit = silent_kit();
        kit.pads[1].id = PadId::new(0).unwrap();
        let error = kit.validate().unwrap_err();
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ValidationCode::DuplicatePadId)
        );
    }

    #[test]
    fn out_of_range_pad_id_is_rejected() {
        let mut kit = silent_kit();
        kit.pads[5].id = PadId(99);
        let error = kit.validate().unwrap_err();
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ValidationCode::PadIdOutOfRange)
        );
    }

    #[test]
    fn wrong_pad_count_is_rejected() {
        let mut kit = silent_kit();
        kit.pads.pop();
        let error = kit.validate().unwrap_err();
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ValidationCode::WrongPadCount)
        );
    }

    #[test]
    fn duplicate_keys_and_midi_notes_are_rejected() {
        let mut kit = silent_kit();
        kit.pads[1].key = kit.pads[0].key;
        kit.pads[1].midi = kit.pads[0].midi;
        let error = kit.validate().unwrap_err();
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ValidationCode::DuplicateKeyMapping)
        );
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ValidationCode::DuplicateMidiMapping)
        );
    }

    #[test]
    fn velocity_and_float_invariants_are_reported() {
        let mut kit = silent_kit();
        kit.pads[0].layers.push(Layer {
            velocity: VelocityRange { min: 100, max: 10 },
            gain_db: f32::NAN,
            tune_cents: 5000.0,
            ..Layer::silent(Default::default())
        });
        let error = kit.validate().unwrap_err();
        let codes: Vec<_> = error.issues.iter().map(|issue| issue.code).collect();
        assert!(codes.contains(&ValidationCode::VelocityRangeInverted));
        assert!(codes.contains(&ValidationCode::NonFiniteValue));
        assert!(codes.contains(&ValidationCode::TuneOutOfRange));
    }

    #[test]
    fn loop_points_require_ordering_and_loop_mode() {
        let mut kit = silent_kit();
        let mut layer = Layer::silent(Default::default());
        layer.loop_points = Some(SampleLoop {
            start_frame: 100,
            end_frame: 100,
        });
        kit.pads[0].layers.push(layer);
        let report = validate_kit_report(&kit);
        assert!(
            report
                .errors()
                .any(|issue| issue.code == ValidationCode::InvalidLoopRange)
        );
        assert!(
            report
                .warnings()
                .any(|issue| issue.code == ValidationCode::LoopWithoutLoopMode)
        );
    }

    #[test]
    fn sample_path_collisions_detect_different_hashes() {
        let mut kit = silent_kit();
        let path = RelativeSamplePath::new("samples/kick.wav").unwrap();
        let first = ContentHash::of_bytes(b"a");
        let second = ContentHash::of_bytes(b"b");
        kit.pads[0].layers.push(Layer::with_sample(
            Default::default(),
            SampleRef::new(path.clone(), first),
        ));
        kit.pads[1].layers.push(Layer::with_sample(
            Default::default(),
            SampleRef::new(RelativeSamplePath::new("SAMPLES/KICK.WAV").unwrap(), second),
        ));
        let error = kit.validate().unwrap_err();
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == ValidationCode::SamplePathCollision)
        );

        kit.pads[1].layers[0].sample = Some(SampleRef::new(path, first));
        assert!(kit.validate().is_ok());
    }

    #[test]
    fn pad_with_no_layers_is_a_warning_not_an_error() {
        let mut kit = silent_kit();
        kit.pads[3].layers.clear();
        let report = validate_kit_report(&kit);
        assert!(report.is_valid());
        assert!(
            report
                .warnings()
                .any(|issue| issue.code == ValidationCode::SilentPad)
        );
        assert!(kit.pads[3].is_silent());
    }

    #[test]
    fn pad_id_helpers_are_consistent() {
        let kit = silent_kit();
        let pad: &Pad = kit.pad_at(2).unwrap();
        assert_eq!(pad.id.get(), 2);
        assert_eq!(kit.pad(pad.id), Some(pad));
        assert!(kit.pad_for_key(pad.key.unwrap().key).is_some());
        let midi = pad.midi.unwrap();
        assert!(kit.pad_for_midi(midi.note, midi.channel).is_some());
    }
}
