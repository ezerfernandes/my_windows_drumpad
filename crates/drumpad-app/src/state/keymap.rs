//! Keyboard profiles, physical-key routing, and the key chart used by the map
//! overlay.
//!
//! Keys are matched on [`KeyCode`] (a *physical* key), never on the produced
//! character, so the same profile works on a US laptop, a German layout, and
//! the Finding 35-key one-hand keypad.
//!
//! Routing happens on the winit event, not on the next egui frame: the shell
//! calls [`KeyRouter::route`] from `WindowEvent::KeyboardInput` before egui sees
//! the event. That keeps key-to-queue dispatch off the render cadence.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

use super::pad::{PAD_COUNT, PadId};
use super::prefs::GridSize;

/// Transport actions available on F1-F5 in the Finding layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransportAction {
    /// Stop every voice with a short fade.
    Panic,
    /// Toggle the metronome.
    Metronome,
    /// Load the previous kit in the set order.
    PreviousKit,
    /// Load the next kit in the set order.
    NextKit,
    /// Show or hide the keyboard map overlay.
    MapOverlay,
}

impl TransportAction {
    /// All five actions in F1-F5 order.
    pub const ALL: [Self; 5] = [
        Self::Panic,
        Self::Metronome,
        Self::PreviousKit,
        Self::NextKit,
        Self::MapOverlay,
    ];

    /// Short label for buttons and the map overlay.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Panic => "Panic",
            Self::Metronome => "Metronome",
            Self::PreviousKit => "Previous kit",
            Self::NextKit => "Next kit",
            Self::MapOverlay => "Key map",
        }
    }

    /// One-line help for the transport row tooltip.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Panic => "Stop all voices with a short fade (F1)",
            Self::Metronome => "Toggle the metronome (F2)",
            Self::PreviousKit => "Load the previous kit (F3)",
            Self::NextKit => "Load the next kit (F4)",
            Self::MapOverlay => "Show the keyboard map overlay (F5)",
        }
    }
}

/// What a physical key does under the active profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    /// Trigger a pad on key down.
    Pad(PadId),
    /// Release a pad on key up, for gate-mode voices.
    PadRelease(PadId),
    /// Run a transport action.
    Transport(TransportAction),
    /// The key does nothing. `true` when the profile documents the key as
    /// deliberately unmapped (5/T/G/B/Space on the Finding keypad).
    Unmapped(bool),
}

/// One key press or release, already extracted from the windowing event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    /// Physical key.
    pub key: KeyCode,
    /// Whether the key went down or came up.
    pub pressed: bool,
    /// OS auto-repeat. Repeated presses are always suppressed.
    pub repeat: bool,
}

impl KeyInput {
    /// A non-repeat key press.
    pub const fn press(key: KeyCode) -> Self {
        Self {
            key,
            pressed: true,
            repeat: false,
        }
    }

    /// A key release.
    pub const fn release(key: KeyCode) -> Self {
        Self {
            key,
            pressed: false,
            repeat: false,
        }
    }
}

/// Category of a keyboard profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// A normal full-size keyboard; the 4x4 block sits under the left hand.
    FullKeyboard,
    /// The reference Finding 35-key single-hand USB keypad (ASIN B09HF5WDR2).
    FindingKeypad,
}

impl ProfileKind {
    /// Human label used in settings and the wizard.
    pub const fn label(self) -> &'static str {
        match self {
            Self::FullKeyboard => "Full keyboard",
            Self::FindingKeypad => "Finding 35-key one-hand keypad",
        }
    }
}

/// A stable identifier for a saved profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(pub u32);

/// A complete pad-to-key mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardProfile {
    /// Stable identifier used by settings and kit files.
    pub id: ProfileId,
    /// Display name.
    pub name: String,
    /// Profile category.
    pub kind: ProfileKind,
    /// One key per pad, row-major.
    pub pad_keys: [KeyCode; PAD_COUNT],
    /// F1-F5 order: panic, metronome, previous kit, next kit, map overlay.
    pub transport_keys: [KeyCode; 5],
    /// Keys that are deliberately left unmapped on this device.
    pub unmapped_keys: Vec<KeyCode>,
    /// Pad surface size this profile routes.
    #[serde(default)]
    pub grid: GridSize,
}

