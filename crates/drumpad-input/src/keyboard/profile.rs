//! Keyboard and one-hand keypad profiles.
//!
//! A profile is the durable, serializable answer to "what does each physical
//! key do?". The built-in full-keyboard profile is the reference 4x4 grid:
//!
//! ```text
//! 1 2 3 4        Q W E R        A S D F        Z X C V
//! ```
//!
//! with `F1..F5` reserved for transport and the fifth column (`5 T G B`) plus
//! `Space` explicitly unmapped so a stray palm hit cannot fire a pad.
//!
//! Rebinding happens by handing the runtime a new profile; nothing restarts.
//! Profiles serialize as JSON so they can travel with a kit or move between
//! machines. [`KeyboardProfile::conflicts`] is the single source of truth for
//! duplicate pads, duplicate keys and duplicate transport actions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::keyboard::keys::PhysicalKey;
use crate::trigger::{PadId, PadIdExt, pad_label};

/// Current profile schema version.
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Identifier of the built-in full-keyboard profile.
pub const FULL_KEYBOARD_PROFILE_ID: &str = "full-keyboard";

/// Identifier of the Finding 35-key single-hand keypad profile.
pub const FINDING_35_PROFILE_ID: &str = "finding-35-key";

/// What a key does when pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    /// Stop every voice immediately.
    Panic,
    /// Toggle the metronome.
    ToggleMetronome,
    /// Select the previous kit.
    PreviousKit,
    /// Select the next kit.
    NextKit,
    /// Show the full keyboard/keypad map overlay.
    ShowMapOverlay,
}

impl TransportAction {
    /// Every transport action, in F1..F5 order.
    pub const ALL: [Self; 5] = [
        Self::Panic,
        Self::ToggleMetronome,
        Self::PreviousKit,
        Self::NextKit,
        Self::ShowMapOverlay,
    ];

    /// Short label for the map overlay and status bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Panic => "Panic",
            Self::ToggleMetronome => "Metronome",
            Self::PreviousKit => "Previous kit",
            Self::NextKit => "Next kit",
            Self::ShowMapOverlay => "Map overlay",
        }
    }

    /// Stable identifier used in profiles and telemetry.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::ToggleMetronome => "toggle_metronome",
            Self::PreviousKit => "previous_kit",
            Self::NextKit => "next_kit",
            Self::ShowMapOverlay => "show_map_overlay",
        }
    }
}

impl core::fmt::Display for TransportAction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.label())
    }
}

/// The action a physical key performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum KeyAction {
    /// Trigger a performance pad.
    Pad(PadId),
    /// Run a transport command.
    Transport(TransportAction),
}

impl KeyAction {
    /// Build a pad action.
    #[must_use]
    pub const fn pad(pad: PadId) -> Self {
        Self::Pad(pad)
    }

    /// Build a transport action.
    #[must_use]
    pub const fn transport(action: TransportAction) -> Self {
        Self::Transport(action)
    }

    /// Pad this action triggers, if any.
    #[must_use]
    pub const fn as_pad(self) -> Option<PadId> {
        match self {
            Self::Pad(pad) => Some(pad),
            Self::Transport(_) => None,
        }
    }

    /// Transport command this action runs, if any.
    #[must_use]
    pub const fn as_transport(self) -> Option<TransportAction> {
        match self {
            Self::Transport(action) => Some(action),
            Self::Pad(_) => None,
        }
    }
}

impl core::fmt::Display for KeyAction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pad(pad) => f.write_str(&pad_label(*pad)),
            Self::Transport(action) => write!(f, "{action}"),
        }
    }
}

/// One key-to-action assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    /// Physical key.
    pub key: PhysicalKey,
    /// Action performed while pressed.
    pub action: KeyAction,
}

impl KeyBinding {
    /// Build a binding.
    #[must_use]
    pub const fn new(key: PhysicalKey, action: KeyAction) -> Self {
        Self { key, action }
    }
}

