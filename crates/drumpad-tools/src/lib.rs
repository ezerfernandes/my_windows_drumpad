//! Offline renderer, benchmarks, latency harness, and QA tooling.
//!
//! The crate is the home of everything that must run **without** an audio
//! device: deterministic offline rendering, scripted MIDI/keyboard events,
//! latency statistics and baseline reports, golden-audio comparison, soak
//! scaffolding, and the startup/load budget harness.
//!
//! See `docs/qa/test-plan.md` for the tiers and determinism rules.

pub mod fake;
pub mod fixture;
pub mod latency;
pub mod offline;
pub mod sequence;

pub use fake::{
    FakeAudioBackend, FakeHarness, FakeScriptError, HarnessReport, KeyboardScript, MidiScript,
};
pub use fixture::{FIXTURES, FixtureBank, FixtureError, FixtureManifest, PadFixture};
pub use latency::{
    AudioMode, BaselineReport, BaselineRow, BaselineStatus, HardwareProfile, LatencyStats, Metric,
    Percentile, TargetVerdict, evaluate, summarize,
};
pub use offline::{
    OfflineRender, OfflineSession, RenderConfig, RenderError, RenderStats, VoiceTraceEntry,
};
pub use sequence::{Action, Sequence, SequenceError, SequenceEvent};
