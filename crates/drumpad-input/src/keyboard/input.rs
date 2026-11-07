//! Keyboard capture state machine.
//!
//! The app feeds raw key transitions and a monotonic timestamp; this module
//! turns them into the shared [`TriggerEvent`]/[`ReleaseEvent`] vocabulary and
//! enforces the performance rules:
//!
//! - key down triggers once; OS auto-repeat is suppressed until key up;
//! - key up produces a gate-mode release for pad bindings;
//! - transport keys act once per press and never produce a release;
//! - focus loss and explicit `release_all` clear every held key, so alt-tab or
//!   a missed key-up cannot leave a gated voice stuck.
//!
//! No work here allocates on the steady-state path: the held-key list is
//! preallocated to the number of bindings.

use crate::keyboard::keys::PhysicalKey;
use crate::keyboard::map::KeyMap;
use crate::keyboard::profile::{KeyAction, TransportAction};
use crate::keyboard::velocity::{KeyboardVelocityConfig, VelocityGenerator};
use crate::trigger::{PadId, ReleaseEvent, TriggerEvent, TriggerSource};

/// What one key transition produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardAction {
    /// A pad was struck.
    Trigger(TriggerEvent),
    /// A held pad was released.
    Release(ReleaseEvent),
    /// A transport command fired.
    Transport(TransportAction),
    /// Nothing fired, for the stated reason.
    Ignored(IgnoreReason),
}

impl KeyboardAction {
    /// Convert to the shared queue event, when the action produces one.
    #[must_use]
    pub fn event(self) -> Option<crate::trigger::InputEvent> {
        match self {
            Self::Trigger(event) => Some(event.into()),
            Self::Release(event) => Some(event.into()),
            Self::Transport(_) | Self::Ignored(_) => None,
        }
    }

    /// Transport command, when the action is one.
    #[must_use]
    pub fn transport(self) -> Option<TransportAction> {
        match self {
            Self::Transport(action) => Some(action),
            _ => None,
        }
    }

    /// True when the transition produced no event or command.
    #[must_use]
    pub fn is_ignored(self) -> bool {
        matches!(self, Self::Ignored(_))
    }

    /// Pad the action applies to, if any.
    #[must_use]
    pub fn pad(self) -> Option<PadId> {
        match self {
            Self::Trigger(event) => Some(event.pad),
            Self::Release(event) => Some(event.pad),
            Self::Transport(_) | Self::Ignored(_) => None,
        }
    }
}

/// Why a key transition did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    /// The key has no binding in the active profile.
    Unmapped,
    /// The key is already down; this was an OS auto-repeat.
    Repeat,
    /// Key up arrived for a key that was not held.
    NotHeld,
    /// Key up completed a transport action, which has no release.
    NoRelease,
}

/// Result of clearing held keys, used on focus loss.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseAll {
    /// Gate-mode releases for every held pad.
    pub releases: Vec<ReleaseEvent>,
    /// Pads that were held and are now released.
    pub pads: Vec<PadId>,
}

impl ReleaseAll {
    /// True when nothing was held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.releases.is_empty()
    }
}

/// Stateful keyboard capture path.
#[derive(Debug, Clone)]
pub struct KeyboardInput {
    map: KeyMap,
    velocity: VelocityGenerator,
    held: Vec<(PhysicalKey, KeyAction)>,
}

impl KeyboardInput {
    /// Build a capture path around a validated map.
    #[must_use]
    pub fn new(map: KeyMap, velocity: KeyboardVelocityConfig, seed: u64) -> Self {
        let capacity = map.len();
        Self {
            map,
            velocity: VelocityGenerator::new(velocity, seed),
            held: Vec::with_capacity(capacity),
        }
    }

    /// Build with the default profile, fixed velocity and seed.
    #[must_use]
    pub fn default_with_profile(profile: &crate::keyboard::profile::KeyboardProfile) -> Self {
        let map = KeyMap::new(profile).expect("built-in profile must be valid");
        Self::new(map, KeyboardVelocityConfig::default(), 0)
    }

