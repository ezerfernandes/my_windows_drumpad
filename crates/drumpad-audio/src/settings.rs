//! Persisted audio settings with validation and safe device fallback.
//!
//! The file is plain JSON with a schema version. Loading never fails hard: a
//! missing or corrupt file yields defaults plus warnings, so a bad settings
//! file can never keep the app from starting.

use crate::AudioError;
use crate::devices::DeviceInfo;
use crate::stream::{ShareMode, StreamConfig};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Current settings schema version.
pub const SETTINGS_VERSION: u32 = 1;

/// Persisted audio configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// Schema version.
    pub version: u32,
    /// Last selected endpoint id.
    pub device_id: Option<String>,
    /// Last selected endpoint name, used when the id is gone.
    pub device_name: Option<String>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Buffer/period in frames.
    pub buffer_frames: u32,
    /// Sharing mode.
    pub share_mode: ShareMode,
    /// Use the low-latency shared period when the driver supports it.
    pub low_latency_shared: bool,
    /// Fall back to shared mode when exclusive mode fails.
    pub fallback_to_shared: bool,
    /// Preferred exclusive-mode bit depth.
    pub exclusive_bits: u16,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            device_id: None,
            device_name: None,
            sample_rate: 48_000,
            buffer_frames: 128,
            // Exclusive mode is the latency default. The backend spike measured
            // 1126-frame buffer / 512-frame period (~23.5 ms software path) in
            // shared mode against 256-frame buffer and period (~5.3 ms) in
            // exclusive mode on the reference endpoint. Exclusive mode blocks
            // other applications, so `fallback_to_shared` stays on: if the
            // endpoint is busy or rejects integer formats the stream reopens in
            // shared mode and the status bar reports the fallback.
            share_mode: ShareMode::Exclusive,
            low_latency_shared: true,
            fallback_to_shared: true,
            exclusive_bits: 16,
        }
    }
}

impl AudioSettings {
    /// Validate and clamp fields. Returns human-readable warnings for every
    /// change so the UI can explain a fallback.
    pub fn validate(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.version != SETTINGS_VERSION {
            warnings.push(format!(
                "settings version {} replaced with {}",
                self.version, SETTINGS_VERSION
            ));
            self.version = SETTINGS_VERSION;
        }
        if !(8_000..=192_000).contains(&self.sample_rate) {
            warnings.push(format!(
                "sample rate {} is out of range; using 48000",
                self.sample_rate
            ));
            self.sample_rate = 48_000;
        }
        if !(16..=4096).contains(&self.buffer_frames) {
            warnings.push(format!(
                "buffer size {} is out of range; using 128",
                self.buffer_frames
            ));
            self.buffer_frames = 128;
        }
        if self.exclusive_bits != 16 && self.exclusive_bits != 24 {
            warnings.push(format!(
                "exclusive bit depth {} is unsupported; using 16",
                self.exclusive_bits
            ));
            self.exclusive_bits = 16;
        }
        if self.device_id.as_deref() == Some("") {
            self.device_id = None;
        }
        if self.device_name.as_deref() == Some("") {
            self.device_name = None;
        }
        warnings
    }

    /// Build a stream request from these settings.
    pub fn stream_config(&self) -> StreamConfig {
        StreamConfig {
            device_id: self.device_id.clone(),
            sample_rate: self.sample_rate,
            channels: 2,
            buffer_frames: self.buffer_frames,
            share_mode: self.share_mode,
            low_latency_shared: self.low_latency_shared,
            fallback_to_shared: self.fallback_to_shared,
            exclusive_bits: self.exclusive_bits,
        }
    }

    /// Capture the negotiated format of a running stream.
    pub fn from_stream_config(config: &StreamConfig, device_name: Option<String>) -> Self {
        Self {
            version: SETTINGS_VERSION,
            device_id: config.device_id.clone(),
            device_name,
            sample_rate: config.sample_rate,
            buffer_frames: config.buffer_frames,
            share_mode: config.share_mode,
            low_latency_shared: config.low_latency_shared,
            fallback_to_shared: config.fallback_to_shared,
            exclusive_bits: config.exclusive_bits,
        }
    }
}

/// Result of loading settings, including non-fatal warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsLoad {
    /// Validated settings.
    pub settings: AudioSettings,
    /// Changes made during loading and validation.
    pub warnings: Vec<String>,
    /// True when the file was missing or unreadable and defaults were used.
    pub used_defaults: bool,
}

/// Load settings from `path`. Missing or corrupt files yield defaults.
pub fn load(path: impl AsRef<Path>) -> SettingsLoad {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<AudioSettings>(&text) {
            Ok(mut settings) => {
                let mut warnings = settings.validate();
                if warnings.is_empty() {
                    warnings.push(format!("loaded audio settings from {}", path.display()));
                }
                SettingsLoad {
                    settings,
                    warnings,
                    used_defaults: false,
                }
            }
            Err(error) => SettingsLoad {
                settings: AudioSettings::default(),
                warnings: vec![format!(
                    "audio settings at {} are invalid ({error}); using defaults",
                    path.display()
                )],
                used_defaults: true,
            },
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SettingsLoad {
            settings: AudioSettings::default(),
            warnings: vec![format!(
                "no audio settings at {}; using defaults",
                path.display()
            )],
            used_defaults: true,
        },
        Err(error) => SettingsLoad {
            settings: AudioSettings::default(),
            warnings: vec![format!(
                "could not read audio settings at {} ({error}); using defaults",
                path.display()
            )],
            used_defaults: true,
        },
    }
}

