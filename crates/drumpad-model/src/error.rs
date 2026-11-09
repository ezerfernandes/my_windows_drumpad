//! Typed errors shared by the Drumpad crates.
//!
//! Fallible boundaries in the app convert into [`Error`] so callers can log one
//! shape while the user still sees a specific, actionable message. The error
//! model deliberately avoids OS or UI dependencies; it belongs to
//! `drumpad-model`, which every other crate may use.

use std::io;
use std::path::PathBuf;

/// Result alias used across Drumpad crates.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can surface while starting or running the app.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A settings value or settings document is invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// The settings file was written by an unsupported schema version.
    #[error(
        "settings schema version {found} is not supported; this build supports version {supported}"
    )]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    /// Reading or writing a file failed.
    #[error("{context}")]
    Io {
        /// What the app was trying to do when the I/O operation failed.
        context: String,
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },

    /// Encoding or decoding JSON failed.
    #[error("{context}")]
    Json {
        /// What the app was trying to do when JSON handling failed.
        context: String,
        /// The underlying serialization error.
        #[source]
        source: serde_json::Error,
    },

    /// The user's home directory could not be resolved.
    #[error("could not resolve the user home directory for app data")]
    MissingHomeDirectory,

    /// The portable marker exists but is not a regular file.
    #[error("portable marker {path} is not a file")]
    InvalidPortableMarker {
        /// Marker path that exists but is not a file.
        path: PathBuf,
    },

    /// A path that needs a parent directory has none.
    #[error("path {path} has no parent directory")]
    MissingParent {
        /// The path that has no parent.
        path: PathBuf,
    },
}

impl Error {
    /// Build an [`Error::Io`] with a stable, human-readable context.
    ///
    /// The context should describe the action, not the user's data, so it is
    /// safe to place in local logs.
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Build an [`Error::Json`] with a stable, human-readable context.
    pub fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Json {
            context: context.into(),
            source,
        }
    }
}

/// Validation failures for the typed settings document.
///
/// These are returned by [`crate::config`] validation and by the settings
/// store before anything is written to disk.
#[derive(Debug, thiserror::Error, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// Log retention is outside the supported range.
    #[error("logging.retention_days {value} is invalid; use {min}..={max}")]
    InvalidLogRetention {
        /// Rejected value.
        value: u32,
        /// Smallest accepted value.
        min: u32,
        /// Largest accepted value.
        max: u32,
    },

    /// The UI scale is outside the supported range or not finite.
    #[error("ui.scale {value} is invalid; use {min}..={max}")]
    InvalidUiScale {
        /// Rejected value.
        value: f32,
        /// Smallest accepted value.
        min: f32,
        /// Largest accepted value.
        max: f32,
    },

    /// The log level name is not recognized.
    #[error("logging.level \"{value}\" is invalid; use one of {allowed:?}")]
    InvalidLogLevel {
        /// Rejected level name.
        value: String,
        /// Accepted level names.
        allowed: Vec<&'static str>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn config_errors_render_actionable_messages() {
        let error = ConfigError::InvalidLogRetention {
            value: 0,
            min: 1,
            max: 365,
        };
        assert_eq!(
            error.to_string(),
            "logging.retention_days 0 is invalid; use 1..=365"
        );
    }

    #[test]
    fn io_errors_keep_context_and_source() {
        let source = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let error = Error::io("could not write settings", source);
        assert_eq!(error.to_string(), "could not write settings");
        assert!(error.source().is_some());
    }

    #[test]
    fn schema_version_error_names_both_versions() {
        let error = Error::UnsupportedSchemaVersion {
            found: 9,
            supported: 1,
        };
        assert_eq!(
            error.to_string(),
            "settings schema version 9 is not supported; this build supports version 1"
        );
    }

    #[test]
    fn json_errors_keep_context_and_source() {
        let source = serde_json::from_str::<serde_json::Value>("{").expect_err("invalid json");
        let error = Error::json("could not parse settings", source);
        assert_eq!(error.to_string(), "could not parse settings");
        assert!(error.source().is_some());
    }

    #[test]
    fn path_and_home_errors_render_actionable_messages() {
        use std::path::PathBuf;

        let marker = PathBuf::from(r"C:\Users\example\drumpad.portable");
        assert!(
            Error::InvalidPortableMarker {
                path: marker.clone()
            }
            .to_string()
            .contains("drumpad.portable")
        );
        assert!(
            Error::MissingParent { path: marker }
                .to_string()
                .contains("has no parent directory")
        );
        assert_eq!(
            Error::MissingHomeDirectory.to_string(),
            "could not resolve the user home directory for app data"
        );
    }

    #[test]
    fn every_config_error_renders_its_guidance() {
        let scale = ConfigError::InvalidUiScale {
            value: 9.0,
            min: 0.75,
            max: 2.0,
        };
        assert!(scale.to_string().contains("ui.scale 9"));

        let level = ConfigError::InvalidLogLevel {
            value: "chatty".to_owned(),
            allowed: vec!["error", "warn", "info", "debug", "trace"],
        };
        assert!(level.to_string().contains("chatty"));
        assert!(level.to_string().contains("info"));
    }
}
