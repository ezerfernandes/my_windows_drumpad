//! Application paths, portable mode, and atomic settings persistence.
//!
//! Installed builds use the per-user Windows directories resolved by
//! `directories`: roaming app data for config, local app data for data, cache,
//! logs, and crash reports. A build is *portable* when a file named
//! [`PORTABLE_MARKER_FILE`] sits next to the executable; everything then lives
//! under `<exe dir>/DrumpadData/`, so moving the folder moves the whole app.
//!
//! Settings writes are atomic: the app writes a temporary file in the target
//! directory, flushes it, then renames it over the destination. A crash during
//! the write therefore leaves the previous good file untouched.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use directories::ProjectDirs;
use drumpad_model::config::{AppSettings, SETTINGS_SCHEMA_VERSION};
use drumpad_model::error::{Error, Result};

/// Marker file that enables portable mode when it sits next to the executable.
pub const PORTABLE_MARKER_FILE: &str = "drumpad.portable";

/// Name of the settings document inside the config directory.
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Directory created beside the executable in portable mode.
pub const PORTABLE_DATA_DIR: &str = "DrumpadData";

const APP_QUALIFIER: &str = "com";
const APP_ORGANIZATION: &str = "Drumpad";
const APP_APPLICATION: &str = "Drumpad";

/// Distinguishes temporary files created by concurrent saves in one process.
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Directory layout for the running app.
///
/// Construct one with [`AppPaths::discover`] and call [`AppPaths::ensure`]
/// before touching any of the directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    config_dir: PathBuf,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    log_dir: PathBuf,
    crash_dir: PathBuf,
    portable: bool,
}

impl AppPaths {
    /// Resolve the layout for the currently running executable.
    ///
    /// Portable mode wins when the marker file is present; otherwise the
    /// per-user Windows directories are used.
    pub fn discover() -> Result<Self> {
        let exe = std::env::current_exe()
            .map_err(|error| Error::io("could not locate the Drumpad executable", error))?;
        let exe_dir = exe
            .parent()
            .ok_or_else(|| Error::MissingParent { path: exe.clone() })?;
        Self::for_executable_dir(exe_dir)
    }

    /// Resolve the layout for a known executable directory.
    ///
    /// This is the testable core of [`AppPaths::discover`].
    pub fn for_executable_dir(exe_dir: &Path) -> Result<Self> {
        let marker = exe_dir.join(PORTABLE_MARKER_FILE);
        match fs::metadata(&marker) {
            Ok(metadata) if metadata.is_file() => Ok(Self::portable(exe_dir)),
            Ok(_) => Err(Error::InvalidPortableMarker { path: marker }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::installed(),
            Err(error) => Err(Error::io(
                format!("could not read the portable marker {}", marker.display()),
                error,
            )),
        }
    }

    /// Portable layout: every directory lives beside the executable.
    pub fn portable(exe_dir: &Path) -> Self {
        Self::rooted(exe_dir.join(PORTABLE_DATA_DIR), true)
    }

    /// Installed layout: per-user directories from `directories`.
    pub fn installed() -> Result<Self> {
        let project_dirs = ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_APPLICATION)
            .ok_or(Error::MissingHomeDirectory)?;
        let data_local_dir = project_dirs.data_local_dir().to_path_buf();
        let (log_dir, crash_dir) = installed_log_dirs(&data_local_dir);
        Ok(Self {
            config_dir: project_dirs.config_dir().to_path_buf(),
            data_dir: data_local_dir,
            cache_dir: project_dirs.cache_dir().to_path_buf(),
            log_dir,
            crash_dir,
            portable: false,
        })
    }

    /// Build a layout rooted at a single directory.
    ///
    /// Portable mode uses this with `<exe dir>/DrumpadData`; tests use it to
    /// avoid touching the real user profile.
    pub fn rooted(root: PathBuf, portable: bool) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            log_dir: root.join("logs"),
            crash_dir: root.join("crashes"),
            portable,
        }
    }

    /// Create the directory tree. Safe to call on every launch.
    pub fn ensure(&self) -> Result<()> {
        for directory in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
            &self.crash_dir,
        ] {
            fs::create_dir_all(directory).map_err(|error| {
                Error::io(
                    format!("could not create the app directory {}", directory.display()),
                    error,
                )
            })?;
        }
        Ok(())
    }

    /// Directory holding user-editable configuration.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Directory holding durable app data such as kits and samples.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Directory holding regenerable caches.
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Directory holding rotating local log files.
    #[must_use]
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Directory holding local crash reports.
    #[must_use]
    pub fn crash_dir(&self) -> &Path {
        &self.crash_dir
    }

    /// Path of the settings document.
    #[must_use]
    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join(SETTINGS_FILE_NAME)
    }

    /// Whether every directory lives beside the executable.
    #[must_use]
    pub fn is_portable(&self) -> bool {
        self.portable
    }
}

