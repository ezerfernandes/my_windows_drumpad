//! Behavior tests for local logging setup.
//!
//! Logging installs a process-global subscriber, so this file contains exactly
//! one test that calls `init` successfully.

use std::fs;

use drumpad_app::logging::{self, LOG_FILE_PREFIX, LoggingOptions};
use drumpad_app::settings::AppPaths;
use drumpad_model::config::{LogLevel, LoggingSettings};
use drumpad_model::error::Error;
use tempfile::tempdir;

#[test]
fn logging_options_follow_settings_and_console_override() {
    let settings = LoggingSettings {
        level: LogLevel::Debug,
        retention_days: 30,
    };

    let options = LoggingOptions::from(&settings);
    assert_eq!(options.level, LogLevel::Debug);
    assert_eq!(options.retention_days, 30);

    assert!(options.with_console(true).console);
    assert!(!options.with_console(false).console);
}

#[test]
fn init_writes_a_daily_log_and_rejects_a_second_subscriber() {
    let temp = tempdir().expect("temp dir");
    let paths = AppPaths::rooted(temp.path().join("root"), true);
    paths.ensure().expect("ensure directories");

    let guard = logging::init(
        &paths,
        LoggingOptions::new(LogLevel::Debug, 7).with_console(true),
    )
    .expect("init logging");
    tracing::info!(
        target: "foundation_logging_test",
        "sentinel line from the logging integration test"
    );

    let second = logging::init(
        &paths,
        LoggingOptions::new(LogLevel::Info, 7).with_console(false),
    );
    assert!(
        matches!(second, Err(Error::Io { .. })),
        "a second global subscriber must be rejected"
    );

    drop(guard);

    let names: Vec<String> = fs::read_dir(paths.log_dir())
        .expect("read log dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let log_name = names
        .iter()
        .find(|name| name.starts_with(LOG_FILE_PREFIX))
        .unwrap_or_else(|| panic!("expected a daily log file in {names:?}"));

    let suffix = log_name
        .strip_prefix(&format!("{LOG_FILE_PREFIX}."))
        .expect("daily suffix");
    assert_eq!(suffix.len(), 10, "unexpected log name {log_name}");

    let contents = fs::read_to_string(paths.log_dir().join(log_name)).expect("read log file");
    assert!(
        contents.contains("sentinel line from the logging integration test"),
        "log file did not receive the sentinel line: {contents}"
    );
}

#[test]
fn init_reports_an_unusable_log_directory() {
    let temp = tempdir().expect("temp dir");
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("create root");
    fs::write(root.join("logs"), b"not a directory").expect("block log dir");
    let paths = AppPaths::rooted(root, true);

    let error = match logging::init(
        &paths,
        LoggingOptions::new(LogLevel::Info, 3).with_console(false),
    ) {
        Ok(_guard) => panic!("a file cannot be a log directory"),
        Err(error) => error,
    };
    assert!(matches!(error, Error::Io { .. }), "unexpected {error:?}");
}

#[test]
fn redact_hides_the_real_user_profile_in_both_separator_styles() {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        return;
    };
    let home = home.to_string_lossy().into_owned();
    if home.len() <= 3 {
        return;
    }

    let text = format!(
        "samples {home}\\Music\\kick.wav and {}",
        home.replace('\\', "/")
    );
    let redacted = logging::redact(&text);

    assert!(
        redacted.contains("%USERPROFILE%"),
        "not redacted: {redacted}"
    );
    assert!(!redacted.contains(&home), "home path leaked: {redacted}");
}

#[test]
fn redact_leaves_text_without_a_profile_path_unchanged() {
    assert_eq!(
        logging::redact("plain diagnostic text"),
        "plain diagnostic text"
    );
}
