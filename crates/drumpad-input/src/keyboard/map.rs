//! Immutable runtime lookup table built from a [`KeyboardProfile`].
//!
//! Building a `KeyMap` validates the profile once, sorts its bindings, and
//! gives the capture path an allocation-free binary search. Rebuilding a
//! `KeyMap` is how a rebind takes effect without restarting the app: the app
//! swaps the map behind its state and the next key event sees the new table.

use crate::keyboard::keys::PhysicalKey;
use crate::keyboard::profile::{
    KeyAction, KeyBinding, KeyboardProfile, ProfileError, TransportAction,
};
use crate::trigger::PadId;

/// Validated, sorted key-to-action lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMap {
    bindings: Box<[KeyBinding]>,
    unmapped: Box<[PhysicalKey]>,
}

impl KeyMap {
    /// Build a runtime map, rejecting conflicting or empty profiles.
    pub fn new(profile: &KeyboardProfile) -> Result<Self, ProfileError> {
        profile.validate()?;
        let mut bindings = profile.bindings.clone();
        bindings.sort_unstable_by_key(|binding| binding.key);
        let mut unmapped = profile.unmapped.clone();
        unmapped.sort_unstable();
        unmapped.dedup();
        Ok(Self {
            bindings: bindings.into_boxed_slice(),
            unmapped: unmapped.into_boxed_slice(),
        })
    }

    /// Action bound to `key`.
    #[must_use]
    pub fn action_for(&self, key: PhysicalKey) -> Option<KeyAction> {
        self.bindings
            .binary_search_by(|binding| binding.key.cmp(&key))
            .ok()
            .map(|index| self.bindings[index].action)
    }

    /// True when the profile explicitly leaves `key` without an action.
    #[must_use]
    pub fn is_unmapped(&self, key: PhysicalKey) -> bool {
        self.unmapped.binary_search(&key).is_ok()
    }

    /// First key bound to `pad`, used for pad labels.
    #[must_use]
    pub fn key_for_pad(&self, pad: PadId) -> Option<PhysicalKey> {
        self.bindings
            .iter()
            .find(|binding| binding.action.as_pad() == Some(pad))
            .map(|binding| binding.key)
    }

    /// Key bound to a transport action.
    #[must_use]
    pub fn key_for_transport(&self, action: TransportAction) -> Option<PhysicalKey> {
        self.bindings
            .iter()
            .find(|binding| binding.action.as_transport() == Some(action))
            .map(|binding| binding.key)
    }

    /// Sorted bindings, suitable for the map overlay.
    #[must_use]
    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    /// Explicitly unmapped keys, sorted.
    #[must_use]
    pub fn unmapped(&self) -> &[PhysicalKey] {
        &self.unmapped
    }

    /// Number of live bindings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// True when nothing is bound (impossible for a validated map).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::PadIdExt;

    #[test]
    fn lookup_returns_the_bound_action() {
        let map = KeyMap::new(&KeyboardProfile::default_full_keyboard()).unwrap();
        assert_eq!(
            map.action_for(PhysicalKey::Digit1),
            Some(KeyAction::pad(PadId::from_index(0)))
        );
        assert_eq!(
            map.action_for(PhysicalKey::F1),
            Some(KeyAction::transport(TransportAction::Panic))
        );
        assert_eq!(map.action_for(PhysicalKey::Digit5), None);
        assert!(map.is_unmapped(PhysicalKey::Digit5));
        assert!(!map.is_unmapped(PhysicalKey::Digit1));
        assert_eq!(map.action_for(PhysicalKey::Other(1)), None);
    }

    #[test]
    fn pad_and_transport_labels_resolve() {
        let map = KeyMap::new(&KeyboardProfile::finding_35_key()).unwrap();
        for pad in PadId::ALL {
            let key = map.key_for_pad(pad).unwrap();
            assert_eq!(map.action_for(key), Some(KeyAction::pad(pad)));
        }
        assert_eq!(
            map.key_for_transport(TransportAction::ShowMapOverlay),
            Some(PhysicalKey::F5)
        );
    }

    #[test]
    fn conflicting_profiles_are_rejected() {
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.bindings.push(KeyBinding::new(
            PhysicalKey::Other(3),
            KeyAction::pad(PadId::from_index(0)),
        ));
        assert!(KeyMap::new(&profile).is_err());
    }

    #[test]
    fn bindings_are_sorted_for_binary_search() {
        let map = KeyMap::new(&KeyboardProfile::default_full_keyboard()).unwrap();
        let keys: Vec<PhysicalKey> = map.bindings().iter().map(|binding| binding.key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
        assert_eq!(map.len(), 21);
        assert!(!map.is_empty());
    }
}