/// Which physical device a profile was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileDevice {
    /// A normal full-size keyboard.
    #[default]
    FullKeyboard,
    /// A single-hand gamer keypad such as the Finding 35-key.
    OneHandKeypad,
    /// Any other device the user captured.
    Other,
}

/// Why a profile cannot be used as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingConflict {
    /// The same physical key is bound more than once.
    Key {
        /// Conflicting key.
        key: PhysicalKey,
        /// Actions bound to it.
        actions: Vec<KeyAction>,
    },
    /// The same pad is reachable from more than one key.
    Pad {
        /// Conflicting pad.
        pad: PadId,
        /// Keys bound to it.
        keys: Vec<PhysicalKey>,
    },
    /// The same transport action is bound more than once.
    Transport {
        /// Conflicting action.
        action: TransportAction,
        /// Keys bound to it.
        keys: Vec<PhysicalKey>,
    },
}

impl MappingConflict {
    /// Short human-readable explanation for the mapping editor.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Key { key, actions } => {
                let names: Vec<String> = actions.iter().map(ToString::to_string).collect();
                format!("{key} is assigned to {}", names.join(" and "))
            }
            Self::Pad { pad, keys } => {
                let names: Vec<String> = keys.iter().map(ToString::to_string).collect();
                format!("{} is assigned to {}", pad_label(*pad), names.join(" and "))
            }
            Self::Transport { action, keys } => {
                let names: Vec<String> = keys.iter().map(ToString::to_string).collect();
                format!("{action} is assigned to {}", names.join(" and "))
            }
        }
    }
}

impl core::fmt::Display for MappingConflict {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// Typed failures from profile parsing, validation and saving.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// The profile contains conflicting assignments.
    #[error("profile '{id}' has {count} conflicting binding(s)")]
    Conflicts {
        /// Profile identifier.
        id: String,
        /// Number of conflicts.
        count: usize,
        /// Full conflict list.
        conflicts: Vec<MappingConflict>,
    },

    /// The profile is empty.
    #[error("profile '{id}' has no bindings")]
    Empty {
        /// Profile identifier.
        id: String,
    },

    /// JSON encoding or decoding failed.
    #[error("profile JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),

    /// Reading or writing a profile file failed.
    #[error("could not access profile file {path}")]
    Io {
        /// File that failed.
        path: std::path::PathBuf,
        /// Underlying operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// The file was written by a newer build.
    #[error("profile schema version {found} is newer than supported version {supported}")]
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u32,
        /// Highest version this build understands.
        supported: u32,
    },
}

impl ProfileError {
    /// Conflicts carried by this error, if any.
    #[must_use]
    pub fn conflicts(&self) -> &[MappingConflict] {
        match self {
            Self::Conflicts { conflicts, .. } => conflicts,
            _ => &[],
        }
    }
}

fn current_schema_version() -> u32 {
    PROFILE_SCHEMA_VERSION
}

/// A durable keyboard or keypad mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardProfile {
    /// Schema version, used to refuse profiles from a newer build.
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    /// Stable identifier, unique within a profile library.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Device class this profile was captured on.
    #[serde(default)]
    pub device: ProfileDevice,
    /// Key-to-action assignments.
    pub bindings: Vec<KeyBinding>,
    /// Keys the user explicitly left without an action.
    #[serde(default)]
    pub unmapped: Vec<PhysicalKey>,
}

impl KeyboardProfile {
    /// The default full-keyboard profile.
    #[must_use]
    pub fn default_full_keyboard() -> Self {
        Self::from_pad_block(
            FULL_KEYBOARD_PROFILE_ID,
            "Full keyboard",
            ProfileDevice::FullKeyboard,
        )
    }

    /// The Finding 35-key single-hand USB keypad profile
    /// (ASIN B09HF5WDR2).
    ///
    /// Its QWERTY block holds the same 4x4 grid, so the pad bindings are the
    /// defaults. The left macro column may only control lighting, so the app
    /// does not rely on it.
    #[must_use]
    pub fn finding_35_key() -> Self {
        Self::from_pad_block(
            FINDING_35_PROFILE_ID,
            "Finding 35-key keypad",
            ProfileDevice::OneHandKeypad,
        )
    }

