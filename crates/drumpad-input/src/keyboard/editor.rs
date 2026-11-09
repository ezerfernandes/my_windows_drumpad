//! Mapping editor state and the profile library.
//!
//! The editor model is UI agnostic: the egui layer arms a pad, calls
//! [`MappingEditor::capture`] with the next physical key, and renders whatever
//! [`CaptureOutcome`] comes back. Conflicts are never silent: the default
//! policy rejects them and returns the list so the UI can explain the clash;
//! the user can switch to [`ConflictPolicy::Replace`] to confirm explicitly.
//!
//! Profiles live in a [`ProfileLibrary`] keyed by identifier, so a full-keyboard
//! profile and a Finding 35-key profile can coexist and travel with a kit as a
//! single JSON document. Loading a profile builds a new [`crate::keyboard::KeyMap`]
//! which the app swaps in without restarting.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::keyboard::keys::PhysicalKey;
use crate::keyboard::profile::{
    KeyAction, KeyBinding, KeyboardProfile, MappingConflict, PROFILE_SCHEMA_VERSION, ProfileDevice,
    ProfileError,
};
use crate::trigger::PadId;

/// How the editor treats a key or pad that is already assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Refuse the capture and report the conflict.
    #[default]
    Reject,
    /// Replace the old assignment after the user confirmed.
    Replace,
}

/// Result of capturing one key for the armed pad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// The binding was applied.
    Applied {
        /// Pad that received the key.
        pad: PadId,
        /// Captured key.
        key: PhysicalKey,
        /// Bindings displaced by the change, for undo.
        displaced: Vec<KeyBinding>,
    },
    /// The capture was refused; the profile is unchanged.
    Rejected {
        /// Why the key or pad cannot be assigned.
        conflicts: Vec<MappingConflict>,
    },
    /// No pad is armed.
    NotArmed,
}

impl CaptureOutcome {
    /// True when the profile changed.
    #[must_use]
    pub fn is_applied(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

/// One-shot capture flow for the mapping editor.
#[derive(Debug, Clone, Default)]
pub struct MappingEditor {
    armed: Option<PadId>,
    policy: ConflictPolicy,
}

impl MappingEditor {
    /// Create an editor with no armed pad and the reject-by-default policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm `pad`: the next captured key is assigned to it.
    pub fn arm(&mut self, pad: PadId) {
        self.armed = Some(pad);
    }

    /// Cancel capture.
    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// Pad waiting for a key, if any.
    #[must_use]
    pub fn armed(&self) -> Option<PadId> {
        self.armed
    }

    /// Conflict policy.
    #[must_use]
    pub fn policy(&self) -> ConflictPolicy {
        self.policy
    }

    /// Change the conflict policy. The UI sets [`ConflictPolicy::Replace`] only
    /// after the user confirms a specific conflict.
    pub fn set_policy(&mut self, policy: ConflictPolicy) {
        self.policy = policy;
    }

    /// Capture `key` for the armed pad.
    ///
    /// On success the editor disarms; on rejection it stays armed so the user
    /// can press a different key.
    pub fn capture(&mut self, profile: &mut KeyboardProfile, key: PhysicalKey) -> CaptureOutcome {
        let Some(pad) = self.armed else {
            return CaptureOutcome::NotArmed;
        };
        let target = KeyAction::pad(pad);

        let mut conflicts = Vec::new();
        if let Some(existing) = profile.action_for(key) {
            if existing != target {
                conflicts.push(MappingConflict::Key {
                    key,
                    actions: vec![existing, target],
                });
            }
        }

        if !conflicts.is_empty() && self.policy == ConflictPolicy::Reject {
            return CaptureOutcome::Rejected { conflicts };
        }

        let displaced = profile.set_binding(key, target);
        self.armed = None;
        CaptureOutcome::Applied {
            pad,
            key,
            displaced,
        }
    }

    /// Restore `profile` to the built-in map for its device class.
    ///
    /// Returns true when something changed.
    pub fn reset(profile: &mut KeyboardProfile) -> bool {
        let before = profile.clone();
        profile.reset_to_default();
        *profile != before
    }
}

/// In-memory collection of keyboard profiles, saved as one JSON document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLibrary {
    /// Schema version of the library document.
    #[serde(default = "library_schema_version")]
    pub schema_version: u32,
    /// Profiles, unordered; lookups are by identifier.
    pub profiles: Vec<KeyboardProfile>,
}

const fn library_schema_version() -> u32 {
    PROFILE_SCHEMA_VERSION
}

#[derive(Debug, Serialize, Deserialize)]
struct LibraryDocument {
    schema_version: u32,
    profiles: Vec<KeyboardProfile>,
}

impl ProfileLibrary {
    /// Empty library.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            profiles: Vec::new(),
        }
    }

    /// Library containing the full-keyboard and Finding 35-key profiles.
    #[must_use]
    pub fn with_builtins() -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            profiles: KeyboardProfile::builtins().to_vec(),
        }
    }

    /// All profiles.
    #[must_use]
    pub fn all(&self) -> &[KeyboardProfile] {
        &self.profiles
    }

    /// Profile by identifier, case sensitive.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&KeyboardProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// First profile for a device class.
    #[must_use]
    pub fn profile_for_device(&self, device: ProfileDevice) -> Option<&KeyboardProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.device == device)
    }

    /// Insert or replace a profile by identifier, returning the old value.
    pub fn upsert(&mut self, profile: KeyboardProfile) -> Option<KeyboardProfile> {
        match self
            .profiles
            .iter_mut()
            .find(|existing| existing.id == profile.id)
        {
            Some(slot) => Some(core::mem::replace(slot, profile)),
            None => {
                self.profiles.push(profile);
                None
            }
        }
    }

    /// Remove a profile by identifier.
    pub fn remove(&mut self, id: &str) -> Option<KeyboardProfile> {
        let index = self.profiles.iter().position(|profile| profile.id == id)?;
        Some(self.profiles.remove(index))
    }

    /// Every conflict in the library, keyed by profile identifier.
    #[must_use]
    pub fn conflicts(&self) -> Vec<(String, Vec<MappingConflict>)> {
        self.profiles
            .iter()
            .map(|profile| (profile.id.clone(), profile.conflicts()))
            .filter(|(_, conflicts)| !conflicts.is_empty())
            .collect()
    }

    /// Pretty JSON for export.
    pub fn to_json(&self) -> Result<String, ProfileError> {
        let document = LibraryDocument {
            schema_version: self.schema_version,
            profiles: self.profiles.clone(),
        };
        Ok(serde_json::to_string_pretty(&document)?)
    }

    /// Parse an exported library document.
    pub fn from_json(json: &str) -> Result<Self, ProfileError> {
        let document: LibraryDocument = serde_json::from_str(json)?;
        if document.schema_version > PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchemaVersion {
                found: document.schema_version,
                supported: PROFILE_SCHEMA_VERSION,
            });
        }
        Ok(Self {
            schema_version: document.schema_version,
            profiles: document.profiles,
        })
    }

    /// Save the library as pretty JSON.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), ProfileError> {
        let path = path.as_ref();
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(|source| ProfileError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Load a library from disk.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let path = path.as_ref();
        let json = std::fs::read_to_string(path).map_err(|source| ProfileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&json)
    }
}

