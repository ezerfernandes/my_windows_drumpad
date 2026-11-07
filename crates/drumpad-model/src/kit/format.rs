//! Versioned JSON kit document format.
//!
//! The v1 document is a plain JSON object. Unknown fields are ignored so a
//! newer minor build can add optional data without breaking older readers; a
//! higher `format_version` is refused rather than guessed. Older versions are
//! upgraded through [`super::migration`].

use serde_json::Value;
use thiserror::Error;

use super::migration::{self, MigrationError, MigrationReport};
use super::model::{CURRENT_FORMAT_VERSION, Kit};

/// JSON Schema for kit format v1, compiled into the crate for tooling and tests.
pub const KIT_V1_SCHEMA: &str = include_str!("../../schema/kit-v1.schema.json");

/// Errors from parsing or serializing a kit document.
#[derive(Debug, Error)]
pub enum FormatError {
    /// JSON syntax or shape was invalid.
    #[error("invalid kit JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The document is from a newer schema.
    #[error("kit format version {found} is newer than supported version {current}")]
    FutureVersion {
        /// Version found in the document.
        found: u32,
        /// Newest version this build understands.
        current: u32,
    },
    /// The document has no version marker.
    #[error("kit document is missing its format version")]
    MissingVersion,
    /// An older version could not be migrated.
    #[error("kit migration failed: {0}")]
    Migration(#[from] MigrationError),
}

/// A parsed kit plus the migration that produced it, if any.
#[derive(Debug)]
pub struct ParsedKit {
    /// Current-version kit.
    pub kit: Kit,
    /// Migration details when the source document was older.
    pub migration: Option<MigrationReport>,
}

/// Serialize a kit as compact JSON.
pub fn to_json(kit: &Kit) -> Result<String, FormatError> {
    Ok(serde_json::to_string(kit)?)
}

/// Serialize a kit as human-readable JSON.
pub fn to_json_pretty(kit: &Kit) -> Result<String, FormatError> {
    Ok(serde_json::to_string_pretty(kit)?)
}

/// Serialize a kit to a JSON value.
pub fn to_value(kit: &Kit) -> Result<Value, FormatError> {
    Ok(serde_json::to_value(kit)?)
}

/// Parse a kit document, migrating older versions and refusing future ones.
pub fn parse_kit_json(text: &str) -> Result<ParsedKit, FormatError> {
    let value: Value = serde_json::from_str(text)?;
    let found = migration::read_version(&value).ok_or(FormatError::MissingVersion)?;
    if found > CURRENT_FORMAT_VERSION {
        return Err(FormatError::FutureVersion {
            found,
            current: CURRENT_FORMAT_VERSION,
        });
    }
    if found == CURRENT_FORMAT_VERSION {
        let kit: Kit = serde_json::from_value(value)?;
        Ok(ParsedKit {
            kit,
            migration: None,
        })
    } else {
        let (kit, report) = migration::migrate_value(value)?;
        Ok(ParsedKit {
            kit,
            migration: Some(report),
        })
    }
}

/// Parse a kit document from raw bytes.
pub fn parse_kit_bytes(bytes: &[u8]) -> Result<ParsedKit, FormatError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        FormatError::Json(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )))
    })?;
    parse_kit_json(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{ContentHash, Layer, RelativeSamplePath, SampleRef};

    #[test]
    fn round_trip_is_lossless() {
        let mut kit = Kit::silent_default();
        kit.metadata.author = Some("Drumpad".into());
        kit.pads[0].layers.push(Layer::with_sample(
            Default::default(),
            SampleRef::new(
                RelativeSamplePath::new("samples/kick.wav").unwrap(),
                ContentHash::of_bytes(b"kick"),
            ),
        ));
        let json = to_json_pretty(&kit).unwrap();
        let parsed = parse_kit_json(&json).unwrap();
        assert!(parsed.migration.is_none());
        assert_eq!(parsed.kit, kit);
    }

    #[test]
    fn unknown_fields_are_ignored_deliberately() {
        let kit = Kit::silent_default();
        let mut value = to_value(&kit).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("future_field".into(), serde_json::json!({"x": 1}));
        let parsed = parse_kit_json(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(parsed.kit.id, kit.id);
    }

    #[test]
    fn future_versions_are_rejected() {
        let mut value = to_value(&Kit::silent_default()).unwrap();
        value["format_version"] = serde_json::json!(99);
        let error = parse_kit_json(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert!(matches!(
            error,
            FormatError::FutureVersion {
                found: 99,
                current: 1
            }
        ));
    }

    #[test]
    fn missing_version_is_rejected() {
        let mut value = to_value(&Kit::silent_default()).unwrap();
        value.as_object_mut().unwrap().remove("format_version");
        let error = parse_kit_json(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert!(matches!(error, FormatError::MissingVersion));
    }
}
