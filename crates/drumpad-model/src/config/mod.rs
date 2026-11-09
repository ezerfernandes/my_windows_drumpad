//! Typed, versioned application settings.
//!
//! The schema deliberately covers only what the foundation owns: startup
//! logging and high-level UI preferences. Audio, input, and kit settings are
//! added by the crates that own those domains; every change bumps
//! [`SETTINGS_SCHEMA_VERSION`] and ships a migration.
//!
//! Settings are stored as JSON with a `schema_version` field. Unknown fields
//! and unknown versions are rejected so a typo or a newer file never silently
//! changes behaviour. [`AppSettings::validate`] enforces the value ranges that
//! serde cannot express.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Result};

/// Current on-disk settings schema version.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Shortest log retention the app accepts, in days.
pub const MIN_LOG_RETENTION_DAYS: u32 = 1;

/// Longest log retention the app accepts, in days.
pub const MAX_LOG_RETENTION_DAYS: u32 = 365;

/// Smallest UI scale the app accepts.
pub const MIN_UI_SCALE: f32 = 0.75;

/// Largest UI scale the app accepts.
pub const MAX_UI_SCALE: f32 = 2.0;

/// Verbosity of the local log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Only errors.
    Error,
    /// Warnings and errors.
    Warn,
    /// Normal operational detail; the default.
    #[default]
    Info,
    /// Developer detail.
    Debug,
    /// Everything, including per-event detail.
    Trace,
}

impl LogLevel {
    /// Every level, ordered from least to most verbose.
    pub const ALL: [Self; 5] = [
        Self::Error,
        Self::Warn,
        Self::Info,
        Self::Debug,
        Self::Trace,
    ];

    /// Lowercase name used in settings, `RUST_LOG`, and tracing filters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for LogLevel {
    type Err = ConfigError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err(ConfigError::InvalidLogLevel {
                value: value.to_owned(),
                allowed: Self::ALL.iter().map(|level| level.as_str()).collect(),
            }),
        }
    }
}

/// Log file behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingSettings {
    /// Minimum level written to the local log file.
    pub level: LogLevel,
    /// Days of daily log files to keep on disk.
    pub retention_days: u32,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            retention_days: 14,
        }
    }
}

/// High-level UI preferences owned by the foundation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// Global scale multiplier applied on top of the monitor DPI.
    pub scale: f32,
    /// Whether pads show the key that triggers them.
    pub show_key_labels: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            scale: 1.0,
            show_key_labels: true,
        }
    }
}

/// The complete on-disk settings document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    /// Schema version this document was written with.
    pub schema_version: u32,
    /// Log file behavior.
    #[serde(default)]
    pub logging: LoggingSettings,
    /// UI preferences.
    #[serde(default)]
    pub ui: UiSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            logging: LoggingSettings::default(),
            ui: UiSettings::default(),
        }
    }
}

impl AppSettings {
    /// Whether this document matches the schema version the build understands.
    #[must_use]
    pub const fn has_supported_schema_version(&self) -> bool {
        self.schema_version == SETTINGS_SCHEMA_VERSION
    }

    /// Reject values that are out of range.
    ///
    /// Call this before writing settings and after reading them so a hand-edited
    /// file cannot put the app into an unsupported state.
    pub fn validate(&self) -> std::result::Result<(), ConfigError> {
        if !(MIN_LOG_RETENTION_DAYS..=MAX_LOG_RETENTION_DAYS).contains(&self.logging.retention_days)
        {
            return Err(ConfigError::InvalidLogRetention {
                value: self.logging.retention_days,
                min: MIN_LOG_RETENTION_DAYS,
                max: MAX_LOG_RETENTION_DAYS,
            });
        }

        if !self.ui.scale.is_finite() || !(MIN_UI_SCALE..=MAX_UI_SCALE).contains(&self.ui.scale) {
            return Err(ConfigError::InvalidUiScale {
                value: self.ui.scale,
                min: MIN_UI_SCALE,
                max: MAX_UI_SCALE,
            });
        }

        Ok(())
    }

