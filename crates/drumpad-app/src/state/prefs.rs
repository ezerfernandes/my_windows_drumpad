//! The Settings screen's model: audio, MIDI, keyboard, and appearance
//! preferences, with validation and a restart-requirement explanation.
//!
//! Preferences are stored beside the foundation's `settings.json` in
//! `preferences.json`. The split is deliberate: `settings.json` is owned by the
//! foundation (`drumpad_model::config`), while the UI-domain choices live here
//! and are versioned separately. Both use the same atomic-write helper, so a
//! crash mid-save can never corrupt the last good file.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::keymap::ProfileId;

/// Current preferences schema version.
pub const PREFERENCES_SCHEMA_VERSION: u32 = 1;

/// File name inside the config directory.
pub const PREFERENCES_FILE_NAME: &str = "preferences.json";

/// Smallest buffer the UI offers, in frames.
pub const MIN_BUFFER_FRAMES: u32 = 32;
/// Largest buffer the UI offers, in frames.
pub const MAX_BUFFER_FRAMES: u32 = 2_048;

/// Sample rates offered by the settings screen.
pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];

/// Buffer sizes offered, in frames.
pub const BUFFER_SIZES: [u32; 6] = [32, 64, 128, 256, 512, 1_024];

/// Theme choices. The dark stage theme is the only shipping theme in v1, but the
/// token structure keeps a high-contrast variant honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    /// Dark, high-contrast stage theme (default).
    Stage,
    /// Same palette with brighter focus rings for very dark rooms.
    HighContrast,
}

impl ThemeChoice {
    /// All choices in menu order.
    pub const ALL: [Self; 2] = [Self::Stage, Self::HighContrast];

    /// Menu label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage (dark)",
            Self::HighContrast => "High contrast",
        }
    }
}

/// Audio preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPrefs {
    /// Device id or display name. `None` uses the system default.
    pub device: Option<String>,
    /// Requested sample rate.
    pub sample_rate: u32,
    /// Requested buffer size in frames.
    pub buffer_frames: u32,
    /// Whether WASAPI exclusive mode is requested.
    pub exclusive: bool,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48_000,
            buffer_frames: 128,
            exclusive: false,
        }
    }
}

/// MIDI input preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiPrefs {
    /// Selected port name. `None` means no MIDI device.
    pub device: Option<String>,
    /// MIDI channel filter, `0` for omni, otherwise `1..=16`.
    pub channel: u8,
}

impl Default for MidiPrefs {
    fn default() -> Self {
        Self {
            device: None,
            channel: 0,
        }
    }
}

/// Keyboard preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardPrefs {
    /// Active profile.
    pub profile: ProfileId,
    /// Show the key label on each pad.
    pub show_key_labels: bool,
    /// Pad surface size.
    pub grid_size: GridSize,
}

impl Default for KeyboardPrefs {
    fn default() -> Self {
        Self {
            profile: ProfileId(2),
            show_key_labels: true,
            grid_size: GridSize::ThreeByThree,
        }
    }
}

/// How many pads the performance surface shows and routes.
///
/// The kit always owns 16 pads; the 3x3 surface shows and routes the first nine
/// and leaves the rest available to MIDI and kit data. 3x3 is the default so a
/// fresh install matches the one-hand reference workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GridSize {
    /// 16 pads in a 4x4 grid.
    FourByFour,
    /// 9 pads in a 3x3 grid. The default.
    #[default]
    ThreeByThree,
}

impl GridSize {
    /// Both choices in menu order.
    pub const ALL: [Self; 2] = [Self::FourByFour, Self::ThreeByThree];

    /// Grid columns.
    pub const fn columns(self) -> usize {
        match self {
            Self::FourByFour => 4,
            Self::ThreeByThree => 3,
        }
    }

    /// Grid rows.
    pub const fn rows(self) -> usize {
        match self {
            Self::FourByFour => 4,
            Self::ThreeByThree => 3,
        }
    }

    /// Number of pads on the surface.
    pub const fn pad_count(self) -> usize {
        self.columns() * self.rows()
    }

