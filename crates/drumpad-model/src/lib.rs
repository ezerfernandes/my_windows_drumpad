//! Domain model for Drumpad: kits, pads, velocity layers, and mappings.
//!
//! The crate is deliberately free of UI, audio-device, and engine
//! dependencies so it can be tested in isolation. `kit` owns the durable
//! document format and persistence helpers; `mappings` owns the
//! platform-neutral key and MIDI mapping types.

pub mod kit;
pub mod mappings;

pub mod config;
pub mod error;
pub mod ids;

pub use ids::{ModelError as IdError, PadId, Velocity};

pub use kit::{
    CURRENT_FORMAT_VERSION, ChokeGroup, ContentHash, DomainError, KeyMapping, Kit, KitId,
    KitMetadata, Layer, LayerId, MidiChannel, MidiMapping, MidiNote, Pad, PlaybackMode, SampleLoop,
    SampleRef, VelocityRange,
};
pub use mappings::{KeyBinding, KeyboardProfile, PhysicalKey};