    /// Active map.
    #[must_use]
    pub fn map(&self) -> &KeyMap {
        &self.map
    }

    /// Active velocity configuration.
    #[must_use]
    pub fn velocity_config(&self) -> &KeyboardVelocityConfig {
        self.velocity.config()
    }

    /// Replace the velocity configuration.
    pub fn set_velocity_config(&mut self, config: KeyboardVelocityConfig) {
        self.velocity.set_config(config);
    }

    /// Replace the key map mid-session.
    ///
    /// Keys held under the old map are released first so a rebind can never
    /// strand a gated voice.
    pub fn replace_map(&mut self, map: KeyMap, timestamp_us: u64) -> ReleaseAll {
        let released = self.release_all(timestamp_us);
        self.map = map;
        self.held.reserve(self.map.len());
        released
    }

    /// Handle a key press.
    pub fn key_down(&mut self, key: PhysicalKey, timestamp_us: u64) -> KeyboardAction {
        if self.is_held(key) {
            return KeyboardAction::Ignored(IgnoreReason::Repeat);
        }
        let Some(action) = self.map.action_for(key) else {
            return KeyboardAction::Ignored(IgnoreReason::Unmapped);
        };
        if self.held.len() == self.held.capacity() {
            // The reservation exactly matches the map, so this cannot happen
            // for a validated map; grow rather than lose a hit if a future map
            // adds bindings dynamically.
            self.held.reserve(1);
        }
        self.held.push((key, action));
        match action {
            KeyAction::Pad(pad) => {
                let velocity = self.velocity.velocity_for(pad);
                KeyboardAction::Trigger(TriggerEvent::new(
                    pad,
                    velocity,
                    TriggerSource::Keyboard,
                    timestamp_us,
                ))
            }
            KeyAction::Transport(transport) => KeyboardAction::Transport(transport),
        }
    }

    /// Handle a key release.
    pub fn key_up(&mut self, key: PhysicalKey, timestamp_us: u64) -> KeyboardAction {
        let Some(index) = self.held.iter().position(|(held, _)| *held == key) else {
            return KeyboardAction::Ignored(IgnoreReason::NotHeld);
        };
        let (_, action) = self.held.swap_remove(index);
        match action {
            KeyAction::Pad(pad) => KeyboardAction::Release(ReleaseEvent::new(
                pad,
                TriggerSource::Keyboard,
                timestamp_us,
            )),
            KeyAction::Transport(_) => KeyboardAction::Ignored(IgnoreReason::NoRelease),
        }
    }

    /// Drop every held key without emitting timestamps of its own.
    ///
    /// Prefer [`KeyboardInput::release_all`] during a session; this exists for
    /// teardown and device loss where no clock read is meaningful any more.
    pub fn clear(&mut self) {
        self.held.clear();
    }

    /// Release every held pad, e.g. when the window loses focus or the app is
    /// minimized/locked.
    pub fn release_all(&mut self, timestamp_us: u64) -> ReleaseAll {
        let mut released = ReleaseAll {
            releases: Vec::with_capacity(self.held.len()),
            pads: Vec::new(),
        };
        for (_, action) in self.held.drain(..) {
            if let KeyAction::Pad(pad) = action {
                released.pads.push(pad);
                released.releases.push(ReleaseEvent::new(
                    pad,
                    TriggerSource::Keyboard,
                    timestamp_us,
                ));
            }
        }
        released
    }

    /// Aliased for focus-loss handlers: no key may stay held once the window
    /// is inactive, including after a lock screen or alt-tab.
    pub fn focus_lost(&mut self, timestamp_us: u64) -> ReleaseAll {
        self.release_all(timestamp_us)
    }

    /// True when `key` is currently held.
    #[must_use]
    pub fn is_held(&self, key: PhysicalKey) -> bool {
        self.held.iter().any(|(held, _)| *held == key)
    }

