//! Local logging, log rotation, and crash capture.
//!
//! Logging is local-only. The app writes daily rolling files under the app log
//! directory and never adds a network transport; diagnostics are opt-in local
//! files that the user can inspect or delete. See
//! `docs/adr/0006-local-only-diagnostics.md`.
//!
//! Crash capture installs a panic hook that renders a report with the app
//! version, platform, thread, panic message, source location, and a backtrace,
//! then stores it under the crash directory. The hook works in a GUI build with
//! no console attached because it writes a file before returning.
//!
//! Paths that contain the user profile are redacted before they are written to
//! a crash report.

use std::io::{self, IsTerminal};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use drumpad_model::config::{
    LogLevel, LoggingSettings, MAX_LOG_RETENTION_DAYS, MIN_LOG_RETENTION_DAYS,
};
use drumpad_model::error::{Error, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::settings::{AppPaths, write_atomic};

/// Prefix of the daily log files. Files look like `drumpad.log.2026-11-07`.
pub const LOG_FILE_PREFIX: &str = "drumpad.log";

/// Prefix of the crash report files.
///
/// Files look like `crash-<unix>-<pid>-<counter>.txt`.
pub const CRASH_FILE_PREFIX: &str = "crash";

/// Distinguishes reports from simultaneous panics in one process.
static CRASH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Device summary supplied by the composition layer once streams are open.
static DEVICE_CONTEXT: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Record the active audio and MIDI devices for the next crash report.
///
/// The app shell calls this after opening streams, for example
/// `"audio=Speakers (Realtek) 48000 Hz 128 frames; midi=Nitro Max"`. Keeping it
/// out of this crate avoids a dependency on the audio and input crates.
pub fn set_device_context(summary: impl Into<String>) {
    let mut context = DEVICE_CONTEXT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *context = Some(summary.into());
}

fn device_context() -> Option<String> {
    // The panic hook must never block: if another thread holds the lock (or a
    // previous panic poisoned it), report that device context is unavailable.
    DEVICE_CONTEXT
        .try_read()
        .ok()
        .and_then(|context| context.clone())
}

/// Controls how logging starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggingOptions {
    /// Minimum level written to the log file.
    pub level: LogLevel,
    /// Days of daily log files to keep.
    pub retention_days: u32,
    /// Mirror logs to stderr when a console is attached.
    pub console: bool,
}

impl LoggingOptions {
    /// Build options for the given level and retention.
    #[must_use]
    pub fn new(level: LogLevel, retention_days: u32) -> Self {
        Self {
            level,
            retention_days,
            console: io::stderr().is_terminal(),
        }
    }

    /// Force console mirroring on or off.
    #[must_use]
    pub fn with_console(mut self, console: bool) -> Self {
        self.console = console;
        self
    }
}

impl From<&LoggingSettings> for LoggingOptions {
    fn from(settings: &LoggingSettings) -> Self {
        Self::new(settings.level, settings.retention_days)
    }
}

/// Keeps the non-blocking log writer alive for the lifetime of the app.
///
/// Drop it last: dropping flushes pending log lines to disk.
pub struct LoggingGuard {
    _guard: WorkerGuard,
}

/// Start file logging and optionally mirror it to stderr.
///
/// `RUST_LOG`, when set, overrides the level from settings. The returned guard
/// must stay alive until shutdown.
pub fn init(paths: &AppPaths, options: LoggingOptions) -> Result<LoggingGuard> {
    let retention_days = options
        .retention_days
        .clamp(MIN_LOG_RETENTION_DAYS, MAX_LOG_RETENTION_DAYS);
    // Keep today plus the configured number of previous days.
    let max_log_files = usize::try_from(retention_days)
        .unwrap_or(usize::MAX)
        .saturating_add(1);

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(max_log_files)
        .build(paths.log_dir())
        .map_err(|error| {
            Error::io(
                format!(
                    "could not open the log directory {}",
                    paths.log_dir().display()
                ),
                io::Error::other(error),
            )
        })?;

    let (file_writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(options.level.as_str()));

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_thread_names(true)
        .with_writer(file_writer);
    let console_layer = options
        .console
        .then(|| fmt::layer().with_ansi(true).with_writer(io::stderr));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(|error| {
            Error::io(
                "could not install the logging subscriber",
                io::Error::other(error),
            )
        })?;

    Ok(LoggingGuard { _guard: guard })
}

