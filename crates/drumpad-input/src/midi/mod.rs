//! USB-MIDI capture and the Alesis Nitro Max integration.
//!
//! The MIDI path is layered so each piece can be tested without hardware:
//!
//! ```text
//! driver callback
//!   -> [parse]      bytes to NoteOn/NoteOff/CC, running status, realtime ignore
//!   -> [router]     channel filter, learn, velocity curve, note map, hi-hat
//!   -> TriggerEvent / ReleaseEvent / HiHat articulation
//!   -> lock-free queue -> audio callback
//! ```
//!
//! [`backend::MidiInputBackend`] wraps midir; [`backend::FakeMidiBackend`] drives
//! the same code in tests. [`hotplug::HotplugManager`] polls for unplug/replug
//! and reopens the saved device by name. [`monitor::MidiMonitor`] keeps a
//! bounded, pausable view of incoming traffic for the Settings screen.
//!
//! Nitro Max note map and hi-hat behavior are documented in
//! `docs/nitro-max.md`.

pub mod backend;
pub mod hihat;
pub mod hotplug;
pub mod latency;
pub mod learn;
pub mod monitor;
pub mod note_map;
pub mod parse;
pub mod router;
pub mod velocity;

#[cfg(feature = "midir-backend")]
pub use backend::MidirBackend;
pub use backend::{
    FakeMidiBackend, MidiConnection, MidiError, MidiHandler, MidiInputBackend, MidiPortInfo,
};
pub use hihat::{HiHatArticulation, HiHatConfig, HiHatEngine, HiHatSound, HiHatTrigger};
pub use hotplug::{HotplugEvent, HotplugManager, MidiPreferences};
pub use latency::{MidiLatencyReport, MidiLatencyTracker};
pub use learn::{CapturedControl, LearnOutcome, MidiLearn};
pub use monitor::{MidiMonitor, MidiMonitorEntry, MonitorKind, note_name};
pub use note_map::{
    AssignPolicy, MidiMapError, MidiNoteMap, NOTE_MAP_SCHEMA_VERSION, NoteConflict, NoteMapping,
};
pub use parse::{ChannelFilter, IgnoreKind, MidiEvent, MidiParser};
pub use router::{MidiIgnoreReason, MidiOutput, MidiRouter, MidiRouterConfig};
pub use velocity::{CurveError, MidiVelocityConfig, VelocityCurve, VelocityCurveTable};
