//! Core kit, pad, layer, and mapping domain types.
//!
//! All identifiers are stable, serializable newtypes. Constructors validate
//! bounds and `Kit::validate` enforces cross-object invariants. Deserialization
//! is deliberately permissive about unknown fields (so a newer minor build can
//! add optional fields without breaking older readers) but strict about the
//! format version, which is checked before the document is interpreted.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Number of playable pads in a v1 kit.
pub const PAD_COUNT: usize = 16;
/// Upper bound on velocity layers per pad.
pub const MAX_LAYERS_PER_PAD: usize = 32;
/// Largest valid choke-group index.
pub const MAX_CHOKE_GROUP: u8 = 15;
/// Current on-disk kit format version.
pub const CURRENT_FORMAT_VERSION: u32 = 1;
/// Legacy pre-release version understood by the migration chain.
pub const LEGACY_FORMAT_VERSION: u32 = 0;

/// Domain-level construction errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    /// A numeric value fell outside its permitted range.
    #[error("{field} value {value} is outside {min}..={max}")]
    OutOfRange {
        /// Field that failed.
        field: &'static str,
        /// Offending value.
        value: i64,
        /// Inclusive minimum.
        min: i64,
        /// Inclusive maximum.
        max: i64,
    },
    /// A required text field was empty.
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    /// A text field exceeded its budget.
    #[error("{field} is longer than {max} characters")]
    TooLong {
        /// Field that failed.
        field: &'static str,
        /// Maximum length in characters.
        max: usize,
    },
    /// A sample path was not a safe, relative path.
    #[error("invalid sample path: {0}")]
    InvalidSamplePath(String),
    /// A content hash was not 64 hexadecimal characters.
    #[error("invalid SHA-256 content hash: {0}")]
    InvalidHash(String),
    /// A stable identifier could not be parsed.
    #[error("invalid {field}: {value}")]
    InvalidId {
        /// Identifier kind.
        field: &'static str,
        /// Offending text.
        value: String,
    },
}

macro_rules! bounded_u8_newtype {
    ($name:ident, $max:expr, $label:literal) => {
        #[doc = concat!("Validated `", $label, "` identifier.")]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(u8);

        impl $name {
            #[doc = concat!("Largest allowed ", $label, " value.")]
            pub const MAX: u8 = $max;

            /// Construct after checking the upper bound.
            pub fn new(value: u8) -> Result<Self, DomainError> {
                if value <= $max {
                    Ok(Self(value))
                } else {
                    Err(DomainError::OutOfRange {
                        field: $label,
                        value: i64::from(value),
                        min: 0,
                        max: i64::from($max),
                    })
                }
            }

            /// Construct from a value the caller already validated.
            pub const fn from_raw_unchecked(value: u8) -> Self {
                debug_assert!(value <= $max);
                Self(value)
            }

            /// Raw value.
            pub const fn get(self) -> u8 {
                self.0
            }

            /// Zero-based index suitable for array access.
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self(0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_u8(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = u8::deserialize(deserializer)?;
                Self::new(raw).map_err(D::Error::custom)
            }
        }
    };
}

pub use crate::ids::PadId;

bounded_u8_newtype!(ChokeGroup, MAX_CHOKE_GROUP, "choke group");
bounded_u8_newtype!(MidiNote, 127, "MIDI note");

/// A kit identifier that survives renames and moves.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct KitId(uuid::Uuid);

impl KitId {
    /// Generate a fresh random identifier.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Parse a canonical UUID string.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        uuid::Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| DomainError::InvalidId {
                field: "kit id",
                value: value.to_owned(),
            })
    }

    /// Canonical text form.
    pub fn as_string(self) -> String {
        self.0.to_string()
    }
}

impl Default for KitId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for KitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for KitId {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for KitId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for KitId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(D::Error::custom)
    }
}

/// Stable layer identifier, unique within a pad.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerId(pub u32);

impl LayerId {
    /// First convenient layer identifier.
    pub const FIRST: Self = Self(1);

    /// Construct.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
}

impl Default for LayerId {
    fn default() -> Self {
        Self::FIRST
    }
}

impl fmt::Display for LayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One-based MIDI channel (1..=16).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MidiChannel(u8);

