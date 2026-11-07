//! Drumpad entry point.
//!
//! Startup resolves the app directory layout (portable or installed), creates
//! it on first launch, loads or writes the settings document, starts local
//! logging, installs crash capture, and hands the resolved paths and settings
//! to the winit/egui app shell. The shell owns the window, audio, input, and UI
//! wiring; this crate only composes the foundation.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use drumpad_app::logging::{self, LoggingOptions};
use drumpad_app::settings::{AppPaths, SettingsStore};
use drumpad_model::error::Result;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // A GUI release has no attached console; the startup failure is
            // still visible in the local log and, when a console exists, on
            // stderr. The app shell owns the in-window error dialog.
            eprintln!("Drumpad failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let paths = AppPaths::discover()?;
    paths.ensure()?;

    let settings_store = SettingsStore::new(paths.settings_path());
    let settings = settings_store.load_or_create()?;

    // Keep the guard for the whole run: dropping it flushes pending log lines.
    let _logging = logging::init(&paths, LoggingOptions::from(&settings.logging))?;
    logging::install_crash_hook(paths.crash_dir().to_path_buf(), env!("CARGO_PKG_VERSION"));

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        portable = paths.is_portable(),
        config_dir = %paths.config_dir().display(),
        "Drumpad starting"
    );

    // The shell receives the already-resolved paths and settings so it never
    // re-implements directory, settings, or logging setup.
    drumpad_app::app::run(drumpad_app::app::AppOptions::new(paths, settings))?;

    tracing::info!("Drumpad closed cleanly");
    Ok(())
}
