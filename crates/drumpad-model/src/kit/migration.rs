//! Forward-only kit schema migrations.
//!
//! A migration reads a document of an older version and produces a
//! current-version [`Kit`]. Documents from the future are always refused:
//! silently discarding fields written by a newer build is worse than asking
//! the person to update Drumpad.

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use super::model::{
    CURRENT_FORMAT_VERSION, KeyMapping, Kit, LEGACY_FORMAT_VERSION, Layer, LayerId, MidiChannel,
    MidiMapping, MidiNote, PhysicalKey, RelativeSamplePath, SampleRef, VelocityRange,
};

/// Summary of the migration applied to one document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MigrationReport {
    /// Source schema version.
    pub from_version: u32,
    /// Resulting schema version.
    pub to_version: u32,
    /// Human-readable list of changes, suitable for logs or a dialog.
    pub changes: Vec<String>,
}

/// Migration failures.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// The document is newer than this build.
    #[error("kit format version {found} is newer than supported version {current}")]
    FutureVersion {
        /// Version found in the document.
        found: u32,
        /// Newest supported version.
        current: u32,
    },
    /// No migration path exists from that version.
    #[error("no migration path from kit format version {found} to {current}")]
    UnsupportedVersion {
        /// Version found in the document.
        found: u32,
        /// Newest supported version.
        current: u32,
    },
    /// The document has no version marker.
    #[error("kit document is missing its format version")]
    MissingVersion,
    /// The legacy document could not be interpreted safely.
    #[error("legacy kit v{version} is invalid: {message}")]
    InvalidLegacy {
        /// Legacy version being migrated.
        version: u32,
        /// Explanation.
        message: String,
    },
}