impl Default for ProfileLibrary {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::profile::{FINDING_35_PROFILE_ID, FULL_KEYBOARD_PROFILE_ID};
    use crate::trigger::PadIdExt;

    fn pad(index: u8) -> PadId {
        PadId::from_index(index)
    }

    #[test]
    fn capture_without_arming_is_refused() {
        let mut editor = MappingEditor::new();
        let mut profile = KeyboardProfile::default_full_keyboard();
        let before = profile.clone();
        assert_eq!(
            editor.capture(&mut profile, PhysicalKey::Other(1)),
            CaptureOutcome::NotArmed
        );
        assert_eq!(profile, before);
    }

    #[test]
    fn capture_applies_an_unused_key() {
        let mut editor = MappingEditor::new();
        let mut profile = KeyboardProfile::default_full_keyboard();
        editor.arm(pad(0));
        let outcome = editor.capture(&mut profile, PhysicalKey::Other(0x70));
        assert!(outcome.is_applied());
        assert_eq!(
            profile.action_for(PhysicalKey::Other(0x70)),
            Some(KeyAction::pad(pad(0)))
        );
        assert_eq!(editor.armed(), None);
    }

    #[test]
    fn conflicts_are_rejected_by_default() {
        let mut editor = MappingEditor::new();
        let mut profile = KeyboardProfile::default_full_keyboard();
        let before = profile.clone();
        editor.arm(pad(0));
        let outcome = editor.capture(&mut profile, PhysicalKey::Digit2);
        let CaptureOutcome::Rejected { conflicts } = outcome else {
            panic!("expected rejection, got {outcome:?}");
        };
        assert!(!conflicts.is_empty());
        assert_eq!(profile, before);
        assert_eq!(editor.armed(), Some(pad(0)), "editor stays armed");
    }