    /// Every built-in profile.
    #[must_use]
    pub fn builtins() -> [Self; 2] {
        [Self::default_full_keyboard(), Self::finding_35_key()]
    }

    fn from_pad_block(id: &str, name: &str, device: ProfileDevice) -> Self {
        let mut bindings = Vec::with_capacity(21);
        for (pad, key) in PadId::ALL.iter().zip(PhysicalKey::PAD_BLOCK) {
            bindings.push(KeyBinding::new(key, KeyAction::pad(*pad)));
        }
        for (key, action) in PhysicalKey::TRANSPORT_KEYS.iter().zip(TransportAction::ALL) {
            bindings.push(KeyBinding::new(*key, KeyAction::transport(action)));
        }
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            id: id.to_owned(),
            name: name.to_owned(),
            device,
            bindings,
            unmapped: PhysicalKey::UNMAPPED_COLUMN.to_vec(),
        }
    }

    /// Action assigned to `key`, if any.
    #[must_use]
    pub fn action_for(&self, key: PhysicalKey) -> Option<KeyAction> {
        self.bindings
            .iter()
            .find(|binding| binding.key == key)
            .map(|binding| binding.action)
    }

    /// First key assigned to `action`, if any.
    #[must_use]
    pub fn key_for(&self, action: KeyAction) -> Option<PhysicalKey> {
        self.bindings
            .iter()
            .find(|binding| binding.action == action)
            .map(|binding| binding.key)
    }

    /// All keys assigned to `pad`.
    #[must_use]
    pub fn keys_for_pad(&self, pad: PadId) -> Vec<PhysicalKey> {
        self.bindings
            .iter()
            .filter(|binding| binding.action.as_pad() == Some(pad))
            .map(|binding| binding.key)
            .collect()
    }

    /// True when the profile explicitly leaves `key` unmapped.
    #[must_use]
    pub fn is_explicitly_unmapped(&self, key: PhysicalKey) -> bool {
        self.unmapped.contains(&key)
    }

    /// Assign `action` to `key`, returning the bindings it displaced.
    ///
    /// The displaced list lets the mapping editor offer undo. Binding a key
    /// also clears it from the explicit unmapped list.
    pub fn set_binding(&mut self, key: PhysicalKey, action: KeyAction) -> Vec<KeyBinding> {
        let mut displaced = Vec::new();
        self.bindings.retain(|binding| {
            if binding.key == key || binding.action == action {
                displaced.push(*binding);
                false
            } else {
                true
            }
        });
        self.bindings.push(KeyBinding::new(key, action));
        self.unmapped.retain(|unmapped| *unmapped != key);
        displaced
    }

    /// Remove every binding for `key` and mark the key as intentionally
    /// unmapped.
    pub fn unbind(&mut self, key: PhysicalKey) -> Option<KeyBinding> {
        let mut removed = None;
        self.bindings.retain(|binding| {
            if binding.key == key {
                removed = Some(*binding);
                false
            } else {
                true
            }
        });
        if !self.unmapped.contains(&key) {
            self.unmapped.push(key);
        }
        removed
    }

    /// Restore the built-in map for this profile's device class while keeping
    /// its identity.
    pub fn reset_to_default(&mut self) {
        let defaults = match self.device {
            ProfileDevice::OneHandKeypad => Self::finding_35_key(),
            ProfileDevice::FullKeyboard | ProfileDevice::Other => Self::default_full_keyboard(),
        };
        self.bindings = defaults.bindings;
        self.unmapped = defaults.unmapped;
    }

    /// All duplicate assignments in a stable order.
    #[must_use]
    pub fn conflicts(&self) -> Vec<MappingConflict> {
        let mut conflicts = Vec::new();

        let mut by_key: BTreeMap<PhysicalKey, Vec<KeyAction>> = BTreeMap::new();
        for binding in &self.bindings {
            by_key.entry(binding.key).or_default().push(binding.action);
        }
        for (key, mut actions) in by_key {
            if actions.len() > 1 {
                actions.sort_unstable();
                actions.dedup();
                conflicts.push(MappingConflict::Key { key, actions });
            }
        }

        let mut by_pad: BTreeMap<PadId, Vec<PhysicalKey>> = BTreeMap::new();
        for binding in &self.bindings {
            if let Some(pad) = binding.action.as_pad() {
                by_pad.entry(pad).or_default().push(binding.key);
            }
        }
        for (pad, mut keys) in by_pad {
            if keys.len() > 1 {
                keys.sort_unstable();
                keys.dedup();
                conflicts.push(MappingConflict::Pad { pad, keys });
            }
        }

        let mut by_transport: BTreeMap<TransportAction, Vec<PhysicalKey>> = BTreeMap::new();
        for binding in &self.bindings {
            if let Some(action) = binding.action.as_transport() {
                by_transport.entry(action).or_default().push(binding.key);
            }
        }
        for (action, mut keys) in by_transport {
            if keys.len() > 1 {
                keys.sort_unstable();
                keys.dedup();
                conflicts.push(MappingConflict::Transport { action, keys });
            }
        }

        conflicts
    }

    /// True when the profile has at least one binding and no conflicts.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.bindings.is_empty() && self.conflicts().is_empty()
    }

    /// Validate the profile, reporting every conflict at once.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.bindings.is_empty() {
            return Err(ProfileError::Empty {
                id: self.id.clone(),
            });
        }
        let conflicts = self.conflicts();
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(ProfileError::Conflicts {
                id: self.id.clone(),
                count: conflicts.len(),
                conflicts,
            })
        }
    }

    /// Pretty JSON suitable for export.
    pub fn to_json(&self) -> Result<String, ProfileError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse an exported profile.
    ///
    /// Conflicts are *not* rejected here: the mapping editor must show them
    /// and let the user decide. Call [`KeyboardProfile::validate`] before the
    /// profile goes live.
    pub fn from_json(json: &str) -> Result<Self, ProfileError> {
        let profile: Self = serde_json::from_str(json)?;
        if profile.schema_version > PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchemaVersion {
                found: profile.schema_version,
                supported: PROFILE_SCHEMA_VERSION,
            });
        }
        Ok(profile)
    }

    /// Pad-only view used when a profile travels with a kit.
    ///
    /// `drumpad_model::KeyboardProfile` stores pad bindings and raw scan codes;
    /// transport actions and the explicit unmapped list are input-side editor
    /// metadata and are intentionally not part of the kit format.
    #[must_use]
    pub fn to_model_profile(&self) -> drumpad_model::KeyboardProfile {
        let bindings = self
            .bindings
            .iter()
            .filter_map(|binding| match binding.action {
                KeyAction::Pad(pad) => Some(drumpad_model::KeyBinding {
                    pad,
                    key: binding.key.into(),
                }),
                KeyAction::Transport(_) => None,
            })
            .collect();
        drumpad_model::KeyboardProfile {
            name: self.name.clone(),
            bindings,
        }
    }

    /// Build an editor profile from the pad bindings stored with a kit.
    ///
    /// The model profile does not carry transport actions or explicit unmapped
    /// keys, so those start empty. The profile takes the model name as its id
    /// and display name; callers can rename it afterwards.
    #[must_use]
    pub fn from_model_profile(
        profile: &drumpad_model::KeyboardProfile,
        device: ProfileDevice,
    ) -> Self {
        let bindings = profile
            .bindings
            .iter()
            .map(|binding| {
                KeyBinding::new(PhysicalKey::from(binding.key), KeyAction::pad(binding.pad))
            })
            .collect();
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            id: profile.name.clone(),
            name: profile.name.clone(),
            device,
            bindings,
            unmapped: Vec::new(),
        }
    }
}

