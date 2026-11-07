//! Real-time playback engine for Drumpad.
//!
//! The engine owns a preallocated voice pool and consumes triggers and
//! parameter commands from lock-free queues. [`mixer::Engine::process`] is the
//! only function that must run on the audio thread; it performs no allocation,
//! locking, logging, or file access.

pub mod limiter;
pub mod meter;
pub mod mixer;
pub mod queue;
pub mod voice;
