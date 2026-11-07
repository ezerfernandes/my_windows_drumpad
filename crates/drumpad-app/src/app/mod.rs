//! Application shell: entry point, intents, and the winit runner.
//!
//! The foundation's `main.rs` resolves paths, settings, and logging, then calls
//! [`run`] with an [`AppOptions`]:
//!
//! ```no_run
//! # use drumpad_app::app::{self, AppOptions};
//! # fn foundation(paths: drumpad_app::settings::AppPaths, settings: drumpad_model::config::AppSettings) -> drumpad_model::error::Result<()> {
//! app::run(AppOptions::new(paths, settings))
//! # }
//! ```
//!
//! [`run_default`] is the convenience path for a standalone launch; it resolves
//! paths and settings but leaves logging to the caller so it can be called after
//! the foundation's logging setup without installing a second subscriber.

pub mod actions;
pub mod hotkeys;
pub mod kits;
pub mod runner;

pub use actions::{Action, dispatch};
pub use kits::KitService;
pub use runner::{AppOptions, FRAME_INTERVAL, UserEvent, run, run_default};