impl Default for KeyboardProfile {
    fn default() -> Self {
        Self::default_full_keyboard()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(index: u8) -> PadId {
        PadId::from_index(index)
    }

    #[test]
    fn default_profile_covers_all_pads_and_transport() {
        let profile = KeyboardProfile::default_full_keyboard();
        assert_eq!(profile.bindings.len(), 21);
        for pad in PadId::ALL {
            let keys = profile.keys_for_pad(pad);
            assert_eq!(keys.len(), 1, "{pad} must have exactly one key");
            assert!(keys[0].is_pad_key());
            assert_eq!(profile.action_for(keys[0]), Some(KeyAction::pad(pad)));
        }
        for (key, action) in PhysicalKey::TRANSPORT_KEYS.iter().zip(TransportAction::ALL) {
            assert_eq!(profile.action_for(*key), Some(KeyAction::transport(action)));
        }
        assert!(profile.is_valid());
    }

    #[test]
    fn default_profile_leaves_the_fifth_column_and_space_unmapped() {
        let profile = KeyboardProfile::default_full_keyboard();
        for key in PhysicalKey::UNMAPPED_COLUMN {
            assert_eq!(profile.action_for(key), None, "{key} must be unmapped");
            assert!(profile.is_explicitly_unmapped(key));
        }
    }

    #[test]
    fn finding_profile_matches_the_reference_keypad() {
        let profile = KeyboardProfile::finding_35_key();
        assert_eq!(profile.device, ProfileDevice::OneHandKeypad);
        assert_eq!(profile.id, FINDING_35_PROFILE_ID);
        for (pad, key) in PadId::ALL.iter().zip(PhysicalKey::PAD_BLOCK) {
            assert_eq!(profile.action_for(key), Some(KeyAction::pad(*pad)));
        }
        let expected = [
            (PhysicalKey::F1, TransportAction::Panic),
            (PhysicalKey::F2, TransportAction::ToggleMetronome),
            (PhysicalKey::F3, TransportAction::PreviousKit),
            (PhysicalKey::F4, TransportAction::NextKit),
            (PhysicalKey::F5, TransportAction::ShowMapOverlay),
        ];
        for (key, action) in expected {
            assert_eq!(
                profile.action_for(key),
                Some(KeyAction::transport(action)),
                "{key} must run {action}"
            );
        }
        for key in [
            PhysicalKey::Digit5,
            PhysicalKey::KeyT,
            PhysicalKey::KeyG,
            PhysicalKey::KeyB,
            PhysicalKey::Space,
        ] {
            assert_eq!(profile.action_for(key), None);
            assert!(profile.is_explicitly_unmapped(key));
        }
        assert!(profile.is_valid());
    }

    #[test]
    fn builtins_have_distinct_identities() {
        let builtins = KeyboardProfile::builtins();
        assert_eq!(builtins.len(), 2);
        assert_ne!(builtins[0].id, builtins[1].id);
        assert_ne!(builtins[0].device, builtins[1].device);
    }

    #[test]
    fn duplicate_pad_bindings_are_reported() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings.push(KeyBinding::new(
            PhysicalKey::Other(7),
            KeyAction::pad(pad(0)),
        ));
        let conflicts = profile.conflicts();
        assert_eq!(conflicts.len(), 1);
        let MappingConflict::Pad {
            pad: conflict_pad,
            keys,
        } = &conflicts[0]
        else {
            panic!("expected pad conflict, got {conflicts:?}");
        };
        assert_eq!(*conflict_pad, pad(0));
        assert_eq!(keys, &[PhysicalKey::Digit1, PhysicalKey::Other(7)]);
        assert!(profile.validate().is_err());
        assert!(!profile.is_valid());
    }