    /// Menu label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::FourByFour => "4x4 (16 pads)",
            Self::ThreeByThree => "3x3 (9 pads)",
        }
    }

    /// Key labels for the surface, row-major.
    pub const fn key_labels(self) -> &'static [&'static str] {
        match self {
            Self::FourByFour => &[
                "1", "2", "3", "4", "Q", "W", "E", "R", "A", "S", "D", "F", "Z", "X", "C", "V",
            ],
            Self::ThreeByThree => &["Q", "W", "E", "A", "S", "D", "Z", "X", "C"],
        }
    }
}

/// Appearance preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearancePrefs {
    /// Theme choice.
    pub theme: ThemeChoice,
    /// Global scale on top of monitor DPI, `0.75..=2.0`.
    pub scale: f32,
    /// Show MIDI note hints on pads.
    pub show_midi_hints: bool,
    /// Reduce hit-flash animation for photosensitivity.
    pub reduce_motion: bool,
}

impl Default for AppearancePrefs {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Stage,
            scale: 1.0,
            show_midi_hints: true,
            reduce_motion: false,
        }
    }
}

/// The full preferences document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiPreferences {
    /// Schema version this document was written with.
    pub schema_version: u32,
    /// Audio choices.
    pub audio: AudioPrefs,
    /// MIDI choices.
    pub midi: MidiPrefs,
    /// Keyboard choices.
    pub keyboard: KeyboardPrefs,
    /// Appearance choices.
    pub appearance: AppearancePrefs,
    /// Show the diagnostics block on the settings screen.
    pub show_diagnostics: bool,
    /// Whether the first-run wizard has been completed or skipped.
    pub wizard_completed: bool,
    /// Folder the wizard and the import flow start in.
    pub sample_folder: Option<PathBuf>,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            audio: AudioPrefs::default(),
            midi: MidiPrefs::default(),
            keyboard: KeyboardPrefs::default(),
            appearance: AppearancePrefs::default(),
            show_diagnostics: false,
            wizard_completed: false,
            sample_folder: None,
        }
    }
}

/// Range violation with a user-facing reason.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsError {
    /// Unsupported sample rate.
    SampleRate {
        /// Requested rate.
        value: u32,
        /// Supported rates.
        allowed: Vec<u32>,
    },
    /// Buffer outside the offered range.
    BufferSize {
        /// Requested frames.
        value: u32,
        /// Smallest supported.
        min: u32,
        /// Largest supported.
        max: u32,
    },
    /// Buffer is not a power of two.
    BufferNotPowerOfTwo {
        /// Requested frames.
        value: u32,
    },
    /// MIDI channel is outside omni + 1..=16.
    MidiChannel {
        /// Requested channel.
        value: u8,
    },
    /// UI scale is out of range or not finite.
    UiScale {
        /// Requested scale.
        value: f32,
    },
    /// Exclusive mode is not implemented in this build.
    ExclusiveNotImplemented,
    /// A device name is present but blank.
    BlankDevice,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SampleRate { value, allowed } => {
                write!(
                    formatter,
                    "sample rate {value} Hz is not supported; use one of {allowed:?}"
                )
            }
            Self::BufferSize { value, min, max } => {
                write!(formatter, "buffer {value} frames is outside {min}..={max}")
            }
            Self::BufferNotPowerOfTwo { value } => {
                write!(formatter, "buffer {value} frames is not a power of two")
            }
            Self::MidiChannel { value } => {
                write!(formatter, "MIDI channel {value} is not 0 (omni) or 1..=16")
            }
            Self::UiScale { value } => {
                write!(formatter, "UI scale {value} is outside 0.75..=2.0")
            }
            Self::ExclusiveNotImplemented => {
                formatter.write_str("exclusive mode is not implemented in this build")
            }
            Self::BlankDevice => formatter.write_str("a device name cannot be blank"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Why a change needs an app restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartReason {
    /// Output device changed.
    AudioDevice,
    /// Sample rate changed.
    SampleRate,
    /// Buffer size changed.
    BufferSize,
    /// Exclusive/shared mode changed.
    AudioMode,
}

impl RestartReason {
    /// Explanation shown under the Apply button.
    pub const fn explanation(self) -> &'static str {
        match self {
            Self::AudioDevice => {
                "The output device is opened at startup, so the new device is used after a restart."
            }
            Self::SampleRate => {
                "The audio stream is built for one sample rate; restart to rebuild it."
            }
            Self::BufferSize => {
                "The audio stream is built for one buffer size; restart to rebuild it."
            }
            Self::AudioMode => "Exclusive and shared modes need a new stream; restart to switch.",
        }
    }
}