    /// Validate and convert a schema mismatch into a typed error.
    pub fn check(&self) -> Result<()> {
        if !self.has_supported_schema_version() {
            return Err(crate::error::Error::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: SETTINGS_SCHEMA_VERSION,
            });
        }
        self.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_valid_and_current() {
        let settings = AppSettings::default();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(settings.has_supported_schema_version());
        assert_eq!(settings.validate(), Ok(()));
        assert_eq!(settings.logging.level, LogLevel::Info);
        assert!(settings.ui.show_key_labels);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let mut settings = AppSettings::default();
        settings.logging.level = LogLevel::Debug;
        settings.logging.retention_days = 30;
        settings.ui.scale = 1.25;
        settings.ui.show_key_labels = false;

        let encoded = serde_json::to_string_pretty(&settings).expect("settings serialize");
        let decoded: AppSettings = serde_json::from_str(&encoded).expect("settings parse");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn missing_optional_sections_fall_back_to_defaults() {
        let decoded: AppSettings =
            serde_json::from_str(r#"{"schema_version":1}"#).expect("settings parse");
        assert_eq!(decoded.logging, LoggingSettings::default());
        assert_eq!(decoded.ui, UiSettings::default());
    }

    #[test]
    fn schema_version_is_required() {
        let error = serde_json::from_str::<AppSettings>("{}").expect_err("version is required");
        assert!(error.to_string().contains("schema_version"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let error = serde_json::from_str::<AppSettings>(
            r#"{"schema_version":1,"logging":{"level":"info","retention_days":7,"typo":true}}"#,
        )
        .expect_err("unknown fields must be rejected");
        assert!(error.to_string().contains("typo"));
    }

    #[test]
    fn invalid_retention_is_rejected() {
        let mut settings = AppSettings::default();
        settings.logging.retention_days = 0;
        assert_eq!(
            settings.validate(),
            Err(ConfigError::InvalidLogRetention {
                value: 0,
                min: MIN_LOG_RETENTION_DAYS,
                max: MAX_LOG_RETENTION_DAYS,
            })
        );
    }

    #[test]
    fn invalid_ui_scale_is_rejected() {
        for scale in [f32::NAN, f32::INFINITY, 0.5, 3.0] {
            let mut settings = AppSettings::default();
            settings.ui.scale = scale;
            assert!(
                matches!(settings.validate(), Err(ConfigError::InvalidUiScale { .. })),
                "scale {scale} should be rejected"
            );
        }
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let settings = AppSettings {
            schema_version: SETTINGS_SCHEMA_VERSION + 1,
            ..AppSettings::default()
        };
        assert!(matches!(
            settings.check(),
            Err(crate::error::Error::UnsupportedSchemaVersion { .. })
        ));
    }

    #[test]
    fn log_level_parses_case_insensitively() {
        assert_eq!("WARN".parse::<LogLevel>(), Ok(LogLevel::Warn));
        assert_eq!("trace".parse::<LogLevel>(), Ok(LogLevel::Trace));
        let error = "chatty".parse::<LogLevel>().expect_err("unknown level");
        assert!(error.to_string().contains("chatty"));
        assert!(error.to_string().contains("error"));
    }

    #[test]
    fn every_log_level_round_trips_through_display_parsing_and_json() {
        for level in LogLevel::ALL {
            assert_eq!(level.as_str(), level.to_string());
            assert_eq!(level.as_str().parse::<LogLevel>(), Ok(level));

            let encoded = serde_json::to_string(&level).expect("serialize level");
            let decoded: LogLevel = serde_json::from_str(&encoded).expect("parse level");
            assert_eq!(decoded, level);
        }
    }

    #[test]
    fn check_accepts_valid_settings_and_surfaces_invalid_ones() {
        AppSettings::default()
            .check()
            .expect("default settings are valid");

        let invalid = AppSettings {
            logging: LoggingSettings {
                retention_days: 999,
                ..LoggingSettings::default()
            },
            ..AppSettings::default()
        };
        assert!(matches!(
            invalid.check(),
            Err(crate::error::Error::Config(
                ConfigError::InvalidLogRetention { .. }
            ))
        ));
    }

    #[test]
    fn log_level_serializes_as_lowercase() {
        let encoded = serde_json::to_string(&LogLevel::Warn).expect("level serializes");
        assert_eq!(encoded, "\"warn\"");
        let decoded: LogLevel = serde_json::from_str("\"debug\"").expect("level parses");
        assert_eq!(decoded, LogLevel::Debug);
    }
}
