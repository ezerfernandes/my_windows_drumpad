//! MIDI note-to-pad mapping and the Alesis Nitro Max template.
//!
//! A mapping is channel agnostic: channel filtering happens once at the
//! parser boundary. Notes are unique, but several notes may target the same
//! pad (a drum's head and rim, or the five hi-hat articulations), which is
//! exactly what the Nitro Max template does.

use serde::{Deserialize, Serialize};

use crate::midi::hihat::HiHatArticulation;
use crate::trigger::{PadId, PadIdExt};

/// One note-to-pad assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteMapping {
    /// MIDI note number `0..=127`.
    pub note: u8,
    /// Pad the note triggers.
    pub pad: PadId,
    /// Human-readable zone name, e.g. `"Snare rim"`.
    #[serde(default)]
    pub label: String,
    /// Hi-hat articulation, when the note is part of the hi-hat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub articulation: Option<HiHatArticulation>,
}

impl NoteMapping {
    /// Build a plain note-to-pad mapping.
    #[must_use]
    pub fn new(note: u8, pad: PadId, label: impl Into<String>) -> Self {
        Self {
            note,
            pad,
            label: label.into(),
            articulation: None,
        }
    }

    /// Build a hi-hat articulation mapping.
    #[must_use]
    pub fn hihat(
        note: u8,
        pad: PadId,
        label: impl Into<String>,
        articulation: HiHatArticulation,
    ) -> Self {
        Self {
            note,
            pad,
            label: label.into(),
            articulation: Some(articulation),
        }
    }
}

/// How to resolve a note that is already mapped to another pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssignPolicy {
    /// Refuse the assignment and report the conflict.
    #[default]
    Reject,
    /// Move the note to the new pad.
    Replace,
}

/// An attempted note assignment collided with an existing one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("note {note} is already assigned to pad {}", existing_pad.get() + 1)]
pub struct NoteConflict {
    /// Conflicting note.
    pub note: u8,
    /// Pad that already owns the note.
    pub existing_pad: PadId,
    /// Pad the caller wanted.
    pub requested_pad: PadId,
}

/// Why a note map could not be saved or loaded.
#[derive(Debug, thiserror::Error)]
pub enum MidiMapError {
    /// Two entries use the same note.
    #[error("note map has {count} duplicate note(s)")]
    DuplicateNotes {
        /// Number of duplicated notes.
        count: usize,
        /// Duplicated note numbers.
        notes: Vec<u8>,
    },

    /// JSON encoding or decoding failed.
    #[error("note map JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),

    /// The document came from a newer build.
    #[error("note map schema version {found} is newer than supported version {supported}")]
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u32,
        /// Highest supported version.
        supported: u32,
    },
}

/// Current note map schema version.
pub const NOTE_MAP_SCHEMA_VERSION: u32 = 1;

/// Ordered note-to-pad map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiNoteMap {
    /// Schema version.
    #[serde(default = "note_map_schema_version")]
    pub schema_version: u32,
    /// Mappings, expected unique by note.
    pub mappings: Vec<NoteMapping>,
}

const fn note_map_schema_version() -> u32 {
    NOTE_MAP_SCHEMA_VERSION
}