/// A crash captured from a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    /// App version that crashed.
    pub version: String,
    /// Wall-clock time of the panic, in seconds since the Unix epoch.
    pub unix_time_secs: u64,
    /// Operating system, from `std::env::consts::OS`.
    pub os: &'static str,
    /// CPU architecture, from `std::env::consts::ARCH`.
    pub arch: &'static str,
    /// Name of the panicking thread.
    pub thread: String,
    /// Audio and MIDI device summary, when the app had opened streams.
    pub device_context: Option<String>,
    /// Panic message, with user paths redacted.
    pub message: String,
    /// Source location of the panic, when known.
    pub location: Option<String>,
    /// Captured backtrace text, with user paths redacted.
    pub backtrace: String,
}

impl CrashReport {
    /// Capture the current panic.
    #[must_use]
    pub fn capture(version: &str, info: &PanicHookInfo<'_>) -> Self {
        let location = info
            .location()
            .map(|location| redact(&location.to_string()));
        Self {
            version: version.to_owned(),
            unix_time_secs: unix_time_secs(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            thread: std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_owned(),
            device_context: device_context().map(|context| redact(&context)),
            message: redact(&panic_message(info)),
            location,
            backtrace: redact(&std::backtrace::Backtrace::force_capture().to_string()),
        }
    }

    /// Render the human-readable report. It is stored locally and never sent.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut report = String::new();
        let _ = writeln!(report, "Drumpad crash report");
        let _ = writeln!(report, "version: {}", self.version);
        let _ = writeln!(report, "unix_time_secs: {}", self.unix_time_secs);
        let _ = writeln!(report, "os: {} ({})", self.os, self.arch);
        let _ = writeln!(report, "thread: {}", self.thread);
        let _ = writeln!(
            report,
            "device: {}",
            self.device_context.as_deref().unwrap_or("<not opened>")
        );
        let _ = writeln!(report, "message: {}", self.message);
        let _ = writeln!(
            report,
            "location: {}",
            self.location.as_deref().unwrap_or("<unknown>")
        );
        let _ = writeln!(report, "backtrace:");
        report.push_str(&self.backtrace);
        if !report.ends_with('\n') {
            report.push('\n');
        }
        let _ = writeln!(
            report,
            "This report is stored locally under the Drumpad crash directory and is never uploaded."
        );
        report
    }

    /// Write the report into `crash_dir` and return its path.
    pub fn write_to(&self, crash_dir: &Path) -> Result<PathBuf> {
        let path = crash_dir.join(format!(
            "{CRASH_FILE_PREFIX}-{}-{}-{}.txt",
            self.unix_time_secs,
            std::process::id(),
            CRASH_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        write_atomic(&path, self.render().as_bytes())?;
        Ok(path)
    }
}

/// Install a panic hook that writes a crash report before the previous hook runs.
///
/// Call once, after the crash directory exists. The hook is process-global.
pub fn install_crash_hook(crash_dir: PathBuf, version: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = CrashReport::capture(version, info);
        match report.write_to(&crash_dir) {
            Ok(path) => eprintln!(
                "Drumpad crashed. A local crash report was written to {}",
                path.display()
            ),
            Err(error) => {
                eprintln!("Drumpad crashed and the crash report could not be written: {error}")
            }
        }
        previous(info);
    }));
}

/// Replace the current user's profile path with `%USERPROFILE%` in `text`.
///
/// This keeps local crash reports useful without copying personal paths into a
/// file the user may share when asking for help.
#[must_use]
pub fn redact(text: &str) -> String {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|path| path.to_string_lossy().into_owned());
    match home {
        Some(home) => redact_home(text, &home),
        None => text.to_owned(),
    }
}

fn redact_home(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['\\', '/']);
    // Never replace a bare drive root or a one-letter home.
    if home.len() <= 3 {
        return text.to_owned();
    }

    let mut redacted = text.replace(home, "%USERPROFILE%");
    let forward_slashes = home.replace('\\', "/");
    if forward_slashes != home {
        redacted = redacted.replace(&forward_slashes, "%USERPROFILE%");
    }
    redacted
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