    #[test]
    fn replace_policy_applies_after_confirmation() {
        let mut editor = MappingEditor::new();
        let mut profile = KeyboardProfile::default_full_keyboard();
        editor.set_policy(ConflictPolicy::Replace);
        editor.arm(pad(0));
        let outcome = editor.capture(&mut profile, PhysicalKey::Digit2);
        let CaptureOutcome::Applied { displaced, .. } = outcome else {
            panic!("expected applied, got {outcome:?}");
        };
        assert_eq!(
            displaced,
            vec![
                KeyBinding::new(PhysicalKey::Digit1, KeyAction::pad(pad(0))),
                KeyBinding::new(PhysicalKey::Digit2, KeyAction::pad(pad(1))),
            ]
        );
        assert_eq!(
            profile.action_for(PhysicalKey::Digit2),
            Some(KeyAction::pad(pad(0)))
        );
        assert_eq!(profile.action_for(PhysicalKey::Digit1), None);
        assert!(profile.is_valid());
    }

    #[test]
    fn reset_restores_defaults() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.unbind(PhysicalKey::KeyQ);
        assert!(MappingEditor::reset(&mut profile));
        assert_eq!(profile, KeyboardProfile::default_full_keyboard());
        assert!(!MappingEditor::reset(&mut profile));
    }

    #[test]
    fn library_round_trips_both_profiles() {
        let library = ProfileLibrary::with_builtins();
        assert_eq!(library.all().len(), 2);
        assert!(library.get(FULL_KEYBOARD_PROFILE_ID).is_some());
        assert!(library.get(FINDING_35_PROFILE_ID).is_some());
        assert_eq!(
            library
                .profile_for_device(ProfileDevice::OneHandKeypad)
                .unwrap()
                .id,
            FINDING_35_PROFILE_ID
        );
        assert!(library.conflicts().is_empty());

        let json = library.to_json().unwrap();
        let restored = ProfileLibrary::from_json(&json).unwrap();
        assert_eq!(restored, library);
    }

    #[test]
    fn library_upsert_and_remove_are_by_id() {
        let mut library = ProfileLibrary::new();
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.name = "Mine".to_owned();
        assert!(library.upsert(profile.clone()).is_none());
        assert_eq!(library.all().len(), 1);
        profile.name = "Renamed".to_owned();
        let old = library.upsert(profile.clone()).unwrap();
        assert_eq!(old.name, "Mine");
        assert_eq!(
            library.get(FULL_KEYBOARD_PROFILE_ID).unwrap().name,
            "Renamed"
        );
        assert_eq!(library.remove(FULL_KEYBOARD_PROFILE_ID), Some(profile));
        assert!(library.get(FULL_KEYBOARD_PROFILE_ID).is_none());
    }

    #[test]
    fn library_saves_and_loads_from_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keyboard-profiles.json");
        let library = ProfileLibrary::with_builtins();
        library.save_to_path(&path).unwrap();
        let restored = ProfileLibrary::load_from_path(&path).unwrap();
        assert_eq!(restored, library);
    }

    #[test]
    fn library_refuses_future_schema_versions() {
        let json = r#"{"schema_version":99,"profiles":[]}"#;
        assert!(matches!(
            ProfileLibrary::from_json(json),
            Err(ProfileError::UnsupportedSchemaVersion { .. })
        ));
    }

    #[test]
    fn disarm_and_policy_accessors_reflect_editor_state() {
        let mut editor = MappingEditor::new();
        assert_eq!(editor.armed(), None);
        assert_eq!(editor.policy(), ConflictPolicy::Reject);
        editor.arm(pad(4));
        assert_eq!(editor.armed(), Some(pad(4)));
        editor.set_policy(ConflictPolicy::Replace);
        assert_eq!(editor.policy(), ConflictPolicy::Replace);
        editor.disarm();
        assert_eq!(editor.armed(), None);
    }

    #[test]
    fn default_library_is_the_builtin_set() {
        assert_eq!(ProfileLibrary::default(), ProfileLibrary::with_builtins());
    }

    #[test]
    fn library_json_without_schema_version_uses_the_current_one() {
        let library = ProfileLibrary::with_builtins();
        let mut value = serde_json::to_value(&library).unwrap();
        value.as_object_mut().unwrap().remove("schema_version");
        let restored: ProfileLibrary = serde_json::from_value(value).unwrap();
        assert_eq!(restored.schema_version, PROFILE_SCHEMA_VERSION);
        assert_eq!(restored, library);
    }

    #[test]
    fn missing_files_and_directories_report_io_errors() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-profiles.json");
        assert!(matches!(
            ProfileLibrary::load_from_path(&missing),
            Err(ProfileError::Io { .. })
        ));

        let library = ProfileLibrary::with_builtins();
        let bad_target = directory
            .path()
            .join("no-such-directory")
            .join("profiles.json");
        assert!(matches!(
            library.save_to_path(&bad_target),
            Err(ProfileError::Io { .. })
        ));
    }
}