/// `directories` gives the installed data directory as `...\Drumpad\Drumpad\data`.
/// Logs and crash reports are siblings of `data`, not children, so the data
/// directory stays reserved for user content.
fn installed_log_dirs(data_local_dir: &Path) -> (PathBuf, PathBuf) {
    let base = data_local_dir.parent().unwrap_or(data_local_dir);
    (base.join("logs"), base.join("crashes"))
}

/// Reads and writes the settings document for one path.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// Create a store for an explicit settings path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The settings document this store manages.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load settings, returning defaults when no file exists yet.
    ///
    /// A corrupt, hand-edited, or future-version file returns an error instead
    /// of being overwritten silently.
    pub fn load(&self) -> Result<AppSettings> {
        match fs::read_to_string(&self.path) {
            Ok(text) => parse_settings(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AppSettings::default())
            }
            Err(error) => Err(Error::io(
                format!("could not read the settings file {}", self.path.display()),
                error,
            )),
        }
    }

    /// Load settings, writing the defaults on first launch.
    pub fn load_or_create(&self) -> Result<AppSettings> {
        if self.path.exists() {
            self.load()
        } else {
            let settings = AppSettings::default();
            self.save(&settings)?;
            Ok(settings)
        }
    }

    /// Validate and atomically persist settings.
    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        settings.check()?;
        let json = serde_json::to_string_pretty(settings)
            .map_err(|error| Error::json("could not serialize settings", error))?;
        write_atomic(&self.path, format!("{json}\n").as_bytes())
    }
}

/// Parse and validate a settings document.
pub fn parse_settings(text: &str) -> Result<AppSettings> {
    let settings: AppSettings = serde_json::from_str(text).map_err(|error| {
        Error::json(
            "the settings file is not valid; fix or delete it to restore defaults",
            error,
        )
    })?;

    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err(Error::UnsupportedSchemaVersion {
            found: settings.schema_version,
            supported: SETTINGS_SCHEMA_VERSION,
        });
    }

    settings.validate()?;
    Ok(settings)
}