fn unix_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AppPaths;
    use tempfile::tempdir;

    #[test]
    fn init_writes_logs_into_the_app_log_directory_with_daily_rotation() {
        let temp = tempdir().expect("temp dir");
        let paths = AppPaths::rooted(temp.path().join("root"), true);
        paths.ensure().expect("ensure directories");

        let guard = init(
            &paths,
            LoggingOptions::new(LogLevel::Info, 3).with_console(false),
        )
        .expect("init logging");
        tracing::info!("sentinel log line from test");
        drop(guard);

        let names: Vec<String> = std::fs::read_dir(paths.log_dir())
            .expect("read log dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let log_name = names
            .iter()
            .find(|name| name.starts_with(LOG_FILE_PREFIX))
            .unwrap_or_else(|| panic!("expected a log file in {names:?}"));

        // Daily rotation appends a yyyy-mm-dd suffix.
        let suffix = log_name
            .strip_prefix(&format!("{LOG_FILE_PREFIX}."))
            .expect("daily suffix");
        assert_eq!(suffix.len(), 10, "unexpected log name {log_name}");
        assert!(
            suffix.chars().enumerate().all(|(index, character)| {
                if index == 4 || index == 7 {
                    character == '-'
                } else {
                    character.is_ascii_digit()
                }
            }),
            "unexpected daily suffix {suffix}"
        );

        let contents =
            std::fs::read_to_string(paths.log_dir().join(log_name)).expect("read log file");
        assert!(
            contents.contains("sentinel log line from test"),
            "log file did not receive the sentinel line: {contents}"
        );
    }

    #[test]
    fn paths_are_redacted_in_both_separator_styles() {
        let home = r"C:\Users\ezers";
        let text = r"sample C:\Users\ezers\Music\kick.wav and C:/Users/ezers/Music/snare.wav";
        assert_eq!(
            redact_home(text, home),
            r"sample %USERPROFILE%\Music\kick.wav and %USERPROFILE%/Music/snare.wav"
        );
    }

    #[test]
    fn short_home_paths_are_not_redacted() {
        assert_eq!(
            redact_home("C:\\temp\\kick.wav", "C:\\"),
            "C:\\temp\\kick.wav"
        );
    }

    #[test]
    fn crash_report_renders_the_details_needed_to_debug() {
        let report = CrashReport {
            version: "0.1.0".to_owned(),
            unix_time_secs: 1_700_000_000,
            os: "windows",
            arch: "x86_64",
            thread: "main".to_owned(),
            device_context: Some("audio=Test Speakers 48000 Hz; midi=Nitro Max".to_owned()),
            message: "boom".to_owned(),
            location: Some("crates/drumpad-app/src/main.rs:42".to_owned()),
            backtrace: "0: core::panicking::panic\n".to_owned(),
        };

        let rendered = report.render();
        for expected in [
            "Drumpad crash report",
            "version: 0.1.0",
            "os: windows (x86_64)",
            "thread: main",
            "device: audio=Test Speakers 48000 Hz; midi=Nitro Max",
            "message: boom",
            "crates/drumpad-app/src/main.rs:42",
            "0: core::panicking::panic",
            "never uploaded",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} in {rendered}"
            );
        }
    }

    #[test]
    fn crash_report_is_written_as_a_file_without_a_console() {
        let temp = tempdir().expect("temp dir");
        let report = CrashReport {
            version: "0.1.0".to_owned(),
            unix_time_secs: 1_700_000_001,
            os: "windows",
            arch: "x86_64",
            thread: "worker".to_owned(),
            device_context: None,
            message: "panic without console".to_owned(),
            location: None,
            backtrace: "<unavailable>\n".to_owned(),
        };

        let path = report.write_to(temp.path()).expect("write report");
        assert!(path.is_file());
        assert!(
            path.file_name()
                .expect("file name")
                .to_string_lossy()
                .starts_with(CRASH_FILE_PREFIX)
        );
        let contents = std::fs::read_to_string(path).expect("read report");
        assert!(contents.contains("panic without console"));
        assert!(contents.contains("location: <unknown>"));
    }
}