impl UiPreferences {
    /// Check every value the screen can produce.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if !SAMPLE_RATES.contains(&self.audio.sample_rate) {
            return Err(SettingsError::SampleRate {
                value: self.audio.sample_rate,
                allowed: SAMPLE_RATES.to_vec(),
            });
        }
        if !(MIN_BUFFER_FRAMES..=MAX_BUFFER_FRAMES).contains(&self.audio.buffer_frames) {
            return Err(SettingsError::BufferSize {
                value: self.audio.buffer_frames,
                min: MIN_BUFFER_FRAMES,
                max: MAX_BUFFER_FRAMES,
            });
        }
        if !self.audio.buffer_frames.is_power_of_two() {
            return Err(SettingsError::BufferNotPowerOfTwo {
                value: self.audio.buffer_frames,
            });
        }
        if self.midi.channel > 16 {
            return Err(SettingsError::MidiChannel {
                value: self.midi.channel,
            });
        }
        if let Some(device) = &self.audio.device
            && device.trim().is_empty()
        {
            return Err(SettingsError::BlankDevice);
        }
        if let Some(device) = &self.midi.device
            && device.trim().is_empty()
        {
            return Err(SettingsError::BlankDevice);
        }
        if !self.appearance.scale.is_finite() || !(0.75..=2.0).contains(&self.appearance.scale) {
            return Err(SettingsError::UiScale {
                value: self.appearance.scale,
            });
        }
        // The audio backend lands in E02. Until WASAPI exclusive is implemented,
        // offering it would let the user select a mode that cannot open a
        // stream. Move this gate into `drumpad-audio` when it ships.
        if self.audio.exclusive {
            return Err(SettingsError::ExclusiveNotImplemented);
        }
        Ok(())
    }

    /// Which restart-requiring fields differ from `other`.
    pub fn restart_reasons(&self, other: &Self) -> Vec<RestartReason> {
        let mut reasons = Vec::new();
        if self.audio.device != other.audio.device {
            reasons.push(RestartReason::AudioDevice);
        }
        if self.audio.sample_rate != other.audio.sample_rate {
            reasons.push(RestartReason::SampleRate);
        }
        if self.audio.buffer_frames != other.audio.buffer_frames {
            reasons.push(RestartReason::BufferSize);
        }
        if self.audio.exclusive != other.audio.exclusive {
            reasons.push(RestartReason::AudioMode);
        }
        reasons
    }

    /// Path of the preferences document in a config directory.
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join(PREFERENCES_FILE_NAME)
    }

    /// Load preferences, returning defaults when the file is absent.
    ///
    /// Corrupt, unknown-field, and future-version files return an error instead
    /// of being overwritten, matching the foundation's settings policy.
    pub fn load(path: &Path) -> Result<Self, PreferencesError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let preferences: Self =
                    serde_json::from_str(&text).map_err(|error| PreferencesError::Invalid {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    })?;
                if preferences.schema_version != PREFERENCES_SCHEMA_VERSION {
                    return Err(PreferencesError::UnsupportedVersion {
                        found: preferences.schema_version,
                        supported: PREFERENCES_SCHEMA_VERSION,
                    });
                }
                preferences
                    .validate()
                    .map_err(|error| PreferencesError::Invalid {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    })?;
                Ok(preferences)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(PreferencesError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            }),
        }
    }

    /// Validate and atomically persist.
    pub fn save(&self, path: &Path) -> Result<(), PreferencesError> {
        self.validate().map_err(|error| PreferencesError::Invalid {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let json =
            serde_json::to_string_pretty(self).map_err(|error| PreferencesError::Invalid {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        // Reuse the foundation's atomic writer (temp file, flush, rename).
        crate::settings::write_atomic(path, format!("{json}\n").as_bytes()).map_err(|error| {
            PreferencesError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            }
        })
    }
}

/// Preferences persistence error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreferencesError {
    /// The file could not be read or written.
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying reason.
        message: String,
    },
    /// The file is not a valid preferences document.
    Invalid {
        /// Offending path.
        path: PathBuf,
        /// Underlying reason.
        message: String,
    },
    /// The file was written by a newer build.
    UnsupportedVersion {
        /// Version in the file.
        found: u32,
        /// Version this build understands.
        supported: u32,
    },
}