/// The 16 pad keys shared by both reference profiles: `1 2 3 4`, `Q W E R`,
/// `A S D F`, `Z X C V`.
pub const GRID_KEYS: [KeyCode; PAD_COUNT] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::KeyQ,
    KeyCode::KeyW,
    KeyCode::KeyE,
    KeyCode::KeyR,
    KeyCode::KeyA,
    KeyCode::KeyS,
    KeyCode::KeyD,
    KeyCode::KeyF,
    KeyCode::KeyZ,
    KeyCode::KeyX,
    KeyCode::KeyC,
    KeyCode::KeyV,
];

/// The nine 3x3 pad keys: `1 2 3`, `Q W E`, `A S D` — the top-left 3x3 of the
/// 4x4 block — row-major.
pub const GRID_KEYS_3X3: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::KeyQ,
    KeyCode::KeyW,
    KeyCode::KeyE,
    KeyCode::KeyA,
    KeyCode::KeyS,
    KeyCode::KeyD,
];

/// Transport keys for the reference profiles.
pub const TRANSPORT_KEYS: [KeyCode; 5] = [
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
];

/// Keys that stay unmapped on the Finding keypad so an accidental hit cannot
/// trigger a pad or a transport action.
pub const FINDING_UNMAPPED: [KeyCode; 5] = [
    KeyCode::Digit5,
    KeyCode::KeyT,
    KeyCode::KeyG,
    KeyCode::KeyB,
    KeyCode::Space,
];

impl KeyboardProfile {
    /// The default full-keyboard profile.
    pub fn full_keyboard() -> Self {
        Self {
            id: ProfileId(1),
            name: "Full keyboard".to_owned(),
            kind: ProfileKind::FullKeyboard,
            pad_keys: GRID_KEYS,
            transport_keys: TRANSPORT_KEYS,
            unmapped_keys: Vec::new(),
            grid: GridSize::FourByFour,
        }
    }

    /// The reference Finding 35-key one-hand keypad profile.
    pub fn finding_keypad() -> Self {
        Self {
            id: ProfileId(2),
            name: "Finding 35-key keypad".to_owned(),
            kind: ProfileKind::FindingKeypad,
            pad_keys: GRID_KEYS,
            transport_keys: TRANSPORT_KEYS,
            unmapped_keys: FINDING_UNMAPPED.to_vec(),
            grid: GridSize::FourByFour,
        }
    }

    /// Resize the pad surface to `grid`, rewriting the pad keys for that
    /// surface and keeping the profile identity, transport keys, and device kind.
    pub fn with_grid(mut self, grid: GridSize) -> Self {
        self.grid = grid;
        match grid {
            GridSize::FourByFour => {
                self.pad_keys = GRID_KEYS;
            }
            GridSize::ThreeByThree => {
                self.pad_keys[..9].copy_from_slice(&GRID_KEYS_3X3);
            }
        }
        self
    }

    /// Number of pads this profile routes.
    pub fn pad_count(&self) -> usize {
        self.grid.pad_count()
    }

    /// Every built-in profile.
    pub fn builtins() -> Vec<Self> {
        vec![Self::full_keyboard(), Self::finding_keypad()]
    }

    /// Look up a built-in profile by id.
    pub fn builtin(id: ProfileId) -> Option<Self> {
        Self::builtins()
            .into_iter()
            .find(|profile| profile.id == id)
    }

    /// Resolve the action for `key` on key down.
    pub fn action_for(&self, key: KeyCode) -> KeyAction {
        if let Some(index) = self.pad_keys[..self.pad_count()]
            .iter()
            .position(|candidate| *candidate == key)
        {
            return KeyAction::Pad(PadId::new(index));
        }
        if let Some(index) = self
            .transport_keys
            .iter()
            .position(|candidate| *candidate == key)
        {
            return KeyAction::Transport(TransportAction::ALL[index]);
        }
        KeyAction::Unmapped(self.unmapped_keys.contains(&key))
    }

    /// Pad label for the key bound to `id`, for example `Q`.
    pub fn pad_label(&self, id: PadId) -> String {
        key_label(self.pad_keys[id.index()])
    }

    /// The pad bound to `key`, if any.
    pub fn pad_for(&self, key: KeyCode) -> Option<PadId> {
        self.pad_keys[..self.pad_count()]
            .iter()
            .position(|candidate| *candidate == key)
            .map(PadId::new)
    }

    /// Labels for all 16 pads; only the first `pad_count()` are on the surface.
    pub fn pad_labels(&self) -> [String; PAD_COUNT] {
        std::array::from_fn(|index| key_label(self.pad_keys[index]))
    }