impl MidiChannel {
    /// Construct after validating the 1..=16 range.
    pub fn new(value: u8) -> Result<Self, DomainError> {
        if (1..=16).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DomainError::OutOfRange {
                field: "MIDI channel",
                value: i64::from(value),
                min: 1,
                max: 16,
            })
        }
    }

    /// Raw one-based channel number.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Construct from a value the caller already validated.
    pub const fn from_raw_unchecked(value: u8) -> Self {
        debug_assert!(value >= 1 && value <= 16);
        Self(value)
    }

    /// Zero-based index.
    pub const fn index(self) -> usize {
        (self.0 - 1) as usize
    }
}

impl Default for MidiChannel {
    fn default() -> Self {
        Self(1)
    }
}

impl Serialize for MidiChannel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for MidiChannel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u8::deserialize(deserializer)?;
        Self::new(raw).map_err(D::Error::custom)
    }
}

/// SHA-256 digest of canonical decoded PCM content.
///
/// The digest is stable across containers and encoders because it is computed
/// from the decoded samples, not the file bytes:
///
/// ```text
/// SHA256("drumpad-pcm-v1" || sample_rate_le_u32 || channels_le_u16 ||
///        frames_le_u64 || interleaved_f32_le)
/// ```
///
/// Raw file hashes (for bundle collision handling) are available through
/// [`ContentHash::of_bytes`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Wrap an existing digest.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Raw digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Compute the canonical digest of interleaved decoded PCM.
    ///
    /// `samples.len()` must be a multiple of `channels`; extra bytes from an
    /// impossible channel count are ignored by saturating the frame count.
    pub fn of_pcm(sample_rate: u32, channels: u16, samples: &[f32]) -> Self {
        let channels = channels.max(1);
        let frames = (samples.len() / usize::from(channels)) as u64;
        let mut hasher = Sha256::new();
        hasher.update(b"drumpad-pcm-v1");
        hasher.update(sample_rate.to_le_bytes());
        hasher.update(channels.to_le_bytes());
        hasher.update(frames.to_le_bytes());
        for sample in samples {
            hasher.update(sample.to_le_bytes());
        }
        Self(hasher.finalize().into())
    }

    /// Compute a SHA-256 digest over raw bytes.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// Compute a SHA-256 digest over a file's raw bytes.
    pub fn of_file(path: &Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self::of_bytes(&bytes))
    }

    /// Lower-case hexadecimal form (64 characters).
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// Parse a 64-character hexadecimal digest.
    pub fn from_hex(value: &str) -> Result<Self, DomainError> {
        if value.len() != 64 {
            return Err(DomainError::InvalidHash(value.to_owned()));
        }
        let bytes = hex::decode(value).map_err(|_| DomainError::InvalidHash(value.to_owned()))?;
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| DomainError::InvalidHash(value.to_owned()))?;
        Ok(Self(array))
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_hex(&raw).map_err(D::Error::custom)
    }
}

/// A portable, kit-relative path to a sample file.
///
/// Only normal path components are accepted. Absolute paths, drive prefixes,
/// parent traversal, NUL bytes, and Windows-reserved device names are rejected
/// so a kit cannot escape its folder or address the host filesystem.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RelativeSamplePath(String);

impl RelativeSamplePath {
    /// Validate and normalize a path (`\` separators become `/`).
    pub fn new(path: impl Into<String>) -> Result<Self, DomainError> {
        let raw = path.into();
        if raw.is_empty() {
            return Err(DomainError::InvalidSamplePath("path is empty".into()));
        }
        if raw.contains('\0') {
            return Err(DomainError::InvalidSamplePath("path contains NUL".into()));
        }
        let normalized = raw.replace('\\', "/");
        if normalized.starts_with('/') {
            return Err(DomainError::InvalidSamplePath(
                "path must be relative to the kit folder".into(),
            ));
        }
        let candidate = Path::new(&normalized);
        if candidate.is_absolute() {
            return Err(DomainError::InvalidSamplePath(
                "path must be relative to the kit folder".into(),
            ));
        }
        let mut parts = Vec::new();
        for component in candidate.components() {
            match component {
                Component::Normal(part) => {
                    let text = part.to_string_lossy();
                    if text.is_empty() {
                        return Err(DomainError::InvalidSamplePath(
                            "path contains an empty component".into(),
                        ));
                    }
                    if is_windows_reserved(&text) {
                        return Err(DomainError::InvalidSamplePath(format!(
                            "`{text}` is a reserved Windows device name"
                        )));
                    }
                    parts.push(text.to_string());
                }
                Component::CurDir => {
                    return Err(DomainError::InvalidSamplePath(
                        "path must not contain `.` components".into(),
                    ));
                }
                Component::ParentDir => {
                    return Err(DomainError::InvalidSamplePath(
                        "path must not contain `..` components".into(),
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(DomainError::InvalidSamplePath(
                        "path must be relative to the kit folder".into(),
                    ));
                }
            }
        }
        if parts.is_empty() {
            return Err(DomainError::InvalidSamplePath("path is empty".into()));
        }
        Ok(Self(parts.join("/")))
    }

    /// Normalized, slash-separated relative path.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve against a kit folder.
    pub fn resolve(&self, kit_dir: &Path) -> PathBuf {
        kit_dir.join(&self.0)
    }
}

impl fmt::Display for RelativeSamplePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_windows_reserved(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
}

impl Serialize for RelativeSamplePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelativeSamplePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(D::Error::custom)
    }
}