    #[test]
    fn duplicate_key_bindings_are_reported() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings.push(KeyBinding::new(
            PhysicalKey::Digit1,
            KeyAction::transport(TransportAction::Panic),
        ));
        let conflicts = profile.conflicts();
        assert!(
            conflicts.iter().any(|conflict| matches!(
                conflict,
                MappingConflict::Key { key, .. } if *key == PhysicalKey::Digit1
            )),
            "expected key conflict, got {conflicts:?}"
        );
    }

    #[test]
    fn duplicate_transport_bindings_are_reported() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings.push(KeyBinding::new(
            PhysicalKey::Other(9),
            KeyAction::transport(TransportAction::Panic),
        ));
        let conflicts = profile.conflicts();
        assert!(conflicts.iter().any(|conflict| matches!(
            conflict,
            MappingConflict::Transport { action, .. } if *action == TransportAction::Panic
        )));
        let error = profile.validate().unwrap_err();
        assert_eq!(error.conflicts().len(), conflicts.len());
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn rebinding_replaces_the_old_key_and_clears_unmapped() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        let displaced = profile.set_binding(PhysicalKey::Digit5, KeyAction::pad(pad(3)));
        assert_eq!(
            displaced,
            vec![KeyBinding::new(PhysicalKey::Digit4, KeyAction::pad(pad(3)))]
        );
        assert_eq!(
            profile.action_for(PhysicalKey::Digit5),
            Some(KeyAction::pad(pad(3)))
        );
        assert_eq!(profile.action_for(PhysicalKey::Digit4), None);
        assert!(!profile.is_explicitly_unmapped(PhysicalKey::Digit5));
        assert!(profile.is_valid());
    }

    #[test]
    fn unbinding_marks_the_key_unmapped() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        let removed = profile.unbind(PhysicalKey::KeyQ);
        assert_eq!(
            removed,
            Some(KeyBinding::new(PhysicalKey::KeyQ, KeyAction::pad(pad(4))))
        );
        assert_eq!(profile.action_for(PhysicalKey::KeyQ), None);
        assert!(profile.is_explicitly_unmapped(PhysicalKey::KeyQ));
        assert_eq!(profile.keys_for_pad(pad(4)), Vec::<PhysicalKey>::new());
    }

    #[test]
    fn reset_restores_the_built_in_map() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.unbind(PhysicalKey::Digit1);
        profile.set_binding(
            PhysicalKey::Other(11),
            KeyAction::transport(TransportAction::Panic),
        );
        profile.reset_to_default();
        assert_eq!(profile, KeyboardProfile::default_full_keyboard());
    }

    #[test]
    fn model_profile_preserves_pad_bindings() {
        let original = KeyboardProfile::default_full_keyboard();
        let model = original.to_model_profile();
        assert_eq!(model.bindings.len(), 16);
        let restored = KeyboardProfile::from_model_profile(&model, ProfileDevice::FullKeyboard);
        for pad in PadId::ALL {
            assert_eq!(restored.keys_for_pad(pad), original.keys_for_pad(pad));
        }
    }

    #[test]
    fn profiles_round_trip_without_loss() {
        let mut profile = KeyboardProfile::finding_35_key();
        profile.set_binding(PhysicalKey::Other(0x56), KeyAction::pad(pad(0)));
        let json = profile.to_json().unwrap();
        let restored = KeyboardProfile::from_json(&json).unwrap();
        assert_eq!(restored, profile);
        assert_eq!(
            restored.action_for(PhysicalKey::Other(0x56)),
            Some(KeyAction::pad(pad(0)))
        );
    }

    #[test]
    fn future_schema_versions_are_refused() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.schema_version = PROFILE_SCHEMA_VERSION + 1;
        let json = serde_json::to_string(&profile).unwrap();
        let error = KeyboardProfile::from_json(&json).unwrap_err();
        assert!(matches!(
            error,
            ProfileError::UnsupportedSchemaVersion { .. }
        ));
    }

    #[test]
    fn conflicts_render_readable_messages() {
        let conflict = MappingConflict::Pad {
            pad: pad(2),
            keys: vec![PhysicalKey::Digit3, PhysicalKey::Other(4)],
        };
        assert_eq!(conflict.describe(), "pad 3 is assigned to 3 and Scan 4");
    }
}