impl fmt::Display for PreferencesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "could not access {}: {message}", path.display())
            }
            Self::Invalid { path, message } => {
                write!(
                    formatter,
                    "{} is not a valid preferences file: {message}",
                    path.display()
                )
            }
            Self::UnsupportedVersion { found, supported } => write!(
                formatter,
                "preferences version {found} is newer than the supported version {supported}"
            ),
        }
    }
}

impl std::error::Error for PreferencesError {}

/// The Settings screen's editing session.
///
/// The draft is edited directly; [`Self::apply`] validates and returns the
/// preferences to persist. A rejected apply leaves the persisted copy untouched,
/// which is what "invalid combinations are blocked" means in practice.
#[derive(Debug, Clone)]
pub struct SettingsSession {
    saved: UiPreferences,
    draft: UiPreferences,
    last_error: Option<SettingsError>,
}

impl SettingsSession {
    /// Start from the persisted preferences.
    pub fn new(saved: UiPreferences) -> Self {
        Self {
            draft: saved.clone(),
            saved,
            last_error: None,
        }
    }

    /// The persisted preferences.
    pub fn saved(&self) -> &UiPreferences {
        &self.saved
    }

    /// The editable draft.
    pub fn draft(&self) -> &UiPreferences {
        &self.draft
    }

    /// Mutable access for the widgets.
    pub fn draft_mut(&mut self) -> &mut UiPreferences {
        &mut self.draft
    }

    /// Whether the draft differs from the persisted copy.
    pub fn is_dirty(&self) -> bool {
        self.draft != self.saved
    }

    /// The last validation error, cleared by a successful apply.
    pub fn last_error(&self) -> Option<&SettingsError> {
        self.last_error.as_ref()
    }

    /// Restart requirements introduced by the draft.
    pub fn restart_reasons(&self) -> Vec<RestartReason> {
        self.draft.restart_reasons(&self.saved)
    }

    /// Validate and accept the draft.
    pub fn apply(&mut self) -> Result<&UiPreferences, SettingsError> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_are_valid_and_target_the_reference_rate() {
        let preferences = UiPreferences::default();
        assert_eq!(preferences.validate(), Ok(()));
        assert_eq!(preferences.audio.sample_rate, 48_000);
        assert_eq!(preferences.audio.buffer_frames, 128);
        assert_eq!(preferences.schema_version, PREFERENCES_SCHEMA_VERSION);
    }

    #[test]
    fn unsupported_rates_are_blocked_with_the_allowed_list() {
        let mut preferences = UiPreferences::default();
        preferences.audio.sample_rate = 22_050;
        let error = preferences.validate().expect_err("rate");
        assert!(matches!(
            error,
            SettingsError::SampleRate { value: 22_050, .. }
        ));
        assert!(error.to_string().contains("44100"));
    }