    /// Replace the binding for one pad, rejecting a key already used by another
    /// pad or by a transport action.
    pub fn rebind_pad(&mut self, id: PadId, key: KeyCode) -> Result<(), BindingConflict> {
        if id.index() >= self.pad_count() {
            return Err(BindingConflict::OutsideSurface { pad: id });
        }
        if let Some(other) = self.pad_for(key)
            && other != id
        {
            return Err(BindingConflict::Pad { key, other });
        }
        if let Some(action) = self
            .transport_keys
            .iter()
            .position(|candidate| *candidate == key)
        {
            return Err(BindingConflict::Transport {
                key,
                action: TransportAction::ALL[action],
            });
        }
        self.pad_keys[id.index()] = key;
        self.unmapped_keys.retain(|candidate| *candidate != key);
        Ok(())
    }

    /// Reset every pad binding to the reference grid, keeping the profile id.
    pub fn reset_bindings(&mut self) {
        let grid = self.grid;
        let mut refreshed = self.clone().with_grid(grid);
        refreshed.transport_keys = TRANSPORT_KEYS;
        *self = refreshed;
    }

    /// Sections shown by the keyboard map overlay.
    pub fn chart(&self) -> Vec<ChartSection> {
        let columns = self.grid.columns();
        let rows = self.grid.rows();
        let mut pad_rows = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut keys = Vec::with_capacity(columns);
            for column in 0..columns {
                let index = row * columns + column;
                let key = self.pad_keys[index];
                keys.push(ChartKey {
                    key,
                    label: key_label(key),
                    action: KeyAction::Pad(PadId::new(index)),
                    pad: Some(PadId::new(index)),
                });
            }
            pad_rows.push(ChartRow { keys });
        }

        let transport = ChartRow {
            keys: self
                .transport_keys
                .iter()
                .zip(TransportAction::ALL)
                .map(|(key, action)| ChartKey {
                    key: *key,
                    label: key_label(*key),
                    action: KeyAction::Transport(action),
                    pad: None,
                })
                .collect(),
        };

        let mut sections = vec![
            ChartSection {
                title: "Pads".to_owned(),
                rows: pad_rows,
            },
            ChartSection {
                title: "Transport".to_owned(),
                rows: vec![transport],
            },
        ];

        if !self.unmapped_keys.is_empty() {
            sections.push(ChartSection {
                title: "Unmapped (safe to hit)".to_owned(),
                rows: vec![ChartRow {
                    keys: self
                        .unmapped_keys
                        .iter()
                        .map(|key| ChartKey {
                            key: *key,
                            label: key_label(*key),
                            action: KeyAction::Unmapped(true),
                            pad: None,
                        })
                        .collect(),
                }],
            });
        }

        sections
    }
}

/// A rebinding clash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingConflict {
    /// The key already triggers another pad.
    Pad {
        /// The key that clashes.
        key: KeyCode,
        /// The pad that already owns it.
        other: PadId,
    },
    /// The key is a transport key.
    Transport {
        /// The key that clashes.
        key: KeyCode,
        /// The transport action that owns it.
        action: TransportAction,
    },
    /// The pad is not on the active surface.
    OutsideSurface {
        /// The pad that is not visible.
        pad: PadId,
    },
}

impl fmt::Display for BindingConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pad { key, other } => write!(
                formatter,
                "{} already triggers pad {}",
                key_label(*key),
                other.number()
            ),
            Self::Transport { key, action } => write!(
                formatter,
                "{} is reserved for {}",
                key_label(*key),
                action.label()
            ),
            Self::OutsideSurface { pad } => write!(
                formatter,
                "pad {} is outside the active pad surface",
                pad.number()
            ),
        }
    }
}

/// One row of the key chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRow {
    /// Keys left to right.
    pub keys: Vec<ChartKey>,
}

/// One key in the map overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartKey {
    /// Physical key.
    pub key: KeyCode,
    /// Short label, for example `Q`.
    pub label: String,
    /// What the key does.
    pub action: KeyAction,
    /// Pad the key triggers, when any.
    pub pad: Option<PadId>,
}

/// A titled block of the key chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartSection {
    /// Section heading.
    pub title: String,
    /// Rows of keys.
    pub rows: Vec<ChartRow>,
}

/// Routes physical keys to pad and transport actions.
///
/// Auto-repeat is suppressed and the set of held keys is tracked so the map
/// overlay can highlight what is physically down, and so gate-mode release
/// events are paired exactly once with their press.
#[derive(Debug, Clone)]
pub struct KeyRouter {
    profile: KeyboardProfile,
    held: HashSet<KeyCode>,
    /// Keys whose release happened while the window was unfocused or that were
    /// released after the profile changed. Cleared on focus.
    stale: HashSet<KeyCode>,
}

