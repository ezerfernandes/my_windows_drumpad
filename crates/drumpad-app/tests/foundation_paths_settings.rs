//! Behavior tests for foundation app paths, portable mode, settings, and
//! atomic file writes.
//!
//! These drive the public `drumpad_app::settings` API only; no UI or audio
//! hardware is involved.

use std::fs;
use std::path::{Path, PathBuf};

use drumpad_app::settings::{
    AppPaths, PORTABLE_DATA_DIR, PORTABLE_MARKER_FILE, SETTINGS_FILE_NAME, SettingsStore,
    write_atomic,
};
use drumpad_model::config::{AppSettings, LogLevel};
use drumpad_model::error::Error;
use tempfile::tempdir;

fn marker_path(directory: &Path) -> PathBuf {
    directory.join(PORTABLE_MARKER_FILE)
}

#[test]
fn discover_resolves_an_installed_layout_for_the_test_executable() {
    let paths = AppPaths::discover().expect("discover app paths");

    assert!(!paths.is_portable());
    for directory in [
        paths.config_dir(),
        paths.data_dir(),
        paths.cache_dir(),
        paths.log_dir(),
        paths.crash_dir(),
    ] {
        assert!(
            directory.is_absolute(),
            "{} should be an absolute installed directory",
            directory.display()
        );
    }
    assert_eq!(
        paths.settings_path(),
        paths.config_dir().join(SETTINGS_FILE_NAME)
    );
}

#[test]
fn executable_directory_without_a_marker_falls_back_to_installed_layout() {
    let temp = tempdir().expect("temp dir");
    let paths = AppPaths::for_executable_dir(temp.path()).expect("installed paths");

    assert!(!paths.is_portable());
    assert_eq!(
        paths.settings_path(),
        paths.config_dir().join(SETTINGS_FILE_NAME)
    );
}

#[test]
fn portable_marker_moves_every_directory_beside_the_executable() {
    let temp = tempdir().expect("temp dir");
    fs::write(marker_path(temp.path()), b"portable").expect("write marker");

    let paths = AppPaths::for_executable_dir(temp.path()).expect("portable paths");
    let root = temp.path().join(PORTABLE_DATA_DIR);

    assert!(paths.is_portable());
    assert_eq!(paths.config_dir(), root.join("config"));
    assert_eq!(paths.data_dir(), root.join("data"));
    assert_eq!(paths.cache_dir(), root.join("cache"));
    assert_eq!(paths.log_dir(), root.join("logs"));
    assert_eq!(paths.crash_dir(), root.join("crashes"));
    assert_eq!(
        paths.settings_path(),
        root.join("config").join(SETTINGS_FILE_NAME)
    );
}

#[test]
fn unreadable_marker_path_reports_an_io_error() {
    // '<' is not legal in a Windows file name, so metadata fails with something
    // other than NotFound and the app reports it instead of silently falling
    // back to the installed layout.
    let invalid_directory = Path::new("invalid<portable>path");
    let error = AppPaths::for_executable_dir(invalid_directory)
        .expect_err("an unreadable marker path must fail");
    assert!(matches!(error, Error::Io { .. }), "unexpected {error:?}");
}

#[test]
fn ensure_reports_a_directory_that_cannot_be_created() {
    let temp = tempdir().expect("temp dir");
    let blocked_root = temp.path().join("root-is-a-file");
    fs::write(&blocked_root, b"not a directory").expect("write blocker");
    let paths = AppPaths::rooted(blocked_root, true);

    let error = paths.ensure().expect_err("ensure must fail");
    assert!(matches!(error, Error::Io { .. }), "unexpected {error:?}");
}

#[test]
fn settings_round_trip_through_atomic_saves_and_replacement() {
    let temp = tempdir().expect("temp dir");
    let store = SettingsStore::new(temp.path().join("config").join(SETTINGS_FILE_NAME));

    let mut settings = AppSettings::default();
    settings.logging.level = LogLevel::Debug;
    settings.logging.retention_days = 7;
    settings.ui.scale = 1.25;
    settings.ui.show_key_labels = false;

    store.save(&settings).expect("first save");
    assert_eq!(store.load().expect("first load"), settings);
    // An existing document is loaded, not overwritten with defaults.
    assert_eq!(store.load_or_create().expect("load existing"), settings);

    settings.logging.retention_days = 30;
    store.save(&settings).expect("replace");
    assert_eq!(store.load().expect("reload"), settings);

    let leftovers: Vec<_> = fs::read_dir(temp.path().join("config"))
        .expect("read config dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files must not survive");
}