/// A platform-neutral physical key identifier.
///
/// Windows scan codes are stored in the low 16 bits; bit 8 (`0x100`) marks an
/// extended key. The input crate is responsible for translating winit events
/// into this representation; the model never sees characters or keyboard
/// layouts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PhysicalKey(u32);

impl PhysicalKey {
    /// Extended-key flag.
    pub const EXTENDED: u32 = 0x100;

    /// Construct from a raw encoded value.
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Construct from a PC set-1 scan code.
    pub const fn from_scancode(scancode: u16) -> Self {
        Self(scancode as u32)
    }

    /// Construct from a scan code with the extended-key flag.
    pub const fn from_extended_scancode(scancode: u8) -> Self {
        Self(Self::EXTENDED | scancode as u32)
    }

    /// Raw encoded value.
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Human-readable key cap for the known default map, if any.
    pub const fn label(self) -> Option<&'static str> {
        Some(match self.0 {
            0x02 => "1",
            0x03 => "2",
            0x04 => "3",
            0x05 => "4",
            0x10 => "Q",
            0x11 => "W",
            0x12 => "E",
            0x13 => "R",
            0x1E => "A",
            0x1F => "S",
            0x20 => "D",
            0x21 => "F",
            0x2C => "Z",
            0x2D => "X",
            0x2E => "C",
            0x2F => "V",
            0x3B => "F1",
            0x3C => "F2",
            0x3D => "F3",
            0x3E => "F4",
            0x3F => "F5",
            _ => return None,
        })
    }
}

impl Serialize for PhysicalKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for PhysicalKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(u32::deserialize(deserializer)?))
    }
}

/// Default 4x4 key block: `1 2 3 4`, `Q W E R`, `A S D F`, `Z X C V`.
pub const DEFAULT_PAD_SCANCODES: [u16; PAD_COUNT] = [
    0x02, 0x03, 0x04, 0x05, 0x10, 0x11, 0x12, 0x13, 0x1E, 0x1F, 0x20, 0x21, 0x2C, 0x2D, 0x2E, 0x2F,
];

/// Default MIDI notes for a 16-pad e-kit layout (Alesis Nitro Max compatible).
pub const DEFAULT_PAD_MIDI_NOTES: [u8; PAD_COUNT] = [
    36, 38, 40, 39, 48, 45, 43, 41, 42, 46, 23, 44, 49, 51, 57, 53,
];

/// Keyboard trigger mapping for one pad.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct KeyMapping {
    /// Physical key that triggers the pad.
    pub key: PhysicalKey,
}

impl KeyMapping {
    /// Construct.
    pub const fn new(key: PhysicalKey) -> Self {
        Self { key }
    }

    /// Default mapping for a pad index (0..16), if the index is in range.
    pub fn default_for(index: usize) -> Option<Self> {
        DEFAULT_PAD_SCANCODES
            .get(index)
            .map(|code| Self::new(PhysicalKey::from_scancode(*code)))
    }
}

/// MIDI note trigger mapping for one pad.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct MidiMapping {
    /// MIDI note number, 0..=127.
    pub note: MidiNote,
    /// One-based MIDI channel.
    #[serde(default)]
    pub channel: MidiChannel,
}

impl MidiMapping {
    /// Construct.
    pub const fn new(note: MidiNote, channel: MidiChannel) -> Self {
        Self { note, channel }
    }

    /// Construct on channel 1.
    pub const fn on_channel_1(note: MidiNote) -> Self {
        Self {
            note,
            channel: MidiChannel::from_raw_unchecked(1),
        }
    }