/// Save settings atomically: write a sibling temp file, flush, then rename.
pub fn save(path: impl AsRef<Path>, settings: &AudioSettings) -> Result<(), AudioError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut validated = settings.clone();
    validated.validate();
    let json = serde_json::to_string_pretty(&validated)?;
    let temp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
    }
    // Windows rename cannot replace an existing file, so remove the old one
    // first. The temp file still contains the complete new content.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// Choose the device to open from persisted settings.
///
/// Order: saved id → saved name → system default → first active device.
pub fn resolve_device(
    settings: &AudioSettings,
    devices: &[DeviceInfo],
) -> Result<(DeviceInfo, bool), AudioError> {
    let had_preference = settings.device_id.is_some() || settings.device_name.is_some();
    if let Some(id) = settings.device_id.as_deref()
        && let Some(device) = devices.iter().find(|device| device.id == id)
        && device.state.is_usable()
    {
        return Ok((device.clone(), false));
    }
    if let Some(name) = settings.device_name.as_deref()
        && let Some(device) = devices
            .iter()
            .find(|device| device.name == name && device.state.is_usable())
    {
        return Ok((device.clone(), true));
    }
    if let Some(device) = devices
        .iter()
        .find(|device| device.is_default && device.state.is_usable())
    {
        return Ok((device.clone(), had_preference));
    }
    devices
        .iter()
        .find(|device| device.state.is_usable())
        .cloned()
        .map(|device| (device, had_preference))
        .ok_or(AudioError::NoDevice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::DeviceState;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "drumpad-audio-settings-{}-{}.json",
            std::process::id(),
            name
        ));
        path
    }

    fn device(id: &str, name: &str, is_default: bool, state: DeviceState) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: name.into(),
            is_default,
            state,
        }
    }

    #[test]
    fn defaults_validate_without_warnings() {
        let mut settings = AudioSettings::default();
        assert!(settings.validate().is_empty());
        assert_eq!(settings.sample_rate, 48_000);
        assert_eq!(settings.buffer_frames, 128);
    }

    #[test]
    fn the_default_request_asks_for_exclusive_mode_with_a_shared_fallback() {
        let settings = AudioSettings::default();
        let request = settings.stream_config();
        assert_eq!(request.share_mode, ShareMode::Exclusive);
        assert!(
            request.fallback_to_shared,
            "exclusive mode must never make the app silent when the endpoint is busy"
        );
        assert_eq!(request.sample_rate, 48_000);
        assert_eq!(request.buffer_frames, 128);
    }

    #[test]
    fn invalid_values_are_clamped_with_warnings() {
        let mut settings = AudioSettings {
            sample_rate: 1,
            buffer_frames: 99_999,
            exclusive_bits: 12,
            ..AudioSettings::default()
        };
        let warnings = settings.validate();
        assert_eq!(settings.sample_rate, 48_000);
        assert_eq!(settings.buffer_frames, 128);
        assert_eq!(settings.exclusive_bits, 16);
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let settings = AudioSettings {
            device_id: Some("endpoint-1".into()),
            device_name: Some("Speakers".into()),
            sample_rate: 44_100,
            buffer_frames: 256,
            share_mode: ShareMode::Exclusive,
            exclusive_bits: 24,
            ..AudioSettings::default()
        };
        save(&path, &settings).unwrap();
        let loaded = load(&path);
        assert!(!loaded.used_defaults);
        assert_eq!(loaded.settings.sample_rate, 44_100);
        assert_eq!(loaded.settings.buffer_frames, 256);
        assert_eq!(loaded.settings.share_mode, ShareMode::Exclusive);
        assert_eq!(loaded.settings.exclusive_bits, 24);
        assert_eq!(loaded.settings.device_id.as_deref(), Some("endpoint-1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        let loaded = load(&path);
        assert!(loaded.used_defaults);
        assert_eq!(loaded.settings, AudioSettings::default());
    }

    #[test]
    fn corrupt_file_yields_defaults_with_a_warning() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = load(&path);
        assert!(loaded.used_defaults);
        assert!(loaded.warnings[0].contains("invalid"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_prefers_saved_id_then_name_then_default() {
        let devices = vec![
            device("a", "Speakers A", false, DeviceState::Active),
            device("b", "Headset B", true, DeviceState::Active),
        ];
        let mut settings = AudioSettings {
            device_id: Some("a".into()),
            ..AudioSettings::default()
        };
        assert_eq!(resolve_device(&settings, &devices).unwrap().0.id, "a");

        settings.device_id = Some("gone".into());
        settings.device_name = Some("Headset B".into());
        let (resolved, fallback) = resolve_device(&settings, &devices).unwrap();
        assert_eq!(resolved.id, "b");
        assert!(fallback);

        settings.device_id = None;
        settings.device_name = Some("also gone".into());
        let (resolved, fallback) = resolve_device(&settings, &devices).unwrap();
        assert_eq!(resolved.id, "b");
        assert!(fallback);
    }

    #[test]
    fn resolve_skips_unusable_devices() {
        let devices = vec![
            device("a", "Speakers A", true, DeviceState::Unplugged),
            device("b", "Headset B", false, DeviceState::Active),
        ];
        let settings = AudioSettings {
            device_id: Some("a".into()),
            ..AudioSettings::default()
        };
        let (resolved, fallback) = resolve_device(&settings, &devices).unwrap();
        assert_eq!(resolved.id, "b");
        assert!(fallback);
    }

    #[test]
    fn resolve_errors_when_nothing_is_usable() {
        let devices = vec![device("a", "Speakers A", true, DeviceState::NotPresent)];
        let error = resolve_device(&AudioSettings::default(), &devices).unwrap_err();
        assert!(matches!(error, AudioError::NoDevice));
    }
}