impl KeyRouter {
    /// A router for one profile.
    pub fn new(profile: KeyboardProfile) -> Self {
        Self {
            profile,
            held: HashSet::new(),
            stale: HashSet::new(),
        }
    }

    /// The active profile.
    pub fn profile(&self) -> &KeyboardProfile {
        &self.profile
    }

    /// Swap the active profile. Held keys are marked stale so no release event
    /// can reach a pad the user never pressed under the new map.
    pub fn set_profile(&mut self, profile: KeyboardProfile) {
        self.stale.extend(self.held.drain());
        self.profile = profile;
    }

    /// Resize the surface, rewriting the pad keys and marking held keys stale.
    pub fn set_grid(&mut self, grid: GridSize) {
        let profile = self.profile.clone().with_grid(grid);
        self.set_profile(profile);
    }

    /// Route one key event. Returns `None` for auto-repeat, an unknown key, a
    /// stale release, or one of the deliberately unmapped keys.
    pub fn route(&mut self, input: KeyInput) -> Option<KeyAction> {
        if input.repeat {
            return None;
        }

        if input.pressed {
            if !self.held.insert(input.key) {
                return None;
            }
            self.stale.remove(&input.key);
            match self.profile.action_for(input.key) {
                KeyAction::Unmapped(_) => None,
                action => Some(action),
            }
        } else {
            let was_held = self.held.remove(&input.key);
            if !was_held && !self.stale.remove(&input.key) {
                return None;
            }
            self.profile.pad_for(input.key).map(KeyAction::PadRelease)
        }
    }

    /// Whether a physical key is currently down.
    pub fn is_down(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }

    /// All keys currently down, for the overlay highlight.
    pub fn held(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.held.iter().copied()
    }

    /// Forget held keys (window focus loss). No release events are emitted, so
    /// the caller must stop all voices separately, which the shell does.
    pub fn clear(&mut self) {
        self.stale.extend(self.held.drain());
    }

    /// Keys that never produce an action in this profile.
    pub fn unmapped(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.profile.unmapped_keys.iter().copied()
    }
}

