//! Crash-safe persistence primitives: atomic writes, rotating backups, and
//! debounced autosave bookkeeping.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use thiserror::Error;

use super::format::{self, FormatError};
use super::migration::MigrationReport;
use super::model::{Kit, unix_time_ms};

/// Default number of rotating backups kept beside a kit manifest.
pub const DEFAULT_BACKUP_COUNT: usize = 5;
/// Default quiet period before an autosave fires.
pub const DEFAULT_AUTOSAVE_DEBOUNCE: Duration = Duration::from_secs(1);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Errors while writing a kit.
#[derive(Debug, Error)]
pub enum SaveError {
    /// Filesystem write failed.
    #[error("failed to write {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Serialization failed.
    #[error("failed to serialize kit: {0}")]
    Format(#[from] FormatError),
}

/// Errors while reading a kit.
#[derive(Debug, Error)]
pub enum LoadError {
    /// Filesystem read failed.
    #[error("failed to read {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The document was invalid or from the future.
    #[error("failed to parse {path}: {source}")]
    Format {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: FormatError,
    },
}

/// A kit loaded from disk, including migration details when relevant.
#[derive(Debug)]
pub struct LoadedKit {
    /// Current-version kit.
    pub kit: Kit,
    /// Migration report when the source document was upgraded.
    pub migration: Option<MigrationReport>,
}

fn save_io(path: &Path, source: std::io::Error) -> SaveError {
    SaveError::Io {
        path: path.to_owned(),
        source,
    }
}

fn load_io(path: &Path, source: std::io::Error) -> LoadError {
    LoadError::Io {
        path: path.to_owned(),
        source,
    }
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("kit.json");
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{file_name}.tmp-{}-{counter}", std::process::id()))
}

/// Write bytes to `path` via a same-directory temp file, fsync, and atomic
/// rename. A crash before the rename leaves the previous file untouched.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| save_io(parent, error))?;
    let temp = temp_path_for(path);

    let write_result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp);
        return Err(save_io(&temp, error));
    }

    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(save_io(path, error));
    }

    // Best effort directory fsync (not available on Windows).
    #[cfg(unix)]
    {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// Serialize and atomically write a kit manifest.
pub fn save_kit_atomic(path: &Path, kit: &Kit) -> Result<(), SaveError> {
    let json = format::to_json_pretty(kit)?;
    atomic_write(path, json.as_bytes())
}

/// Save a kit manifest, rotating the previous version into `backup_dir` first.
pub fn save_kit_with_backup(
    path: &Path,
    kit: &Kit,
    backup_dir: &Path,
    keep: usize,
) -> Result<(), SaveError> {
    if path.exists() {
        rotate_backups(path, backup_dir, keep)?;
    }
    save_kit_atomic(path, kit)
}

/// Copy the current manifest to `kit.bak1.json`, shifting older backups.
pub fn rotate_backups(path: &Path, backup_dir: &Path, keep: usize) -> Result<(), SaveError> {
    if keep == 0 || !path.exists() {
        return Ok(());
    }
    fs::create_dir_all(backup_dir).map_err(|error| save_io(backup_dir, error))?;

    for index in (1..keep).rev() {
        let from = backup_dir.join(format!("kit.bak{index}.json"));
        let to = backup_dir.join(format!("kit.bak{}.json", index + 1));
        if from.exists() {
            if to.exists() {
                let _ = fs::remove_file(&to);
            }
            fs::rename(&from, &to).map_err(|error| save_io(&from, error))?;
        }
    }

    let newest = backup_dir.join("kit.bak1.json");
    fs::copy(path, &newest).map_err(|error| save_io(&newest, error))?;
    Ok(())
}

/// Read and parse a kit manifest, applying migrations when needed.
pub fn load_kit_file(path: &Path) -> Result<LoadedKit, LoadError> {
    let bytes = fs::read(path).map_err(|error| load_io(path, error))?;
    let parsed = format::parse_kit_bytes(&bytes).map_err(|source| LoadError::Format {
        path: path.to_owned(),
        source,
    })?;
    Ok(LoadedKit {
        kit: parsed.kit,
        migration: parsed.migration,
    })
}

/// Move a corrupted manifest aside so the library can continue to operate.
pub fn backup_before_migration(
    path: &Path,
    backup_dir: &Path,
    from_version: u32,
) -> Result<PathBuf, SaveError> {
    fs::create_dir_all(backup_dir).map_err(|error| save_io(backup_dir, error))?;
    let destination = backup_dir.join(format!("pre-migration-v{from_version}.json"));
    fs::copy(path, &destination).map_err(|error| save_io(&destination, error))?;
    Ok(destination)
}

/// Debounced autosave state machine.
///
/// Edits reset the quiet period. [`AutosaveController::poll`] returns `true`
/// exactly once per settled batch so the caller can perform the actual save.
#[derive(Clone, Debug)]
pub struct AutosaveController {
    debounce: Duration,
    dirty: bool,
    pending_since: Option<Instant>,
    last_saved: Option<Instant>,
}

impl AutosaveController {
    /// Create a controller with the given quiet period.
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            dirty: false,
            pending_since: None,
            last_saved: None,
        }
    }

    /// Record an edit at `now`.
    pub fn note_edit(&mut self, now: Instant) {
        self.dirty = true;
        self.pending_since = Some(now);
    }

    /// True when there are unsaved edits.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Time of the most recent save, if any.
    pub fn last_saved(&self) -> Option<Instant> {
        self.last_saved
    }

    /// True when the debounce interval has elapsed since the last edit.
    pub fn poll(&mut self, now: Instant) -> bool {
        let Some(since) = self.pending_since else {
            return false;
        };
        if now.saturating_duration_since(since) >= self.debounce {
            self.pending_since = None;
            true
        } else {
            false
        }
    }

    /// Mark the persisted state current.
    pub fn mark_saved(&mut self, now: Instant) {
        self.dirty = false;
        self.pending_since = None;
        self.last_saved = Some(now);
    }
}