    #[test]
    fn buffer_sizes_must_be_supported_and_powers_of_two() {
        let mut preferences = UiPreferences::default();
        preferences.audio.buffer_frames = 100;
        assert!(matches!(
            preferences.validate(),
            Err(SettingsError::BufferNotPowerOfTwo { value: 100 })
        ));
        preferences.audio.buffer_frames = 4_096;
        assert!(matches!(
            preferences.validate(),
            Err(SettingsError::BufferSize { .. })
        ));
        for size in BUFFER_SIZES {
            preferences.audio.buffer_frames = size;
            assert_eq!(preferences.validate(), Ok(()), "{size}");
        }
    }

    #[test]
    fn midi_channel_is_omni_or_one_to_sixteen() {
        let mut preferences = UiPreferences::default();
        for channel in 0..=16 {
            preferences.midi.channel = channel;
            assert_eq!(preferences.validate(), Ok(()));
        }
        preferences.midi.channel = 17;
        assert!(matches!(
            preferences.validate(),
            Err(SettingsError::MidiChannel { value: 17 })
        ));
    }

    #[test]
    fn ui_scale_matches_the_foundation_range() {
        let mut preferences = UiPreferences::default();
        for scale in [0.75, 1.0, 1.5, 2.0] {
            preferences.appearance.scale = scale;
            assert_eq!(preferences.validate(), Ok(()));
        }
        for scale in [0.5, 2.5, f32::NAN] {
            preferences.appearance.scale = scale;
            assert!(matches!(
                preferences.validate(),
                Err(SettingsError::UiScale { .. })
            ));
        }
    }

    #[test]
    fn the_pad_grid_choice_defaults_to_3x3_and_round_trips() {
        assert_eq!(
            UiPreferences::default().keyboard.grid_size,
            GridSize::ThreeByThree
        );
        // Preferences written before the field existed must load as 3x3.
        let legacy: UiPreferences = serde_json::from_str(
            r#"{"schema_version":1,"keyboard":{"profile":2,"show_key_labels":true}}"#,
        )
        .expect("legacy preferences");
        assert_eq!(legacy.keyboard.grid_size, GridSize::ThreeByThree);
        let legacy_whole_section: UiPreferences =
            serde_json::from_str(r#"{"schema_version":1}"#).expect("legacy preferences");
        assert_eq!(
            legacy_whole_section.keyboard.grid_size,
            GridSize::ThreeByThree
        );

        let mut preferences = UiPreferences::default();
        preferences.keyboard.grid_size = GridSize::FourByFour;
        let json = serde_json::to_string(&preferences).expect("serialize");
        let decoded: UiPreferences = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.keyboard.grid_size, GridSize::FourByFour);
        assert_eq!(decoded, preferences);
        assert_eq!(GridSize::ThreeByThree.pad_count(), 9);
        assert_eq!(GridSize::FourByFour.pad_count(), 16);
    }
    #[test]
    fn exclusive_mode_is_blocked_until_the_backend_supports_it() {
        let mut preferences = UiPreferences::default();
        preferences.audio.exclusive = true;
        assert_eq!(
            preferences.validate(),
            Err(SettingsError::ExclusiveNotImplemented)
        );
        assert!(
            SettingsError::ExclusiveNotImplemented
                .to_string()
                .contains("not implemented")
        );
    }

    #[test]
    fn a_blank_device_name_is_rejected() {
        let mut preferences = UiPreferences::default();
        preferences.audio.device = Some("   ".to_owned());
        assert_eq!(preferences.validate(), Err(SettingsError::BlankDevice));
        preferences.audio.device = Some("Speakers (Realtek)".to_owned());
        assert_eq!(preferences.validate(), Ok(()));
    }

    #[test]
    fn restart_reasons_cover_every_audio_stream_field() {
        let base = UiPreferences::default();
        let mut changed = base.clone();
        changed.audio.device = Some("USB DAC".to_owned());
        changed.audio.sample_rate = 96_000;
        changed.audio.buffer_frames = 256;
        changed.audio.exclusive = true;
        let reasons = changed.restart_reasons(&base);
        assert_eq!(reasons.len(), 4);
        for reason in reasons {
            assert!(!reason.explanation().is_empty());
        }
    }