/// Short display label for a physical key.
pub fn key_label(key: KeyCode) -> String {
    match key {
        KeyCode::Digit0 => "0".to_owned(),
        KeyCode::Digit1 => "1".to_owned(),
        KeyCode::Digit2 => "2".to_owned(),
        KeyCode::Digit3 => "3".to_owned(),
        KeyCode::Digit4 => "4".to_owned(),
        KeyCode::Digit5 => "5".to_owned(),
        KeyCode::Digit6 => "6".to_owned(),
        KeyCode::Digit7 => "7".to_owned(),
        KeyCode::Digit8 => "8".to_owned(),
        KeyCode::Digit9 => "9".to_owned(),
        KeyCode::Space => "Space".to_owned(),
        KeyCode::Escape => "Esc".to_owned(),
        KeyCode::Enter => "Enter".to_owned(),
        KeyCode::Tab => "Tab".to_owned(),
        KeyCode::Backspace => "Backspace".to_owned(),
        KeyCode::ArrowUp => "Up".to_owned(),
        KeyCode::ArrowDown => "Down".to_owned(),
        KeyCode::ArrowLeft => "Left".to_owned(),
        KeyCode::ArrowRight => "Right".to_owned(),
        other => {
            // `KeyCode` has no `Display`, so use its stable `Debug` name and
            // shorten the common prefixes into keyboard-cap labels.
            let name = format!("{other:?}");
            match name.strip_prefix("Key") {
                Some(letter) => letter.to_owned(),
                None => name,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding() -> KeyboardProfile {
        KeyboardProfile::finding_keypad()
    }

    #[test]
    fn the_finding_layout_maps_pads_and_transport() {
        let profile = finding();
        assert_eq!(
            profile.action_for(KeyCode::Digit1),
            KeyAction::Pad(PadId::new(0))
        );
        assert_eq!(
            profile.action_for(KeyCode::KeyV),
            KeyAction::Pad(PadId::new(15))
        );
        assert_eq!(
            profile.action_for(KeyCode::F1),
            KeyAction::Transport(TransportAction::Panic)
        );
        assert_eq!(
            profile.action_for(KeyCode::F5),
            KeyAction::Transport(TransportAction::MapOverlay)
        );
    }

    #[test]
    fn the_five_unmapped_finding_keys_stay_unmapped() {
        let profile = finding();
        for key in FINDING_UNMAPPED {
            assert_eq!(
                profile.action_for(key),
                KeyAction::Unmapped(true),
                "{key:?}"
            );
        }
        assert_eq!(
            profile.action_for(KeyCode::KeyP),
            KeyAction::Unmapped(false),
            "unknown keys are unmapped too, just not documented"
        );
    }

    #[test]
    fn key_labels_are_short_and_readable() {
        assert_eq!(key_label(KeyCode::Digit1), "1");
        assert_eq!(key_label(KeyCode::KeyQ), "Q");
        assert_eq!(key_label(KeyCode::F3), "F3");
        assert_eq!(key_label(KeyCode::Space), "Space");
    }

    #[test]
    fn router_emits_one_action_per_press_and_suppresses_repeat() {
        let mut router = KeyRouter::new(finding());
        assert_eq!(
            router.route(KeyInput::press(KeyCode::KeyQ)),
            Some(KeyAction::Pad(PadId::new(4)))
        );
        assert_eq!(
            router.route(KeyInput {
                key: KeyCode::KeyQ,
                pressed: true,
                repeat: true,
            }),
            None,
            "auto-repeat must not retrigger"
        );
        assert_eq!(
            router.route(KeyInput::press(KeyCode::KeyQ)),
            None,
            "a duplicate press without a release must not retrigger"
        );
        assert_eq!(
            router.route(KeyInput::release(KeyCode::KeyQ)),
            Some(KeyAction::PadRelease(PadId::new(4)))
        );
        assert_eq!(
            router.route(KeyInput::release(KeyCode::KeyQ)),
            None,
            "a release must be paired exactly once"
        );
    }

    #[test]
    fn router_ignores_unmapped_keys_but_still_tracks_them() {
        let mut router = KeyRouter::new(finding());
        assert_eq!(router.route(KeyInput::press(KeyCode::KeyT)), None);
        assert!(router.is_down(KeyCode::KeyT));
        assert_eq!(router.route(KeyInput::release(KeyCode::KeyT)), None);
    }

    #[test]
    fn transport_keys_route_without_a_pad_release() {
        let mut router = KeyRouter::new(finding());
        assert_eq!(
            router.route(KeyInput::press(KeyCode::F4)),
            Some(KeyAction::Transport(TransportAction::NextKit))
        );
        assert_eq!(router.route(KeyInput::release(KeyCode::F4)), None);
    }

    #[test]
    fn switching_profiles_cannot_fire_a_release_for_a_pad_the_user_never_pressed() {
        let mut router = KeyRouter::new(finding());
        router.route(KeyInput::press(KeyCode::KeyQ));

        // Build a profile where Q belongs to pad 10 instead of pad 5.
        let mut other = finding();
        other
            .rebind_pad(PadId::new(4), KeyCode::KeyP)
            .expect("move pad 5 off Q");
        other
            .rebind_pad(PadId::new(9), KeyCode::KeyQ)
            .expect("rebind");
        router.set_profile(other);
        assert_eq!(
            router.route(KeyInput::release(KeyCode::KeyQ)),
            Some(KeyAction::PadRelease(PadId::new(9))),
            "the release is paired with the new profile's pad, not a phantom press"
        );
    }

    #[test]
    fn focus_loss_clears_held_keys() {
        let mut router = KeyRouter::new(finding());
        router.route(KeyInput::press(KeyCode::KeyA));
        assert!(router.is_down(KeyCode::KeyA));
        router.clear();
        assert!(!router.is_down(KeyCode::KeyA));
        assert_eq!(
            router.route(KeyInput::release(KeyCode::KeyA)),
            Some(KeyAction::PadRelease(PadId::new(8)))
        );
    }

    #[test]
    fn rebinding_rejects_a_key_that_already_triggers_a_pad() {
        let mut profile = finding();
        let error = profile
            .rebind_pad(PadId::new(0), KeyCode::KeyV)
            .expect_err("conflict");
        assert_eq!(
            error,
            BindingConflict::Pad {
                key: KeyCode::KeyV,
                other: PadId::new(15)
            }
        );
        assert!(error.to_string().contains("pad 16"));
    }

    #[test]
    fn rebinding_rejects_transport_keys() {
        let mut profile = finding();
        let error = profile
            .rebind_pad(PadId::new(0), KeyCode::F2)
            .expect_err("conflict");
        assert_eq!(
            error,
            BindingConflict::Transport {
                key: KeyCode::F2,
                action: TransportAction::Metronome
            }
        );
    }

    #[test]
    fn rebinding_a_key_removes_it_from_the_unmapped_list() {
        let mut profile = finding();
        profile
            .rebind_pad(PadId::new(0), KeyCode::Digit5)
            .expect("rebind onto an unmapped key");
        assert!(!profile.unmapped_keys.contains(&KeyCode::Digit5));
    }

    #[test]
    fn reset_restores_the_reference_grid() {
        let mut profile = finding();
        profile
            .rebind_pad(PadId::new(0), KeyCode::KeyP)
            .expect("rebind");
        profile.reset_bindings();
        assert_eq!(profile.pad_keys, GRID_KEYS);
        assert_eq!(
            profile.action_for(KeyCode::KeyP),
            KeyAction::Unmapped(false)
        );
    }

    #[test]
    fn the_chart_has_pads_transport_and_the_unmapped_row() {
        let profile = finding();
        let chart = profile.chart();
        assert_eq!(chart.len(), 3);
        assert_eq!(chart[0].rows.len(), 4);
        assert_eq!(chart[0].rows[0].keys.len(), 4);
        assert_eq!(chart[1].rows[0].keys.len(), 5);
        assert_eq!(chart[2].rows[0].keys.len(), 5);
    }

    #[test]
    fn the_3x3_profile_maps_the_top_left_block_to_pads_one_through_nine() {
        let profile = finding().with_grid(GridSize::ThreeByThree);
        assert_eq!(profile.pad_count(), 9);
        for (index, key) in GRID_KEYS_3X3.iter().enumerate() {
            assert_eq!(
                profile.action_for(*key),
                KeyAction::Pad(PadId::new(index)),
                "{key:?} should map to pad {}",
                index + 1
            );
        }
        assert_eq!(
            profile.action_for(KeyCode::Digit4),
            KeyAction::Unmapped(false)
        );
        assert_eq!(
            profile.action_for(KeyCode::F1),
            KeyAction::Transport(TransportAction::Panic)
        );
    }

    #[test]
    fn the_4x4_profile_still_maps_all_sixteen_pads() {
        let profile = finding();
        assert_eq!(profile.pad_count(), 16);
        for (index, key) in GRID_KEYS.iter().enumerate() {
            assert_eq!(profile.action_for(*key), KeyAction::Pad(PadId::new(index)));
        }
    }

    #[test]
    fn switching_grid_rewrites_the_labels_and_chart() {
        let profile = finding();
        assert_eq!(profile.pad_labels()[0], "1");
        let small = profile.clone().with_grid(GridSize::ThreeByThree);
        assert_eq!(small.pad_labels()[0], "1");
        assert_eq!(small.pad_labels()[8], "D");
        assert_eq!(small.chart()[0].rows.len(), 3);
        assert_eq!(small.chart()[0].rows[0].keys.len(), 3);
        assert_eq!(profile.chart()[0].rows.len(), 4);
    }

    #[test]
    fn a_router_switching_to_3x3_drops_the_rest_of_the_4x4_block() {
        let mut router = KeyRouter::new(finding());
        assert_eq!(
            router.route(KeyInput::press(KeyCode::Digit4)),
            Some(KeyAction::Pad(PadId::new(3)))
        );
        router.set_grid(GridSize::ThreeByThree);
        assert_eq!(router.route(KeyInput::release(KeyCode::Digit4)), None);
        assert_eq!(
            router.route(KeyInput::press(KeyCode::Digit1)),
            Some(KeyAction::Pad(PadId::new(0)))
        );
        assert_eq!(
            router.route(KeyInput::press(KeyCode::KeyQ)),
            Some(KeyAction::Pad(PadId::new(3)))
        );
    }

    #[test]
    fn rebinding_a_pad_outside_the_surface_is_refused() {
        let mut profile = finding().with_grid(GridSize::ThreeByThree);
        let error = profile
            .rebind_pad(PadId::new(10), KeyCode::KeyP)
            .expect_err("outside surface");
        assert_eq!(
            error,
            BindingConflict::OutsideSurface {
                pad: PadId::new(10)
            }
        );
    }
    #[test]
    fn profiles_round_trip_through_json() {
        let profile = finding();
        let json = serde_json::to_string(&profile).expect("serialize");
        let decoded: KeyboardProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, profile);
    }
}