    /// Default mapping for a pad index (0..16).
    pub fn default_for(index: usize) -> Option<Self> {
        DEFAULT_PAD_MIDI_NOTES
            .get(index)
            .map(|note| Self::on_channel_1(MidiNote::from_raw_unchecked(*note)))
    }
}

/// Playback mode for a pad.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    /// Play the whole sample on each trigger.
    #[default]
    OneShot,
    /// Play while the triggering key or note is held.
    Gate,
    /// Loop between the configured loop points.
    Loop,
}

/// Inclusive velocity range, 1..=127.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct VelocityRange {
    /// Lowest velocity in the range.
    pub min: u8,
    /// Highest velocity in the range.
    pub max: u8,
}

impl VelocityRange {
    /// Construct after validating bounds and ordering.
    pub fn new(min: u8, max: u8) -> Result<Self, DomainError> {
        if !(1..=127).contains(&min) {
            return Err(DomainError::OutOfRange {
                field: "velocity min",
                value: i64::from(min),
                min: 1,
                max: 127,
            });
        }
        if !(1..=127).contains(&max) {
            return Err(DomainError::OutOfRange {
                field: "velocity max",
                value: i64::from(max),
                min: 1,
                max: 127,
            });
        }
        if min > max {
            return Err(DomainError::OutOfRange {
                field: "velocity range",
                value: i64::from(min),
                min: i64::from(max),
                max: 127,
            });
        }
        Ok(Self { min, max })
    }

    /// Range covering every velocity.
    pub const FULL: Self = Self { min: 1, max: 127 };

    /// True when `velocity` lies inside the inclusive range.
    pub fn contains(&self, velocity: u8) -> bool {
        (self.min..=self.max).contains(&velocity)
    }
}

impl Default for VelocityRange {
    fn default() -> Self {
        Self::FULL
    }
}

/// Sample-accurate loop points, in frames.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct SampleLoop {
    /// First frame of the loop.
    pub start_frame: u64,
    /// Frame after the last frame of the loop.
    pub end_frame: u64,
}

impl SampleLoop {
    /// Construct after checking that the range is non-empty.
    pub fn new(start_frame: u64, end_frame: u64) -> Result<Self, DomainError> {
        if start_frame >= end_frame {
            return Err(DomainError::OutOfRange {
                field: "loop end frame",
                value: end_frame as i64,
                min: (start_frame + 1) as i64,
                max: i64::MAX,
            });
        }
        Ok(Self {
            start_frame,
            end_frame,
        })
    }

    /// Number of frames in the loop.
    pub fn len(&self) -> u64 {
        self.end_frame - self.start_frame
    }

    /// Always false; a constructed loop is non-empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A reference to a sample file used by a kit.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SampleRef {
    /// Kit-relative path.
    pub path: RelativeSamplePath,
    /// Canonical decoded-content hash, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<ContentHash>,
    /// Original file name for display and relinking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
}

impl SampleRef {
    /// Construct with a known content hash.
    pub fn new(path: RelativeSamplePath, hash: ContentHash) -> Self {
        Self {
            path,
            hash: Some(hash),
            original_name: None,
        }
    }

    /// Resolve against a kit folder.
    pub fn resolve(&self, kit_dir: &Path) -> PathBuf {
        self.path.resolve(kit_dir)
    }
}

/// One velocity layer inside a pad.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Layer {
    /// Stable layer id, unique within the pad.
    pub id: LayerId,
    /// Sample used by the layer, if assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<SampleRef>,
    /// Velocity range that selects the layer.
    #[serde(default)]
    pub velocity: VelocityRange,
    /// Per-layer gain in dB.
    #[serde(default)]
    pub gain_db: f32,
    /// Per-layer tuning in cents.
    #[serde(default)]
    pub tune_cents: f32,
    /// Optional loop points for [`PlaybackMode::Loop`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_points: Option<SampleLoop>,
}

impl Layer {
    /// Silent layer with the given id covering the full velocity range.
    pub fn silent(id: LayerId) -> Self {
        Self {
            id,
            sample: None,
            velocity: VelocityRange::FULL,
            gain_db: 0.0,
            tune_cents: 0.0,
            loop_points: None,
        }
    }

    /// Layer with an assigned sample.
    pub fn with_sample(id: LayerId, sample: SampleRef) -> Self {
        Self {
            sample: Some(sample),
            ..Self::silent(id)
        }
    }
}