    #[test]
    fn appearance_changes_do_not_require_a_restart() {
        let base = UiPreferences::default();
        let mut changed = base.clone();
        changed.appearance.theme = ThemeChoice::HighContrast;
        changed.appearance.scale = 1.5;
        changed.keyboard.show_key_labels = false;
        assert!(changed.restart_reasons(&base).is_empty());
    }

    #[test]
    fn preferences_round_trip_atomically() {
        let temp = tempdir().expect("temp");
        let path = UiPreferences::path_in(temp.path());
        let mut preferences = UiPreferences::default();
        preferences.audio.sample_rate = 96_000;
        preferences.midi.channel = 10;
        preferences.appearance.theme = ThemeChoice::HighContrast;
        preferences.wizard_completed = true;

        preferences.save(&path).expect("save");
        assert_eq!(UiPreferences::load(&path).expect("load"), preferences);

        preferences.appearance.scale = 1.25;
        preferences.save(&path).expect("replace");
        assert_eq!(
            UiPreferences::load(&path).expect("load replaced"),
            preferences
        );

        let leftovers: Vec<_> = std::fs::read_dir(temp.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_missing_file_loads_defaults_without_writing() {
        let temp = tempdir().expect("temp");
        let path = temp.path().join(PREFERENCES_FILE_NAME);
        assert_eq!(
            UiPreferences::load(&path).expect("defaults"),
            UiPreferences::default()
        );
        assert!(!path.exists());
    }

    #[test]
    fn invalid_values_are_never_written() {
        let temp = tempdir().expect("temp");
        let path = temp.path().join(PREFERENCES_FILE_NAME);
        let mut preferences = UiPreferences::default();
        preferences.audio.sample_rate = 1;
        assert!(preferences.save(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_and_future_files_are_reported_not_overwritten() {
        let temp = tempdir().expect("temp");
        let path = temp.path().join(PREFERENCES_FILE_NAME);
        std::fs::write(&path, b"{ not json").expect("write");
        assert!(matches!(
            UiPreferences::load(&path),
            Err(PreferencesError::Invalid { .. })
        ));
        assert_eq!(std::fs::read(&path).expect("still there"), b"{ not json");

        std::fs::write(&path, br#"{"schema_version":99}"#).expect("write future");
        assert!(matches!(
            UiPreferences::load(&path),
            Err(PreferencesError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let temp = tempdir().expect("temp");
        let path = temp.path().join(PREFERENCES_FILE_NAME);
        std::fs::write(&path, br#"{"schema_version":1,"surprise":true}"#).expect("write");
        assert!(matches!(
            UiPreferences::load(&path),
            Err(PreferencesError::Invalid { .. })
        ));
    }

    #[test]
    fn the_session_blocks_an_invalid_apply_and_keeps_the_saved_copy() {
        let mut session = SettingsSession::new(UiPreferences::default());
        session.draft_mut().audio.buffer_frames = 100;
        assert!(session.is_dirty());
        let error = session.apply().expect_err("invalid");
        assert!(matches!(error, SettingsError::BufferNotPowerOfTwo { .. }));
        assert_eq!(session.saved().audio.buffer_frames, 128);
        assert_eq!(session.last_error(), Some(&error));
    }

    #[test]
    fn the_session_applies_and_reports_restart_requirements() {
        let mut session = SettingsSession::new(UiPreferences::default());
        session.draft_mut().audio.sample_rate = 96_000;
        let reasons = session.restart_reasons();
        assert_eq!(reasons, vec![RestartReason::SampleRate]);
        let applied = session.apply().expect("apply").clone();
        assert_eq!(applied.audio.sample_rate, 96_000);
        assert!(!session.is_dirty());
        assert_eq!(session.last_error(), None);
        assert!(session.restart_reasons().is_empty());
    }

    #[test]
    fn revert_restores_the_saved_preferences() {
        let mut session = SettingsSession::new(UiPreferences::default());
        session.draft_mut().audio.buffer_frames = 256;
        session.revert();
        assert!(!session.is_dirty());
        assert_eq!(session.draft().audio.buffer_frames, 128);
    }
}
