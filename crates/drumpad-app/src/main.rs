//! Drumpad entry point.
//!
//! Startup resolves the app directory layout (portable or installed), creates
//! it on first launch, loads or writes the settings document, starts local
//! logging, and installs crash capture. The winit/egui shell is owned by the UI
//! pair and is wired in at the end of [`run`] once it lands.

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
            // still visible in a terminal when one exists, and future UI work
            // turns this into an error dialog with a concrete next action.
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

    let _logging = logging::init(&paths, LoggingOptions::from(&settings.logging))?;
    logging::install_crash_hook(paths.crash_dir().to_path_buf(), env!("CARGO_PKG_VERSION"));

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        portable = paths.is_portable(),
        config_dir = %paths.config_dir().display(),
        "Drumpad starting"
    );

    // E08-T01 (UI pair) owns the app shell. It receives the resolved paths and
    // settings here so the shell never re-implements directory or settings
    // resolution. Until that lands, startup verifies the foundation and exits.
    tracing::info!("foundation startup complete; waiting for the app shell");

    Ok(())
}