/// One of the 16 playable pads.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Pad {
    /// Stable pad id, unique within the kit.
    pub id: PadId,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Velocity layers, ordered low to high by convention.
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Playback mode for the pad.
    #[serde(default)]
    pub playback: PlaybackMode,
    /// Pad gain in dB.
    #[serde(default)]
    pub gain_db: f32,
    /// Stereo pan, -1.0 (left) to 1.0 (right).
    #[serde(default)]
    pub pan: f32,
    /// Pad tuning in semitones.
    #[serde(default)]
    pub tune_semitones: f32,
    /// Optional choke group shared with other pads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choke_group: Option<ChokeGroup>,
    /// Optional MIDI trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiMapping>,
    /// Optional keyboard trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyMapping>,
}

impl Pad {
    /// A pad with no layers and no mappings.
    pub fn silent(id: PadId) -> Self {
        Self {
            id,
            name: None,
            layers: Vec::new(),
            playback: PlaybackMode::OneShot,
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            choke_group: None,
            midi: None,
            key: None,
        }
    }

    /// A silent pad with default keyboard and MIDI mappings for its slot.
    pub fn default_for_slot(slot: usize) -> Self {
        let id = PadId::from_raw_unchecked(slot as u8);
        Self {
            name: Some(format!("Pad {}", slot + 1)),
            key: KeyMapping::default_for(slot),
            midi: MidiMapping::default_for(slot),
            ..Self::silent(id)
        }
    }

    /// True when the pad has no playable sample.
    pub fn is_silent(&self) -> bool {
        !self.layers.iter().any(|layer| layer.sample.is_some())
    }

    /// First layer with an assigned sample.
    pub fn first_sample(&self) -> Option<&SampleRef> {
        self.layers.iter().find_map(|layer| layer.sample.as_ref())
    }

    /// All distinct samples referenced by the pad, in layer order.
    pub fn samples(&self) -> impl Iterator<Item = &SampleRef> {
        self.layers.iter().filter_map(|layer| layer.sample.as_ref())
    }
}

/// Metadata about a kit document.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KitMetadata {
    /// Display name.
    pub name: String,
    /// Optional author or pack name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Optional free-form description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional tags for search.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Creation time as Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_unix_ms: Option<u64>,
    /// Last save time as Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<u64>,
}

impl KitMetadata {
    /// Construct metadata with just a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            author: None,
            description: None,
            tags: Vec::new(),
            created_unix_ms: None,
            modified_unix_ms: None,
        }
    }
}

/// A complete, versioned kit document.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Kit {
    /// On-disk schema version.
    pub format_version: u32,
    /// Stable kit identifier.
    pub id: KitId,
    /// Display and audit metadata.
    pub metadata: KitMetadata,
    /// Exactly [`PAD_COUNT`] pads in performance order.
    pub pads: Vec<Pad>,
}