/// Write bytes to `path` through a flushed temporary file and an atomic rename.
///
/// The rename replaces an existing destination on Windows, so a reader never
/// observes a partially written document. Callers must serialize writes to the
/// same path: Windows rename-over-existing is not safe to run concurrently from
/// several threads. Settings saves happen on the UI thread.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error::MissingParent {
        path: path.to_path_buf(),
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        Error::io(
            format!("could not create the directory {}", parent.display()),
            error,
        )
    })?;

    let file_name = path
        .file_name()
        .ok_or_else(|| Error::MissingParent {
            path: path.to_path_buf(),
        })?
        .to_string_lossy()
        .into_owned();
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    })();

    if let Err(error) = result {
        // Best effort: never leave a partial temporary file behind.
        let _ = fs::remove_file(&temporary);
        return Err(Error::io(
            format!("could not write {}", path.display()),
            error,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use drumpad_model::config::LogLevel;
    use tempfile::tempdir;

    #[test]
    fn portable_layout_lives_beside_the_executable() {
        let temp = tempdir().expect("temp dir");
        fs::write(temp.path().join(PORTABLE_MARKER_FILE), b"portable").expect("marker");
        let paths = AppPaths::for_executable_dir(temp.path()).expect("paths");

        assert!(paths.is_portable());
        let root = temp.path().join(PORTABLE_DATA_DIR);
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
    fn installed_logs_and_crashes_are_siblings_of_data() {
        let data = Path::new(r"C:\Users\ezers\AppData\Local\Drumpad\Drumpad\data");
        let (logs, crashes) = installed_log_dirs(data);
        assert_eq!(
            logs,
            Path::new(r"C:\Users\ezers\AppData\Local\Drumpad\Drumpad\logs")
        );
        assert_eq!(
            crashes,
            Path::new(r"C:\Users\ezers\AppData\Local\Drumpad\Drumpad\crashes")
        );
    }

    #[test]
    fn marker_must_be_a_file() {
        let temp = tempdir().expect("temp dir");
        fs::create_dir(temp.path().join(PORTABLE_MARKER_FILE)).expect("marker dir");
        let error = AppPaths::for_executable_dir(temp.path()).expect_err("marker is a directory");
        assert!(matches!(error, Error::InvalidPortableMarker { .. }));
    }

    #[test]
    fn ensure_creates_the_directory_tree_once() {
        let temp = tempdir().expect("temp dir");
        let paths = AppPaths::rooted(temp.path().join("root"), true);
        paths.ensure().expect("first ensure");
        paths.ensure().expect("second ensure is a no-op");

        for directory in [
            paths.config_dir(),
            paths.data_dir(),
            paths.cache_dir(),
            paths.log_dir(),
            paths.crash_dir(),
        ] {
            assert!(directory.is_dir(), "{} should exist", directory.display());
        }
    }

    #[test]
    fn settings_round_trip_and_replace_atomically() {
        let temp = tempdir().expect("temp dir");
        let store = SettingsStore::new(temp.path().join("config").join(SETTINGS_FILE_NAME));

        let mut settings = AppSettings::default();
        settings.logging.level = LogLevel::Debug;
        settings.logging.retention_days = 30;
        store.save(&settings).expect("save");
        assert_eq!(store.load().expect("load"), settings);

        settings.ui.scale = 1.5;
        store.save(&settings).expect("replace");
        assert_eq!(store.load().expect("load replaced"), settings);

        let leftovers: Vec<_> = fs::read_dir(temp.path().join("config"))
            .expect("read config dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files must not survive a save"
        );
    }

    #[test]
    fn missing_settings_load_as_defaults_and_first_launch_writes_them() {
        let temp = tempdir().expect("temp dir");
        let store = SettingsStore::new(temp.path().join("settings.json"));

        assert_eq!(store.load().expect("load missing"), AppSettings::default());
        assert!(!store.path().exists());

        let created = store.load_or_create().expect("create");
        assert_eq!(created, AppSettings::default());
        assert!(store.path().is_file());
        assert_eq!(store.load().expect("load created"), AppSettings::default());
    }

    #[test]
    fn invalid_values_are_not_written() {
        let temp = tempdir().expect("temp dir");
        let store = SettingsStore::new(temp.path().join("settings.json"));

        let mut settings = AppSettings::default();
        settings.logging.retention_days = 0;
        let error = store.save(&settings).expect_err("invalid retention");
        assert!(matches!(
            error,
            Error::Config(drumpad_model::error::ConfigError::InvalidLogRetention { .. })
        ));
        assert!(!store.path().exists());
    }

    #[test]
    fn corrupt_settings_are_reported_not_overwritten() {
        let temp = tempdir().expect("temp dir");
        let path = temp.path().join("settings.json");
        fs::write(&path, b"{ not json").expect("write corrupt");
        let store = SettingsStore::new(&path);

        let error = store.load().expect_err("corrupt settings fail");
        assert!(matches!(error, Error::Json { .. }));
        assert_eq!(fs::read(&path).expect("still there"), b"{ not json");
    }

    #[test]
    fn future_schema_versions_are_rejected() {
        let temp = tempdir().expect("temp dir");
        let path = temp.path().join("settings.json");
        fs::write(
            &path,
            format!(r#"{{"schema_version":{}}}"#, SETTINGS_SCHEMA_VERSION + 1),
        )
        .expect("write future settings");

        let error = SettingsStore::new(&path)
            .load()
            .expect_err("future rejected");
        assert!(matches!(error, Error::UnsupportedSchemaVersion { .. }));
    }

    #[test]
    fn invalid_values_in_a_file_are_rejected() {
        let error =
            parse_settings(r#"{"schema_version":1,"logging":{"level":"info","retention_days":0}}"#)
                .expect_err("retention rejected");
        assert!(matches!(
            error,
            Error::Config(drumpad_model::error::ConfigError::InvalidLogRetention { .. })
        ));
    }
}
