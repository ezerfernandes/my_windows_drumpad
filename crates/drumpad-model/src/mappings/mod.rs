//! Platform-neutral keyboard profiles.
//!
//! The kit stores one key mapping per pad (`KeyMapping`). Profiles are a
//! higher-level, shareable list of pad-to-key bindings used by the input and
//! settings layers. They are plain data so they can travel with a kit or live
//! in the app configuration.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::kit::{KeyMapping, Kit, PAD_COUNT, PadId};

pub use crate::kit::KeyMapping as PadKeyMapping;
pub use crate::kit::PhysicalKey;

/// One pad-to-key binding inside a profile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct KeyBinding {
    /// Pad that receives the trigger.
    pub pad: PadId,
    /// Physical key that triggers it.
    pub key: PhysicalKey,
}

impl KeyBinding {
    /// Construct.
    pub const fn new(pad: PadId, key: PhysicalKey) -> Self {
        Self { pad, key }
    }
}

/// A named set of physical-key bindings.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KeyboardProfile {
    /// Display name, for example `Full keyboard` or `Finding oNe`.
    pub name: String,
    /// Bindings, one per pad in the ideal case.
    pub bindings: Vec<KeyBinding>,
}

impl KeyboardProfile {
    /// Build an empty profile.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bindings: Vec::new(),
        }
    }

    /// Build the default 4x4 full-keyboard profile (`1 2 3 4` ... `Z X C V`).
    pub fn default_full_keyboard() -> Self {
        let mut profile = Self::new("Full keyboard");
        for slot in 0..PAD_COUNT {
            if let Some(mapping) = KeyMapping::default_for(slot) {
                profile.bindings.push(KeyBinding::new(
                    PadId::from_raw_unchecked(slot as u8),
                    mapping.key,
                ));
            }
        }
        profile
    }

    /// Build a profile from a kit's current pad mappings.
    pub fn from_kit(name: impl Into<String>, kit: &Kit) -> Self {
        let mut profile = Self::new(name);
        for pad in &kit.pads {
            if let Some(mapping) = pad.key {
                profile.bindings.push(KeyBinding::new(pad.id, mapping.key));
            }
        }
        profile
    }

    /// Look up the pad bound to a key.
    pub fn pad_for_key(&self, key: PhysicalKey) -> Option<PadId> {
        self.bindings
            .iter()
            .find(|binding| binding.key == key)
            .map(|binding| binding.pad)
    }

    /// Look up the key bound to a pad.
    pub fn key_for_pad(&self, pad: PadId) -> Option<PhysicalKey> {
        self.bindings
            .iter()
            .find(|binding| binding.pad == pad)
            .map(|binding| binding.key)
    }

    /// Reject duplicate pads or keys and out-of-range pad ids.
    pub fn validate(&self) -> Result<(), MappingError> {
        let mut pads = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for binding in &self.bindings {
            if binding.pad.get() > PadId::MAX {
                return Err(MappingError::PadOutOfRange(binding.pad));
            }
            if !pads.insert(binding.pad) {
                return Err(MappingError::DuplicatePad(binding.pad));
            }
            if !keys.insert(binding.key) {
                return Err(MappingError::DuplicateKey(binding.key));
            }
        }
        Ok(())
    }

    /// Apply this profile's bindings to a kit, clearing unmapped pads.
    pub fn apply_to(&self, kit: &mut Kit) {
        for pad in &mut kit.pads {
            pad.key = None;
        }
        for binding in &self.bindings {
            if let Some(pad) = kit.pad_mut(binding.pad) {
                pad.key = Some(KeyMapping::new(binding.key));
            }
        }
    }
}

impl Default for KeyboardProfile {
    fn default() -> Self {
        Self::default_full_keyboard()
    }
}

/// Profile validation errors.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum MappingError {
    /// A binding uses a pad id outside the 0..=15 range.
    #[error("pad {} is outside 0..={}", .0.get(), PadId::MAX)]
    PadOutOfRange(PadId),
    /// Two bindings share a pad.
    #[error("pad {} is bound more than once", .0.get())]
    DuplicatePad(PadId),
    /// Two bindings share a key.
    #[error("physical key {} is bound more than once", .0.raw())]
    DuplicateKey(PhysicalKey),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::Kit;

    #[test]
    fn default_profile_covers_all_pads() {
        let profile = KeyboardProfile::default_full_keyboard();
        assert_eq!(profile.bindings.len(), PAD_COUNT);
        profile.validate().unwrap();
        for slot in 0..PAD_COUNT {
            let pad = PadId::from_raw_unchecked(slot as u8);
            assert!(profile.key_for_pad(pad).is_some());
        }
    }

    #[test]
    fn profile_round_trips_through_kit() {
        let mut kit = Kit::silent_default();
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings[0].key = PhysicalKey::from_scancode(0x39);
        profile.validate().unwrap();
        profile.apply_to(&mut kit);
        assert_eq!(kit.pads[0].key.unwrap().key.raw(), 0x39);
        let back = KeyboardProfile::from_kit("copy", &kit);
        assert_eq!(back.key_for_pad(kit.pads[0].id).unwrap().raw(), 0x39);
    }

    #[test]
    fn out_of_range_pad_is_rejected() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings[0].pad = PadId(99);
        assert!(matches!(
            profile.validate(),
            Err(MappingError::PadOutOfRange(_))
        ));
    }

    #[test]
    fn conflicts_are_rejected() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings[1].key = profile.bindings[0].key;
        assert!(matches!(
            profile.validate(),
            Err(MappingError::DuplicateKey(_))
        ));
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings[1].pad = profile.bindings[0].pad;
        assert!(matches!(
            profile.validate(),
            Err(MappingError::DuplicatePad(_))
        ));
    }

    #[test]
    fn profile_serde_round_trip() {
        let profile = KeyboardProfile::default_full_keyboard();
        let json = serde_json::to_string(&profile).unwrap();
        let back: KeyboardProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, profile);
    }
}