impl Default for AutosaveController {
    fn default() -> Self {
        Self::new(DEFAULT_AUTOSAVE_DEBOUNCE)
    }
}

/// Current time in Unix milliseconds.
pub fn now_ms() -> u64 {
    unix_time_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::Kit;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit.json");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files remain: {leftovers:?}");
    }

    #[test]
    fn failed_save_keeps_previous_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit.json");
        let kit = Kit::silent_default();
        save_kit_atomic(&path, &kit).unwrap();
        let before = fs::read(&path).unwrap();

        // A serialization failure must never reach the write/rename path.
        let bad_kit = Kit {
            metadata: crate::kit::KitMetadata {
                name: String::new(),
                ..kit.metadata.clone()
            },
            ..kit.clone()
        };
        assert!(bad_kit.validate().is_err());

        // Invalid JSON-like bytes cannot be produced by the serializer, so
        // simulate a failed temp write by targeting an unwritable directory.
        let missing_parent = path.join("not-a-directory");
        assert!(atomic_write(&missing_parent, b"x").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn backups_rotate_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kit.json");
        let backups = dir.path().join(".backups");
        atomic_write(&path, b"version-0").unwrap();
        for index in 1..4 {
            rotate_backups(&path, &backups, 3).unwrap();
            atomic_write(&path, format!("version-{index}").as_bytes()).unwrap();
        }
        assert_eq!(
            fs::read_to_string(backups.join("kit.bak1.json")).unwrap(),
            "version-2"
        );
        assert_eq!(
            fs::read_to_string(backups.join("kit.bak2.json")).unwrap(),
            "version-1"
        );
        assert_eq!(
            fs::read_to_string(backups.join("kit.bak3.json")).unwrap(),
            "version-0"
        );
        assert!(!backups.join("kit.bak4.json").exists());
    }

    #[test]
    fn autosave_debounces_edits() {
        let base = Instant::now();
        let mut controller = AutosaveController::new(Duration::from_millis(1500));
        controller.note_edit(base);
        assert!(!controller.poll(base + Duration::from_millis(500)));
        controller.note_edit(base + Duration::from_millis(1000));
        assert!(!controller.poll(base + Duration::from_millis(2000)));
        assert!(controller.poll(base + Duration::from_millis(2600)));
        assert!(!controller.poll(base + Duration::from_millis(4000)));
        assert!(controller.is_dirty());
        controller.mark_saved(base + Duration::from_millis(4100));
        assert!(!controller.is_dirty());
    }

    #[test]
    fn autosave_meets_two_second_budget() {
        let base = Instant::now();
        let mut controller = AutosaveController::default();
        controller.note_edit(base);
        assert!(controller.poll(base + Duration::from_millis(2000)));
    }
}
