//! Physical key codes.
//!
//! The input layer never looks at characters. Windows delivers a layout-mapped
//! character for keyboard events, but a drummer on an AZERTY or Dvorak layout
//! still expects the same physical 4x4 block. `drumpad-app` converts
//! `winit::keyboard::KeyCode` (and Windows scan codes) into [`PhysicalKey`] at
//! the edge; everything below this module is layout independent.
//!
//! Keys the app does not know by name arrive as [`PhysicalKey::Other`] carrying
//! the platform scan code, so remapping can still capture them and profiles
//! round-trip them unchanged.

use serde::{Deserialize, Serialize};

/// A physical key on a full keyboard or one-hand keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalKey {
    /// `1` above the letter row.
    Digit1,
    /// `2` above the letter row.
    Digit2,
    /// `3` above the letter row.
    Digit3,
    /// `4` above the letter row.
    Digit4,
    /// `5`, unmapped by default.
    Digit5,
    /// `Q`.
    KeyQ,
    /// `W`.
    KeyW,
    /// `E`.
    KeyE,
    /// `R`.
    KeyR,
    /// `T`, unmapped by default.
    KeyT,
    /// `A`.
    KeyA,
    /// `S`.
    KeyS,
    /// `D`.
    KeyD,
    /// `F`.
    KeyF,
    /// `G`, unmapped by default.
    KeyG,
    /// `Z`.
    KeyZ,
    /// `X`.
    KeyX,
    /// `C`.
    KeyC,
    /// `V`.
    KeyV,
    /// `B`, unmapped by default.
    KeyB,
    /// `F1`, transport: panic / stop all.
    F1,
    /// `F2`, transport: metronome toggle.
    F2,
    /// `F3`, transport: previous kit.
    F3,
    /// `F4`, transport: next kit.
    F4,
    /// `F5`, transport: keyboard map overlay.
    F5,
    /// Space bar, unmapped by default.
    Space,
    /// Any other key, identified by the platform scan code.
    Other(u32),
}

impl PhysicalKey {
    /// The sixteen pad keys in grid order: `1 2 3 4 / Q W E R / A S D F / Z X C V`.
    pub const PAD_BLOCK: [Self; 16] = [
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::KeyQ,
        Self::KeyW,
        Self::KeyE,
        Self::KeyR,
        Self::KeyA,
        Self::KeyS,
        Self::KeyD,
        Self::KeyF,
        Self::KeyZ,
        Self::KeyX,
        Self::KeyC,
        Self::KeyV,
    ];

    /// The fifth column of the keypad, deliberately unmapped.
    pub const UNMAPPED_COLUMN: [Self; 5] = [
        Self::Digit5,
        Self::KeyT,
        Self::KeyG,
        Self::KeyB,
        Self::Space,
    ];

    /// Transport keys in order.
    pub const TRANSPORT_KEYS: [Self; 5] = [Self::F1, Self::F2, Self::F3, Self::F4, Self::F5];

    /// Every named key, in a stable order suitable for a map overlay.
    pub const NAMED: [Self; 26] = [
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::Digit5,
        Self::KeyQ,
        Self::KeyW,
        Self::KeyE,
        Self::KeyR,
        Self::KeyT,
        Self::KeyA,
        Self::KeyS,
        Self::KeyD,
        Self::KeyF,
        Self::KeyG,
        Self::KeyZ,
        Self::KeyX,
        Self::KeyC,
        Self::KeyV,
        Self::KeyB,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::Space,
    ];