    /// Number of held keys (chords included).
    #[must_use]
    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    /// Pads currently held, for diagnostics and tests.
    #[must_use]
    pub fn held_pads(&self) -> Vec<PadId> {
        self.held
            .iter()
            .filter_map(|(_, action)| action.as_pad())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::profile::KeyboardProfile;
    use crate::trigger::PadIdExt;
    use crate::trigger::Velocity;

    fn keyboard() -> KeyboardInput {
        KeyboardInput::default_with_profile(&KeyboardProfile::default_full_keyboard())
    }

    #[test]
    fn pad_press_triggers_and_release_releases() {
        let mut input = keyboard();
        let pressed = input.key_down(PhysicalKey::Digit1, 100);
        let KeyboardAction::Trigger(trigger) = pressed else {
            panic!("expected trigger, got {pressed:?}");
        };
        assert_eq!(trigger.pad, PadId::from_index(0));
        assert_eq!(trigger.source, TriggerSource::Keyboard);
        assert_eq!(trigger.timestamp_us, 100);
        assert_eq!(trigger.velocity, Velocity::DEFAULT);

        let released = input.key_up(PhysicalKey::Digit1, 150);
        let KeyboardAction::Release(release) = released else {
            panic!("expected release, got {released:?}");
        };
        assert_eq!(release.pad, PadId::from_index(0));
        assert_eq!(release.timestamp_us, 150);
        assert_eq!(input.held_count(), 0);
    }

    #[test]
    fn auto_repeat_never_retriggers() {
        let mut input = keyboard();
        assert!(!input.key_down(PhysicalKey::Digit1, 1).is_ignored());
        for repeat in 0..20 {
            assert_eq!(
                input.key_down(PhysicalKey::Digit1, 2 + repeat),
                KeyboardAction::Ignored(IgnoreReason::Repeat)
            );
        }
        assert_eq!(input.held_count(), 1);
        assert!(!input.key_up(PhysicalKey::Digit1, 30).is_ignored());
    }

    #[test]
    fn unmapped_keys_do_nothing() {
        let mut input = keyboard();
        for key in PhysicalKey::UNMAPPED_COLUMN {
            assert_eq!(
                input.key_down(key, 1),
                KeyboardAction::Ignored(IgnoreReason::Unmapped)
            );
            assert_eq!(
                input.key_up(key, 2),
                KeyboardAction::Ignored(IgnoreReason::NotHeld)
            );
        }
        assert_eq!(input.held_count(), 0);
    }

    #[test]
    fn transport_actions_fire_once_per_press() {
        let mut input = keyboard();
        assert_eq!(
            input.key_down(PhysicalKey::F1, 1),
            KeyboardAction::Transport(TransportAction::Panic)
        );
        assert_eq!(
            input.key_down(PhysicalKey::F1, 2),
            KeyboardAction::Ignored(IgnoreReason::Repeat)
        );
        assert_eq!(
            input.key_up(PhysicalKey::F1, 3),
            KeyboardAction::Ignored(IgnoreReason::NoRelease)
        );
        assert_eq!(
            input.key_down(PhysicalKey::F5, 4),
            KeyboardAction::Transport(TransportAction::ShowMapOverlay)
        );
    }

    #[test]
    fn chords_hold_independent_pads() {
        let mut input = keyboard();
        for key in [
            PhysicalKey::Digit1,
            PhysicalKey::KeyW,
            PhysicalKey::KeyF,
            PhysicalKey::KeyV,
        ] {
            assert!(!input.key_down(key, 1).is_ignored());
        }
        assert_eq!(
            input.held_pads(),
            vec![
                PadId::from_index(0),
                PadId::from_index(5),
                PadId::from_index(11),
                PadId::from_index(15)
            ]
        );
        assert_eq!(input.held_count(), 4);
        input.key_up(PhysicalKey::KeyW, 2);
        let mut held = input.held_pads();
        held.sort_unstable();
        assert_eq!(
            held,
            vec![
                PadId::from_index(0),
                PadId::from_index(11),
                PadId::from_index(15)
            ]
        );
    }

    #[test]
    fn focus_loss_releases_every_held_pad() {
        let mut input = keyboard();
        input.key_down(PhysicalKey::Digit1, 1);
        input.key_down(PhysicalKey::KeyV, 2);
        input.key_down(PhysicalKey::F2, 3);
        let released = input.focus_lost(10);
        assert_eq!(released.releases.len(), 2);
        assert_eq!(
            released.pads,
            vec![PadId::from_index(0), PadId::from_index(15)]
        );
        assert!(
            released
                .releases
                .iter()
                .all(|event| event.timestamp_us == 10)
        );
        assert_eq!(input.held_count(), 0);

        // The OS will still deliver the eventual key-up; it must be harmless
        // and a fresh press must work (stuck-key recovery).
        assert_eq!(
            input.key_up(PhysicalKey::Digit1, 11),
            KeyboardAction::Ignored(IgnoreReason::NotHeld)
        );
        assert!(!input.key_down(PhysicalKey::Digit1, 12).is_ignored());
    }

    #[test]
    fn release_all_is_empty_when_nothing_is_held() {
        let mut input = keyboard();
        let released = input.release_all(5);
        assert!(released.is_empty());
        assert!(released.pads.is_empty());
    }

    #[test]
    fn rebinding_mid_hold_releases_old_pad() {
        let mut input = keyboard();
        input.key_down(PhysicalKey::Digit1, 1);
        let mut profile = KeyboardProfile::default_full_keyboard();
        profile.set_binding(PhysicalKey::Digit1, KeyAction::pad(PadId::from_index(9)));
        let map = KeyMap::new(&profile).unwrap();
        let released = input.replace_map(map, 2);
        assert_eq!(released.pads, vec![PadId::from_index(0)]);
        let action = input.key_down(PhysicalKey::Digit1, 3);
        assert_eq!(action.pad(), Some(PadId::from_index(9)));
    }

    #[test]
    fn all_sixteen_pads_hold_simultaneously() {
        let mut input = keyboard();
        for key in PhysicalKey::PAD_BLOCK {
            assert!(
                !input.key_down(key, 1).is_ignored(),
                "{key} was dropped from a 16-key chord"
            );
        }
        assert_eq!(input.held_count(), 16);
        let mut pads = input.held_pads();
        pads.sort_unstable();
        assert_eq!(pads, PadId::ALL.to_vec());
        for key in PhysicalKey::PAD_BLOCK {
            assert!(!input.key_up(key, 2).is_ignored(), "{key} did not release");
        }
        assert_eq!(input.held_count(), 0);
    }

    #[test]
    fn fast_repeated_taps_are_never_lost() {
        let mut input = keyboard();
        for tap in 0..10_000 {
            let action = input.key_down(PhysicalKey::Digit1, tap * 2);
            assert!(!action.is_ignored(), "tap {tap} was suppressed");
            let release = input.key_up(PhysicalKey::Digit1, tap * 2 + 1);
            assert!(!release.is_ignored(), "release {tap} was lost");
        }
        assert_eq!(input.held_count(), 0);
    }

    #[test]
    fn velocity_configuration_applies_to_new_triggers() {
        let mut input = keyboard();
        input.set_velocity_config(KeyboardVelocityConfig::new(
            crate::keyboard::velocity::VelocityMode::Fixed(Velocity::new(42).unwrap()),
        ));
        let KeyboardAction::Trigger(trigger) = input.key_down(PhysicalKey::Digit2, 1) else {
            panic!("expected trigger");
        };
        assert_eq!(trigger.velocity.get(), 42);
    }

    #[test]
    fn action_helpers_expose_events_and_transport() {
        let mut input = keyboard();
        let action = input.key_down(PhysicalKey::Digit1, 1);
        assert!(action.event().is_some());
        assert_eq!(action.transport(), None);
        assert_eq!(action.pad(), Some(PadId::from_index(0)));
        assert!(!action.is_ignored());

        let transport = input.key_down(PhysicalKey::F4, 2);
        assert_eq!(transport.transport(), Some(TransportAction::NextKit));
        assert!(transport.event().is_none());
    }
}