#[test]
fn load_or_create_writes_defaults_on_first_launch() {
    let temp = tempdir().expect("temp dir");
    let store = SettingsStore::new(temp.path().join("config").join(SETTINGS_FILE_NAME));

    assert_eq!(
        store.load().expect("missing loads as default"),
        AppSettings::default()
    );
    assert!(!store.path().exists(), "load must not write");

    let created = store.load_or_create().expect("first launch");
    assert_eq!(created, AppSettings::default());
    assert!(store.path().is_file());
}

#[test]
fn load_rejects_corrupt_future_and_invalid_documents() {
    let temp = tempdir().expect("temp dir");
    let path = temp.path().join(SETTINGS_FILE_NAME);

    fs::write(&path, b"{ not json").expect("write corrupt");
    let error = SettingsStore::new(&path).load().expect_err("corrupt");
    assert!(matches!(error, Error::Json { .. }), "unexpected {error:?}");

    fs::write(&path, r#"{"schema_version":99}"#).expect("write future");
    let error = SettingsStore::new(&path).load().expect_err("future");
    assert!(
        matches!(error, Error::UnsupportedSchemaVersion { found: 99, .. }),
        "unexpected {error:?}"
    );

    fs::write(
        &path,
        r#"{"schema_version":1,"logging":{"level":"info","retention_days":0}}"#,
    )
    .expect("write invalid");
    let error = SettingsStore::new(&path).load().expect_err("invalid value");
    assert!(matches!(error, Error::Config(_)), "unexpected {error:?}");

    fs::write(
        &path,
        r#"{"schema_version":1,"ui":{"scale":1.0,"unknown_field":true}}"#,
    )
    .expect("write unknown field");
    let error = SettingsStore::new(&path).load().expect_err("unknown field");
    assert!(matches!(error, Error::Json { .. }), "unexpected {error:?}");
}

#[test]
fn load_reports_a_settings_path_that_is_not_a_file() {
    let temp = tempdir().expect("temp dir");
    let directory = temp.path().join("settings-is-a-directory");
    fs::create_dir(&directory).expect("create directory");

    let error = SettingsStore::new(&directory)
        .load()
        .expect_err("a directory is not a settings document");
    assert!(matches!(error, Error::Io { .. }), "unexpected {error:?}");
}

#[test]
fn save_rejects_invalid_values_without_touching_disk() {
    let temp = tempdir().expect("temp dir");
    let store = SettingsStore::new(temp.path().join(SETTINGS_FILE_NAME));

    let mut settings = AppSettings::default();
    settings.logging.retention_days = 0;
    let error = store.save(&settings).expect_err("invalid retention");
    assert!(matches!(error, Error::Config(_)), "unexpected {error:?}");
    assert!(!store.path().exists());
}

#[test]
fn write_atomic_replaces_an_existing_document() {
    let temp = tempdir().expect("temp dir");
    let path = temp.path().join("document.json");

    write_atomic(&path, b"first").expect("first write");
    write_atomic(&path, b"second").expect("replace write");

    assert_eq!(fs::read(&path).expect("read"), b"second");
}

#[test]
fn write_atomic_reports_missing_parent_and_file_name_paths() {
    let error = write_atomic(Path::new(""), b"data").expect_err("empty path");
    assert!(
        matches!(error, Error::MissingParent { .. }),
        "unexpected {error:?}"
    );

    let temp = tempdir().expect("temp dir");
    let path_without_name = temp.path().join("child").join("..");
    let error = write_atomic(&path_without_name, b"data").expect_err("path without file name");
    assert!(
        matches!(error, Error::MissingParent { .. }),
        "unexpected {error:?}"
    );
}

#[test]
fn write_atomic_reports_a_parent_that_is_a_file() {
    let temp = tempdir().expect("temp dir");
    let blocker = temp.path().join("blocker");
    fs::write(&blocker, b"file").expect("write blocker");

    let error = write_atomic(&blocker.join("document.json"), b"data")
        .expect_err("parent cannot be created");
    assert!(matches!(error, Error::Io { .. }), "unexpected {error:?}");
}

#[test]
fn write_atomic_cleans_up_when_the_rename_fails() {
    let temp = tempdir().expect("temp dir");
    let target_directory = temp.path().join("target-is-a-directory");
    fs::create_dir(&target_directory).expect("create target directory");

    let error =
        write_atomic(&target_directory, b"data").expect_err("renaming over a directory must fail");
    assert!(matches!(error, Error::Io { .. }), "unexpected {error:?}");

    let leftovers: Vec<_> = fs::read_dir(temp.path())
        .expect("read temp dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "failed writes must clean up temporary files"
    );
}