impl MidiNoteMap {
    /// Empty map.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: NOTE_MAP_SCHEMA_VERSION,
            mappings: Vec::new(),
        }
    }

    /// Map from a list of assignments, sorting them by note.
    #[must_use]
    pub fn from_mappings(mappings: Vec<NoteMapping>) -> Self {
        let mut map = Self {
            schema_version: NOTE_MAP_SCHEMA_VERSION,
            mappings,
        };
        map.mappings.sort_unstable_by_key(|mapping| mapping.note);
        map
    }

    /// The Alesis Nitro Max template.
    ///
    /// Every documented pad and zone is covered. Head and rim share a pad so a
    /// rim hit chokes/articulates with its drum; the five hi-hat notes share
    /// the hi-hat pad and carry their articulation. Pads 11-16 stay free for
    /// user samples.
    #[must_use]
    pub fn nitro_max_template() -> Self {
        let pad = PadId::from_index;
        Self::from_mappings(vec![
            NoteMapping::new(36, pad(0), "Kick"),
            NoteMapping::new(38, pad(1), "Snare"),
            NoteMapping::new(40, pad(1), "Snare rim"),
            NoteMapping::new(48, pad(2), "Tom 1"),
            NoteMapping::new(50, pad(2), "Tom 1 rim"),
            NoteMapping::new(45, pad(3), "Tom 2"),
            NoteMapping::new(47, pad(3), "Tom 2 rim"),
            NoteMapping::new(43, pad(4), "Tom 3"),
            NoteMapping::new(58, pad(4), "Tom 3 rim"),
            NoteMapping::new(41, pad(5), "Tom 4"),
            NoteMapping::new(39, pad(5), "Tom 4 rim"),
            NoteMapping::new(51, pad(6), "Ride"),
            NoteMapping::new(49, pad(7), "Crash 1"),
            NoteMapping::new(57, pad(8), "Crash 2"),
            NoteMapping::hihat(46, pad(9), "Hi-hat open", HiHatArticulation::Open),
            NoteMapping::hihat(23, pad(9), "Hi-hat half-open", HiHatArticulation::HalfOpen),
            NoteMapping::hihat(42, pad(9), "Hi-hat closed", HiHatArticulation::Closed),
            NoteMapping::hihat(44, pad(9), "Hi-hat pedal", HiHatArticulation::Pedal),
            NoteMapping::hihat(21, pad(9), "Hi-hat splash", HiHatArticulation::Splash),
        ])
    }

    /// Mapping for `note`, if present.
    #[must_use]
    pub fn mapping(&self, note: u8) -> Option<&NoteMapping> {
        self.mappings.iter().find(|mapping| mapping.note == note)
    }

    /// Pad triggered by `note`.
    #[must_use]
    pub fn pad_for(&self, note: u8) -> Option<PadId> {
        self.mapping(note).map(|mapping| mapping.pad)
    }

    /// All mappings, sorted by note.
    #[must_use]
    pub fn mappings(&self) -> &[NoteMapping] {
        &self.mappings
    }

    /// Notes mapped to `pad`, in note order.
    #[must_use]
    pub fn notes_for_pad(&self, pad: PadId) -> Vec<u8> {
        self.mappings
            .iter()
            .filter(|mapping| mapping.pad == pad)
            .map(|mapping| mapping.note)
            .collect()
    }

    /// Duplicate note numbers, in sorted order.
    #[must_use]
    pub fn duplicates(&self) -> Vec<u8> {
        let mut notes: Vec<u8> = self.mappings.iter().map(|mapping| mapping.note).collect();
        notes.sort_unstable();
        let mut duplicates = Vec::new();
        for window in notes.windows(2) {
            if window[0] == window[1] {
                duplicates.push(window[0]);
            }
        }
        duplicates.sort_unstable();
        duplicates.dedup();
        duplicates
    }

    /// Validate that every note is unique.
    pub fn validate(&self) -> Result<(), MidiMapError> {
        let duplicates = self.duplicates();
        if duplicates.is_empty() {
            Ok(())
        } else {
            Err(MidiMapError::DuplicateNotes {
                count: duplicates.len(),
                notes: duplicates,
            })
        }
    }

    /// Assign `note` to `pad`, honoring `policy` when the note already exists.
    pub fn assign(
        &mut self,
        note: u8,
        pad: PadId,
        label: impl Into<String>,
        policy: AssignPolicy,
    ) -> Result<Option<NoteMapping>, NoteConflict> {
        let label = label.into();
        if let Some(index) = self
            .mappings
            .iter()
            .position(|mapping| mapping.note == note)
        {
            let existing = self.mappings[index].clone();
            if existing.pad != pad && policy == AssignPolicy::Reject {
                return Err(NoteConflict {
                    note,
                    existing_pad: existing.pad,
                    requested_pad: pad,
                });
            }
            let replacement = NoteMapping {
                note,
                pad,
                label,
                articulation: existing.articulation,
            };
            self.mappings[index] = replacement;
            return Ok(Some(existing));
        }
        let insert_at = self.mappings.len();
        self.mappings.insert(
            insert_at,
            NoteMapping {
                note,
                pad,
                label,
                articulation: None,
            },
        );
        self.mappings.sort_unstable_by_key(|mapping| mapping.note);
        Ok(None)
    }

    /// Remove the mapping for `note`.
    pub fn remove(&mut self, note: u8) -> Option<NoteMapping> {
        let index = self
            .mappings
            .iter()
            .position(|mapping| mapping.note == note)?;
        Some(self.mappings.remove(index))
    }

    /// Pretty JSON for saving with a kit.
    pub fn to_json(&self) -> Result<String, MidiMapError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse a saved note map.
    pub fn from_json(json: &str) -> Result<Self, MidiMapError> {
        let mut map: Self = serde_json::from_str(json)?;
        if map.schema_version > NOTE_MAP_SCHEMA_VERSION {
            return Err(MidiMapError::UnsupportedSchemaVersion {
                found: map.schema_version,
                supported: NOTE_MAP_SCHEMA_VERSION,
            });
        }
        map.mappings.sort_unstable_by_key(|mapping| mapping.note);
        Ok(map)
    }
}