impl Kit {
    /// Create a kit with 16 silent pads carrying the default mappings.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: CURRENT_FORMAT_VERSION,
            id: KitId::new(),
            metadata: KitMetadata::new(name),
            pads: (0..PAD_COUNT).map(Pad::default_for_slot).collect(),
        }
    }

    /// Factory kit used on first run: no samples, default mappings.
    pub fn silent_default() -> Self {
        Self::new("Default Kit")
    }

    /// Look a pad up by stable id.
    pub fn pad(&self, id: PadId) -> Option<&Pad> {
        self.pads.iter().find(|pad| pad.id == id)
    }

    /// Mutable pad lookup by stable id.
    pub fn pad_mut(&mut self, id: PadId) -> Option<&mut Pad> {
        self.pads.iter_mut().find(|pad| pad.id == id)
    }

    /// Pad in performance order.
    pub fn pad_at(&self, slot: usize) -> Option<&Pad> {
        self.pads.get(slot)
    }

    /// Mutable pad in performance order.
    pub fn pad_at_mut(&mut self, slot: usize) -> Option<&mut Pad> {
        self.pads.get_mut(slot)
    }

    /// Find the pad bound to a physical key.
    pub fn pad_for_key(&self, key: PhysicalKey) -> Option<&Pad> {
        self.pads
            .iter()
            .find(|pad| pad.key.is_some_and(|mapping| mapping.key == key))
    }

    /// Find the pad bound to a MIDI note and channel.
    pub fn pad_for_midi(&self, note: MidiNote, channel: MidiChannel) -> Option<&Pad> {
        self.pads.iter().find(|pad| {
            pad.midi
                .is_some_and(|mapping| mapping.note == note && mapping.channel == channel)
        })
    }

    /// First pad without a sample, in performance order.
    pub fn first_empty_pad(&self) -> Option<PadId> {
        self.pads
            .iter()
            .find(|pad| pad.is_silent())
            .map(|pad| pad.id)
    }

    /// Next unused pad id, if the kit is not already full.
    pub fn next_free_pad_id(&self) -> Option<PadId> {
        (0..=PadId::MAX)
            .map(PadId::from_raw_unchecked)
            .find(|id| self.pad(*id).is_none())
    }

    /// Next unused layer id inside a pad.
    pub fn next_free_layer_id(&self, pad_id: PadId) -> LayerId {
        let mut next = 1;
        if let Some(pad) = self.pad(pad_id) {
            while pad.layers.iter().any(|layer| layer.id.0 == next) {
                next += 1;
            }
        }
        LayerId(next)
    }

    /// Distinct samples referenced by the kit, in pad/slot order.
    pub fn samples(&self) -> Vec<&SampleRef> {
        let mut seen = std::collections::BTreeSet::new();
        let mut result = Vec::new();
        for pad in &self.pads {
            for sample in pad.samples() {
                if seen.insert(sample.path.as_str().to_owned()) {
                    result.push(sample);
                }
            }
        }
        result
    }

    /// Stamp the modification time and return the previous value.
    pub fn touch(&mut self, now_unix_ms: u64) {
        if self.metadata.created_unix_ms.is_none() {
            self.metadata.created_unix_ms = Some(now_unix_ms);
        }
        self.metadata.modified_unix_ms = Some(now_unix_ms);
    }

    /// Validate all invariants, returning every issue found.
    pub fn validate(&self) -> Result<(), crate::kit::ValidationError> {
        crate::kit::validate_kit(self)
    }
}

/// Current wall-clock time in Unix milliseconds.
pub fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_stable_and_rate_sensitive() {
        let samples = [0.0f32, 0.5, -0.5, 1.0];
        let a = ContentHash::of_pcm(48_000, 2, &samples);
        let b = ContentHash::of_pcm(48_000, 2, &samples);
        let c = ContentHash::of_pcm(44_100, 2, &samples);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.to_hex().len(), 64);
        assert_eq!(ContentHash::from_hex(&a.to_hex()).unwrap(), a);
    }

    #[test]
    fn sample_paths_reject_escapes_and_devices() {
        assert!(RelativeSamplePath::new("samples/kick.wav").is_ok());
        assert!(RelativeSamplePath::new("samples\\kick.wav").is_ok());
        assert_eq!(
            RelativeSamplePath::new("samples\\kick.wav")
                .unwrap()
                .as_str(),
            "samples/kick.wav"
        );
        for bad in [
            "",
            "/abs/kick.wav",
            "C:\\kits\\kick.wav",
            "../kick.wav",
            "samples/../../kick.wav",
            "./kick.wav",
            "samples/CON.wav",
            "samples/a\0b.wav",
        ] {
            assert!(
                RelativeSamplePath::new(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn bounded_ids_reject_out_of_range_values() {
        assert!(PadId::new(15).is_ok());
        assert!(PadId::new(16).is_err());
        assert!(MidiNote::new(127).is_ok());
        assert!(MidiNote::new(128).is_err());
        assert!(MidiChannel::new(0).is_err());
        assert!(MidiChannel::new(16).is_ok());
        assert!(ChokeGroup::new(15).is_ok());
        assert!(ChokeGroup::new(16).is_err());
    }

    #[test]
    fn velocity_and_loop_ranges_validate() {
        assert!(VelocityRange::new(1, 127).is_ok());
        assert!(VelocityRange::new(0, 100).is_err());
        assert!(VelocityRange::new(100, 50).is_err());
        assert!(SampleLoop::new(10, 11).is_ok());
        assert!(SampleLoop::new(11, 11).is_err());
    }

    #[test]
    fn default_keys_and_notes_are_unique() {
        let keys: std::collections::BTreeSet<_> = DEFAULT_PAD_SCANCODES.iter().copied().collect();
        assert_eq!(keys.len(), PAD_COUNT);
        let notes: std::collections::BTreeSet<_> = DEFAULT_PAD_MIDI_NOTES.iter().copied().collect();
        assert_eq!(notes.len(), PAD_COUNT);
    }
}