    /// Short label shown on a pad or in the map overlay.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::KeyQ => "Q",
            Self::KeyW => "W",
            Self::KeyE => "E",
            Self::KeyR => "R",
            Self::KeyT => "T",
            Self::KeyA => "A",
            Self::KeyS => "S",
            Self::KeyD => "D",
            Self::KeyF => "F",
            Self::KeyG => "G",
            Self::KeyZ => "Z",
            Self::KeyX => "X",
            Self::KeyC => "C",
            Self::KeyV => "V",
            Self::KeyB => "B",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::Space => "Space",
            Self::Other(_) => "Scan",
        }
    }

    /// Human-readable name, including the scan code for [`PhysicalKey::Other`].
    #[must_use]
    pub fn display_name(self) -> String {
        match self {
            Self::Other(code) => format!("Scan {code}"),
            named => named.label().to_owned(),
        }
    }

    /// True for keys in the 4x4 pad block.
    #[must_use]
    pub fn is_pad_key(self) -> bool {
        Self::PAD_BLOCK.contains(&self)
    }

    /// True for keys in the fifth column, which stays unmapped by default.
    #[must_use]
    pub fn is_fifth_column(self) -> bool {
        Self::UNMAPPED_COLUMN.contains(&self)
    }

    /// Canonical model key (raw PC scan code) for kits and portable profiles.
    ///
    /// The typed enum is the input-side view; `drumpad_model::PhysicalKey` is
    /// the durable wire format. Unknown keys pass their raw code through.
    #[must_use]
    pub const fn to_model(self) -> drumpad_model::PhysicalKey {
        match self {
            Self::Digit1 => drumpad_model::PhysicalKey::from_scancode(0x02),
            Self::Digit2 => drumpad_model::PhysicalKey::from_scancode(0x03),
            Self::Digit3 => drumpad_model::PhysicalKey::from_scancode(0x04),
            Self::Digit4 => drumpad_model::PhysicalKey::from_scancode(0x05),
            Self::Digit5 => drumpad_model::PhysicalKey::from_scancode(0x06),
            Self::KeyQ => drumpad_model::PhysicalKey::from_scancode(0x10),
            Self::KeyW => drumpad_model::PhysicalKey::from_scancode(0x11),
            Self::KeyE => drumpad_model::PhysicalKey::from_scancode(0x12),
            Self::KeyR => drumpad_model::PhysicalKey::from_scancode(0x13),
            Self::KeyT => drumpad_model::PhysicalKey::from_scancode(0x14),
            Self::KeyA => drumpad_model::PhysicalKey::from_scancode(0x1E),
            Self::KeyS => drumpad_model::PhysicalKey::from_scancode(0x1F),
            Self::KeyD => drumpad_model::PhysicalKey::from_scancode(0x20),
            Self::KeyF => drumpad_model::PhysicalKey::from_scancode(0x21),
            Self::KeyG => drumpad_model::PhysicalKey::from_scancode(0x22),
            Self::KeyZ => drumpad_model::PhysicalKey::from_scancode(0x2C),
            Self::KeyX => drumpad_model::PhysicalKey::from_scancode(0x2D),
            Self::KeyC => drumpad_model::PhysicalKey::from_scancode(0x2E),
            Self::KeyV => drumpad_model::PhysicalKey::from_scancode(0x2F),
            Self::KeyB => drumpad_model::PhysicalKey::from_scancode(0x30),
            Self::Space => drumpad_model::PhysicalKey::from_scancode(0x39),
            Self::F1 => drumpad_model::PhysicalKey::from_scancode(0x3B),
            Self::F2 => drumpad_model::PhysicalKey::from_scancode(0x3C),
            Self::F3 => drumpad_model::PhysicalKey::from_scancode(0x3D),
            Self::F4 => drumpad_model::PhysicalKey::from_scancode(0x3E),
            Self::F5 => drumpad_model::PhysicalKey::from_scancode(0x3F),
            Self::Other(code) => drumpad_model::PhysicalKey::from_raw(code),
        }
    }

    /// Recover the typed key from a model scan code.
    #[must_use]
    pub const fn from_model(key: drumpad_model::PhysicalKey) -> Self {
        match key.raw() {
            0x02 => Self::Digit1,
            0x03 => Self::Digit2,
            0x04 => Self::Digit3,
            0x05 => Self::Digit4,
            0x06 => Self::Digit5,
            0x10 => Self::KeyQ,
            0x11 => Self::KeyW,
            0x12 => Self::KeyE,
            0x13 => Self::KeyR,
            0x14 => Self::KeyT,
            0x1E => Self::KeyA,
            0x1F => Self::KeyS,
            0x20 => Self::KeyD,
            0x21 => Self::KeyF,
            0x22 => Self::KeyG,
            0x2C => Self::KeyZ,
            0x2D => Self::KeyX,
            0x2E => Self::KeyC,
            0x2F => Self::KeyV,
            0x30 => Self::KeyB,
            0x39 => Self::Space,
            0x3B => Self::F1,
            0x3C => Self::F2,
            0x3D => Self::F3,
            0x3E => Self::F4,
            0x3F => Self::F5,
            other => Self::Other(other),
        }
    }
}

impl From<PhysicalKey> for drumpad_model::PhysicalKey {
    fn from(key: PhysicalKey) -> Self {
        key.to_model()
    }
}

impl From<drumpad_model::PhysicalKey> for PhysicalKey {
    fn from(key: drumpad_model::PhysicalKey) -> Self {
        Self::from_model(key)
    }
}

impl core::fmt::Display for PhysicalKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Other(code) => write!(f, "Scan {code}"),
            named => f.write_str(named.label()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_block_matches_the_documented_grid() {
        let labels: Vec<&str> = PhysicalKey::PAD_BLOCK
            .iter()
            .map(|key| key.label())
            .collect();
        assert_eq!(
            labels,
            [
                "1", "2", "3", "4", "Q", "W", "E", "R", "A", "S", "D", "F", "Z", "X", "C", "V"
            ]
        );
    }

    #[test]
    fn fifth_column_and_transport_are_separate() {
        assert_eq!(PhysicalKey::UNMAPPED_COLUMN.len(), 5);
        for key in PhysicalKey::UNMAPPED_COLUMN {
            assert!(!key.is_pad_key(), "{key} must stay out of the pad block");
            assert!(key.is_fifth_column());
        }
        assert_eq!(PhysicalKey::TRANSPORT_KEYS.len(), 5);
        for key in PhysicalKey::TRANSPORT_KEYS {
            assert!(!key.is_pad_key());
            assert!(!key.is_fifth_column());
        }
    }

    #[test]
    fn named_keys_are_unique() {
        let mut names: Vec<&str> = PhysicalKey::NAMED.iter().map(|key| key.label()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PhysicalKey::NAMED.len());
    }

    #[test]
    fn unknown_keys_keep_their_scan_code() {
        let key = PhysicalKey::Other(0x1234);
        assert_eq!(key.display_name(), "Scan 4660");
        assert_eq!(key.to_string(), "Scan 4660");
        assert_ne!(key, PhysicalKey::Other(0x1235));
    }

    #[test]
    fn named_keys_round_trip_through_the_model_type() {
        for key in PhysicalKey::NAMED {
            assert_eq!(PhysicalKey::from_model(key.to_model()), key);
        }
        let other = PhysicalKey::Other(0x1234);
        assert_eq!(PhysicalKey::from_model(other.to_model()), other);
    }

    #[test]
    fn keys_round_trip_through_json() {
        for key in PhysicalKey::NAMED {
            let json = serde_json::to_string(&key).unwrap();
            assert_eq!(serde_json::from_str::<PhysicalKey>(&json).unwrap(), key);
        }
        let other = PhysicalKey::Other(90);
        let json = serde_json::to_string(&other).unwrap();
        assert_eq!(serde_json::from_str::<PhysicalKey>(&json).unwrap(), other);
    }
}