/// Read the declared version from a raw JSON document.
///
/// v1 uses `format_version`; the pre-release v0 draft used `version`.
pub fn read_version(value: &Value) -> Option<u32> {
    value
        .get("format_version")
        .or_else(|| value.get("version"))
        .and_then(Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
}

/// Migrate a raw document to the current version.
pub fn migrate_value(value: Value) -> Result<(Kit, MigrationReport), MigrationError> {
    let found = read_version(&value).ok_or(MigrationError::MissingVersion)?;
    if found > CURRENT_FORMAT_VERSION {
        return Err(MigrationError::FutureVersion {
            found,
            current: CURRENT_FORMAT_VERSION,
        });
    }

    let mut current = value;
    let mut version = found;
    let mut changes = Vec::new();
    while version < CURRENT_FORMAT_VERSION {
        match version {
            LEGACY_FORMAT_VERSION => {
                let (migrated, step_changes) = migrate_v0_to_v1(current)?;
                current = serde_json::to_value(migrated).map_err(|error| {
                    MigrationError::InvalidLegacy {
                        version: 0,
                        message: error.to_string(),
                    }
                })?;
                changes.extend(step_changes);
                version = 1;
            }
            other => {
                return Err(MigrationError::UnsupportedVersion {
                    found: other,
                    current: CURRENT_FORMAT_VERSION,
                });
            }
        }
    }

    if version != CURRENT_FORMAT_VERSION {
        return Err(MigrationError::UnsupportedVersion {
            found: version,
            current: CURRENT_FORMAT_VERSION,
        });
    }

    let kit: Kit =
        serde_json::from_value(current.clone()).map_err(|error| MigrationError::InvalidLegacy {
            version: found,
            message: error.to_string(),
        })?;

    Ok((
        kit,
        MigrationReport {
            from_version: found,
            to_version: CURRENT_FORMAT_VERSION,
            changes,
        },
    ))
}

#[derive(Debug, Deserialize)]
struct LegacyKitV0 {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pads: Vec<LegacyPadV0>,
}

#[derive(Debug, Deserialize)]
struct LegacyPadV0 {
    slot: u8,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sample_path: Option<String>,
    #[serde(default)]
    sample_original_name: Option<String>,
    #[serde(default)]
    key_code: Option<u32>,
    #[serde(default)]
    midi_note: Option<u8>,
    #[serde(default)]
    midi_channel: Option<u8>,
    #[serde(default)]
    gain_db: Option<f32>,
    #[serde(default)]
    pan: Option<f32>,
}

fn migrate_v0_to_v1(value: Value) -> Result<(Kit, Vec<String>), MigrationError> {
    let legacy: LegacyKitV0 =
        serde_json::from_value(value).map_err(|error| MigrationError::InvalidLegacy {
            version: 0,
            message: error.to_string(),
        })?;

    let name = legacy
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Migrated Kit".to_owned());
    let mut kit = Kit::new(name);
    kit.metadata.author = legacy.author;
    kit.metadata.description = legacy.description;

    let mut changes = vec![
        "renamed `version` to `format_version`".to_owned(),
        "upgraded document to v1".to_owned(),
    ];
    let mut assigned = 0usize;

    for legacy_pad in legacy.pads {
        let slot = usize::from(legacy_pad.slot);
        if slot >= kit.pads.len() {
            changes.push(format!(
                "dropped legacy pad slot {} (outside 0..16)",
                legacy_pad.slot
            ));
            continue;
        }
        let pad = &mut kit.pads[slot];
        if let Some(name) = legacy_pad.name {
            pad.name = Some(name);
        }
        if let Some(raw_path) = legacy_pad.sample_path {
            let path = RelativeSamplePath::new(raw_path.clone()).map_err(|error| {
                MigrationError::InvalidLegacy {
                    version: 0,
                    message: format!("pad slot {} sample path: {error}", legacy_pad.slot),
                }
            })?;
            let layer = Layer {
                id: LayerId::FIRST,
                sample: Some(SampleRef {
                    path,
                    hash: None,
                    original_name: legacy_pad.sample_original_name,
                }),
                velocity: VelocityRange::FULL,
                gain_db: 0.0,
                tune_cents: 0.0,
                loop_points: None,
            };
            pad.layers.push(layer);
            assigned += 1;
        }
        if let Some(key_code) = legacy_pad.key_code {
            pad.key = Some(KeyMapping::new(PhysicalKey::from_raw(key_code)));
        }
        if let Some(note) = legacy_pad.midi_note {
            let note = MidiNote::new(note).map_err(|error| MigrationError::InvalidLegacy {
                version: 0,
                message: format!("pad slot {} MIDI note: {error}", legacy_pad.slot),
            })?;
            let channel = legacy_pad
                .midi_channel
                .map(MidiChannel::new)
                .transpose()
                .map_err(|error| MigrationError::InvalidLegacy {
                    version: 0,
                    message: format!("pad slot {} MIDI channel: {error}", legacy_pad.slot),
                })?
                .unwrap_or_default();
            pad.midi = Some(MidiMapping::new(note, channel));
        }
        if let Some(gain) = legacy_pad.gain_db
            && gain.is_finite()
        {
            pad.gain_db = gain;
        }
        if let Some(pan) = legacy_pad.pan
            && pan.is_finite()
        {
            pad.pan = pan;
        }
    }

    if assigned > 0 {
        changes.push(format!("converted {assigned} legacy sample assignment(s)"));
    }
    changes.push("added content hashes as `null`; verify samples to fill them".to_owned());
    Ok((kit, changes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_document() -> Value {
        serde_json::json!({
            "version": 0,
            "name": "Legacy Kit",
            "author": "Old Timer",
            "pads": [
                {
                    "slot": 0,
                    "name": "Kick",
                    "sample_path": "samples/kick.wav",
                    "sample_original_name": "Kick.wav",
                    "key_code": 2,
                    "midi_note": 36
                },
                {
                    "slot": 3,
                    "sample_path": "samples/snare.wav",
                    "midi_note": 38,
                    "midi_channel": 2
                }
            ]
        })
    }

    #[test]
    fn v0_migrates_to_current() {
        let (kit, report) = migrate_value(legacy_document()).unwrap();
        assert_eq!(kit.format_version, CURRENT_FORMAT_VERSION);
        assert_eq!(report.from_version, 0);
        assert_eq!(report.to_version, CURRENT_FORMAT_VERSION);
        assert_eq!(kit.metadata.name, "Legacy Kit");
        assert_eq!(kit.metadata.author.as_deref(), Some("Old Timer"));
        assert_eq!(kit.pads[0].name.as_deref(), Some("Kick"));
        assert_eq!(
            kit.pads[0].layers[0].sample.as_ref().unwrap().path.as_str(),
            "samples/kick.wav"
        );
        assert_eq!(kit.pads[0].midi.unwrap().note.get(), 36);
        assert_eq!(kit.pads[3].midi.unwrap().channel.get(), 2);
    }

    #[test]
    fn future_versions_are_rejected() {
        let error = migrate_value(serde_json::json!({"format_version": 2})).unwrap_err();
        assert!(matches!(
            error,
            MigrationError::FutureVersion {
                found: 2,
                current: 1
            }
        ));
    }

    #[test]
    fn missing_version_is_rejected() {
        let error = migrate_value(serde_json::json!({"name": "Nope"})).unwrap_err();
        assert!(matches!(error, MigrationError::MissingVersion));
    }

    #[test]
    fn unsafe_legacy_paths_are_rejected() {
        let mut value = legacy_document();
        value["pads"][0]["sample_path"] = serde_json::json!("../../escape.wav");
        let error = migrate_value(value).unwrap_err();
        assert!(matches!(error, MigrationError::InvalidLegacy { .. }));
    }
}