impl Default for MidiNoteMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nitro_template_covers_every_documented_zone() {
        let map = MidiNoteMap::nitro_max_template();
        let expected = [
            36, 38, 40, 48, 50, 45, 47, 43, 58, 51, 49, 57, 41, 39, 46, 23, 42, 44, 21,
        ];
        assert_eq!(map.mappings().len(), expected.len());
        for note in expected {
            assert!(map.mapping(note).is_some(), "note {note} missing");
        }
        assert!(map.validate().is_ok());
        assert!(map.duplicates().is_empty());
    }

    #[test]
    fn nitro_template_assigns_pads_and_articulations() {
        let map = MidiNoteMap::nitro_max_template();
        assert_eq!(map.pad_for(36), Some(PadId::from_index(0)));
        assert_eq!(map.pad_for(38), Some(PadId::from_index(1)));
        assert_eq!(map.pad_for(40), Some(PadId::from_index(1)));
        assert_eq!(map.pad_for(58), Some(PadId::from_index(4)));
        for (note, articulation) in [
            (46, HiHatArticulation::Open),
            (23, HiHatArticulation::HalfOpen),
            (42, HiHatArticulation::Closed),
            (44, HiHatArticulation::Pedal),
            (21, HiHatArticulation::Splash),
        ] {
            let mapping = map.mapping(note).unwrap();
            assert_eq!(mapping.pad, PadId::from_index(9));
            assert_eq!(mapping.articulation, Some(articulation));
        }
        assert_eq!(
            map.notes_for_pad(PadId::from_index(9)),
            vec![21, 23, 42, 44, 46]
        );
    }

    #[test]
    fn assignment_rejects_conflicts_by_default() {
        let mut map = MidiNoteMap::nitro_max_template();
        let error = map
            .assign(36, PadId::from_index(10), "Kick", AssignPolicy::Reject)
            .unwrap_err();
        assert_eq!(error.note, 36);
        assert_eq!(error.existing_pad, PadId::from_index(0));
        assert_eq!(map.pad_for(36), Some(PadId::from_index(0)), "map unchanged");
    }

    #[test]
    fn replace_policy_moves_the_note() {
        let mut map = MidiNoteMap::nitro_max_template();
        let previous = map
            .assign(36, PadId::from_index(10), "Kick", AssignPolicy::Replace)
            .unwrap()
            .unwrap();
        assert_eq!(previous.pad, PadId::from_index(0));
        assert_eq!(map.pad_for(36), Some(PadId::from_index(10)));
        assert!(map.validate().is_ok());
    }

    #[test]
    fn new_notes_are_inserted_in_order() {
        let mut map = MidiNoteMap::new();
        map.assign(50, PadId::from_index(1), "B", AssignPolicy::Reject)
            .unwrap();
        map.assign(40, PadId::from_index(0), "A", AssignPolicy::Reject)
            .unwrap();
        map.assign(60, PadId::from_index(2), "C", AssignPolicy::Reject)
            .unwrap();
        let notes: Vec<u8> = map.mappings().iter().map(|mapping| mapping.note).collect();
        assert_eq!(notes, vec![40, 50, 60]);
    }

    #[test]
    fn removal_and_duplicate_detection_work() {
        let mut map = MidiNoteMap::nitro_max_template();
        assert_eq!(map.remove(36).unwrap().note, 36);
        assert_eq!(map.remove(36), None);

        let broken = MidiNoteMap {
            schema_version: NOTE_MAP_SCHEMA_VERSION,
            mappings: vec![
                NoteMapping::new(36, PadId::from_index(0), "Kick"),
                NoteMapping::new(36, PadId::from_index(1), "Other"),
            ],
        };
        let error = broken.validate().unwrap_err();
        assert!(matches!(error, MidiMapError::DuplicateNotes { .. }));
    }

    #[test]
    fn note_maps_round_trip_through_json() {
        let map = MidiNoteMap::nitro_max_template();
        let json = map.to_json().unwrap();
        let restored = MidiNoteMap::from_json(&json).unwrap();
        assert_eq!(restored, map);
    }

    #[test]
    fn future_schema_versions_are_refused() {
        let json = r#"{"schema_version":99,"mappings":[]}"#;
        assert!(matches!(
            MidiNoteMap::from_json(json),
            Err(MidiMapError::UnsupportedSchemaVersion { .. })
        ));
    }
}
