//! Kit library: first-run defaults, CRUD, quarantine, and on-disk layout.
//!
//! Layout under the library root:
//!
//! ```text
//! kits/
//!   <slug>-<kit-id-prefix>/
//!     kit.json
//!     samples/...
//!     .backups/kit.bak1.json...
//!   active-kit
//!   quarantine/<broken-kit>-<timestamp>/
//! ```
//!
//! Every manifest write goes through [`super::persistence::atomic_write`], and
//! every overwrite keeps rotating backups of the previous manifest.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::demo::{self, DemoError};
use super::format::{self, FormatError};
use super::migration::MigrationReport;
use super::model::{Kit, KitId, KitMetadata, unix_time_ms};
use super::persistence::{
    self, DEFAULT_BACKUP_COUNT, LoadError, LoadedKit, SaveError, save_kit_atomic,
    save_kit_with_backup,
};
use super::repair::{RepairReport, repair_kit};
use super::validation::{ValidationError, validate_kit, validate_kit_report};

const MANIFEST: &str = "kit.json";
const ACTIVE_FILE: &str = "active-kit";
const QUARANTINE_DIR: &str = "quarantine";
const BACKUP_DIR: &str = ".backups";

/// A rotating backup of a kit manifest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BackupInfo {
    /// Rotation index, 1 = newest.
    pub index: usize,
    /// Backup file.
    pub path: PathBuf,
    /// Backup modification time in Unix milliseconds.
    pub modified_unix_ms: Option<u64>,
}

/// A lightweight description of a kit on disk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KitSummary {
    /// Stable kit id.
    pub id: KitId,
    /// Display name.
    pub name: String,
    /// Folder name under the library root.
    pub dir_name: String,
    /// Last modification time in Unix milliseconds.
    pub modified_unix_ms: Option<u64>,
}

/// Result of deleting a kit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeleteResult {
    /// Kit that was removed.
    pub deleted: KitId,
    /// Kit that became active when the deleted kit was active.
    pub fallback: Option<KitSummary>,
    /// True when the active selection changed.
    pub active_changed: bool,
}

/// Errors from the kit library.
#[derive(Debug, Error)]
pub enum LibraryError {
    /// Filesystem operation failed.
    #[error("library I/O error at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A manifest could not be parsed.
    #[error("library format error at {path}: {source}")]
    Format {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: FormatError,
    },
    /// A manifest could not be read.
    #[error(transparent)]
    Load(#[from] LoadError),
    /// A manifest could not be written.
    #[error(transparent)]
    Save(#[from] SaveError),
    /// The kit failed validation.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// No kit with that id exists.
    #[error("kit {0} was not found")]
    NotFound(KitId),
    /// A kit name was empty or too long.
    #[error("invalid kit name: {0}")]
    InvalidName(String),
    /// A corrupt kit was moved out of the library.
    #[error("kit {kit} was corrupt and moved to {}: {reason}", quarantined_to.display())]
    Quarantined {
        /// Kit that failed to load.
        kit: KitId,
        /// Original folder.
        original: PathBuf,
        /// New quarantine folder.
        quarantined_to: PathBuf,
        /// Why it was quarantined.
        reason: String,
    },
    /// A parseable kit violates a hard invariant and must be repaired.
    #[error("kit {id} failed validation: {}", .issues.join("; "))]
    Invalid {
        /// Kit that failed validation.
        id: KitId,
        /// Error messages.
        issues: Vec<String>,
    },
    /// Demo-kit installation failed.
    #[error(transparent)]
    Demo(#[from] DemoError),
}

fn io_error(path: &Path, source: std::io::Error) -> LibraryError {
    LibraryError::Io {
        path: path.to_owned(),
        source,
    }
}

fn short_id(id: KitId) -> String {
    id.as_string().chars().take(8).collect()
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "kit".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn validate_name(name: &str) -> Result<String, LibraryError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(LibraryError::InvalidName(
            "name must not be empty".to_owned(),
        ));
    }
    if trimmed.chars().count() > 128 {
        return Err(LibraryError::InvalidName(
            "name must be 128 characters or fewer".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn dir_name_for(name: &str, id: KitId) -> String {
    format!("{}-{}", slugify(name), short_id(id))
}

fn copy_file(from: &Path, to: &Path) -> Result<(), LibraryError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    fs::copy(from, to).map_err(|error| io_error(from, error))?;
    Ok(())
}

/// A directory-backed kit library.
#[derive(Clone, Debug)]
pub struct KitLibrary {
    root: PathBuf,
    active: Option<KitId>,
}

impl KitLibrary {
    /// Open (or create) a library at `root`.
    ///
    /// On first run the folder is created, a silent default kit is written,
    /// and it becomes active.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| io_error(&root, error))?;
        let mut library = Self { root, active: None };

        let summaries = library.list()?;
        if summaries.is_empty() {
            let kit = library.create("Default Kit")?;
            library.set_active(kit.id)?;
        } else {
            let saved = library.read_active_file()?;
            let active = saved
                .filter(|id| summaries.iter().any(|summary| summary.id == *id))
                .or_else(|| summaries.first().map(|summary| summary.id));
            if let Some(active) = active {
                library.set_active(active)?;
            }
        }
        Ok(library)
    }

    /// Library root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Currently active kit id.
    pub fn active_kit_id(&self) -> Option<KitId> {
        self.active
    }

    /// List readable kits, ordered by name.
    pub fn list(&self) -> Result<Vec<KitSummary>, LibraryError> {
        let entries = fs::read_dir(&self.root).map_err(|error| io_error(&self.root, error))?;
        let mut summaries = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&self.root, error))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == QUARANTINE_DIR {
                continue;
            }
            let manifest = path.join(MANIFEST);
            if !manifest.is_file() {
                continue;
            }
            let bytes = match fs::read(&manifest) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let parsed = match format::parse_kit_bytes(&bytes) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };
            summaries.push(KitSummary {
                id: parsed.kit.id,
                name: parsed.kit.metadata.name.clone(),
                dir_name: name.into_owned(),
                modified_unix_ms: parsed.kit.metadata.modified_unix_ms,
            });
        }
        summaries.sort_by_key(|summary| summary.name.to_lowercase());
        Ok(summaries)
    }

    /// Ensure at least one kit exists and is active.
    pub fn ensure_default_kit(&mut self) -> Result<Kit, LibraryError> {
        if let Some(summary) = self.list()?.first() {
            if self.active.is_none() {
                self.set_active(summary.id)?;
            }
            return self.load(summary.id);
        }
        let kit = self.create("Default Kit")?;
        self.set_active(kit.id)?;
        Ok(kit)
    }

    /// Create a new kit folder and manifest.
    pub fn create(&mut self, name: &str) -> Result<Kit, LibraryError> {
        let name = validate_name(name)?;
        let mut kit = Kit::new(name);
        kit.touch(unix_time_ms());
        validate_kit(&kit)?;
        self.write_new_kit(&kit)?;
        Ok(kit)
    }

    /// Persist an existing kit, rotating its previous manifest into backups.
    pub fn save(&mut self, kit: &mut Kit) -> Result<(), LibraryError> {
        validate_kit(kit)?;
        let dir = self
            .find_dir(kit.id)?
            .ok_or(LibraryError::NotFound(kit.id))?;
        kit.touch(unix_time_ms());
        save_kit_with_backup(
            &dir.join(MANIFEST),
            kit,
            &dir.join(BACKUP_DIR),
            DEFAULT_BACKUP_COUNT,
        )?;
        self.set_active(kit.id)?;
        Ok(())
    }

    /// Load a kit by id, quarantining it if the manifest is corrupt.
    pub fn load(&self, id: KitId) -> Result<Kit, LibraryError> {
        self.load_with_report(id).map(|(kit, _)| kit)
    }

    /// Read a manifest, quarantining structurally corrupt kits but returning a
    /// typed error for documents written by a newer build.
    fn load_manifest(&self, id: KitId, dir: &Path) -> Result<LoadedKit, LibraryError> {
        let manifest = dir.join(MANIFEST);
        match persistence::load_kit_file(&manifest) {
            Ok(loaded) => Ok(loaded),
            Err(LoadError::Format { path, source }) => {
                if matches!(&source, FormatError::FutureVersion { .. }) {
                    // A newer kit is not corrupt; keep it where the newer
                    // build can still find it. Quarantine is only for data we
                    // cannot interpret at all.
                    return Err(LibraryError::Format { path, source });
                }
                let reason = source.to_string();
                let quarantined_to = self.quarantine(dir)?;
                Err(LibraryError::Quarantined {
                    kit: id,
                    original: dir.to_owned(),
                    quarantined_to,
                    reason,
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Load a kit, returning migration details when the document was upgraded.
    pub fn load_with_report(
        &self,
        id: KitId,
    ) -> Result<(Kit, Option<MigrationReport>), LibraryError> {
        let dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let manifest = dir.join(MANIFEST);
        let loaded = self.load_manifest(id, &dir)?;

        if let Some(report) = &loaded.migration {
            persistence::backup_before_migration(
                &manifest,
                &dir.join(BACKUP_DIR),
                report.from_version,
            )?;
            save_kit_atomic(&manifest, &loaded.kit)?;
        }
        let report = validate_kit_report(&loaded.kit);
        if !report.is_valid() {
            return Err(LibraryError::Invalid {
                id,
                issues: report.messages(),
            });
        }
        Ok((loaded.kit, loaded.migration))
    }

    /// Load a kit, repair structural problems, and persist the repair.
    ///
    /// This is the explicit recovery path. A plain [`Self::load`] never edits
    /// a kit behind the caller's back.
    pub fn load_repairing(&self, id: KitId) -> Result<(Kit, RepairReport), LibraryError> {
        let dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let manifest = dir.join(MANIFEST);
        let mut loaded = self.load_manifest(id, &dir)?;
        if loaded.kit.id != id {
            return Err(LibraryError::Invalid {
                id,
                issues: vec![format!("manifest id is {}", loaded.kit.id)],
            });
        }
        if let Some(report) = &loaded.migration {
            persistence::backup_before_migration(
                &manifest,
                &dir.join(BACKUP_DIR),
                report.from_version,
            )?;
        }
        let repair = repair_kit(&mut loaded.kit);
        if !repair.is_valid() {
            return Err(LibraryError::Invalid {
                id,
                issues: repair.remaining.messages(),
            });
        }
        save_kit_with_backup(
            &manifest,
            &loaded.kit,
            &dir.join(BACKUP_DIR),
            DEFAULT_BACKUP_COUNT,
        )?;
        Ok((loaded.kit, repair))
    }

    /// List rotating manifest backups, newest first.
    pub fn list_backups(&self, id: KitId) -> Result<Vec<BackupInfo>, LibraryError> {
        let dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let backup_dir = dir.join(BACKUP_DIR);
        let mut backups = Vec::new();
        let entries = match fs::read_dir(&backup_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(backups),
            Err(error) => return Err(io_error(&backup_dir, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&backup_dir, error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name
                .strip_prefix("kit.bak")
                .and_then(|rest| rest.strip_suffix(".json"))
            else {
                continue;
            };
            let Ok(index) = rest.parse::<usize>() else {
                continue;
            };
            let path = entry.path();
            let modified_unix_ms = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64);
            backups.push(BackupInfo {
                index,
                path,
                modified_unix_ms,
            });
        }
        backups.sort_by_key(|backup| backup.index);
        Ok(backups)
    }

    /// Restore a rotating backup, preserving the current manifest first.
    pub fn restore_backup(&mut self, id: KitId, index: usize) -> Result<Kit, LibraryError> {
        let dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let backup_path = dir.join(BACKUP_DIR).join(format!("kit.bak{index}.json"));
        if !backup_path.is_file() {
            return Err(LibraryError::NotFound(id));
        }
        let mut loaded = persistence::load_kit_file(&backup_path)?;
        if loaded.kit.id != id {
            return Err(LibraryError::Invalid {
                id,
                issues: vec![format!("backup id is {}", loaded.kit.id)],
            });
        }
        validate_kit(&loaded.kit)?;
        let manifest = dir.join(MANIFEST);
        if manifest.is_file() {
            let preserved = dir
                .join(BACKUP_DIR)
                .join(format!("pre-restore-{}.json", unix_time_ms()));
            fs::copy(&manifest, &preserved).map_err(|error| io_error(&manifest, error))?;
        }
        loaded.kit.touch(unix_time_ms());
        save_kit_atomic(&manifest, &loaded.kit)?;
        Ok(loaded.kit)
    }

    /// Install the embedded demo kit as a new library entry and activate it.
    pub fn install_demo_kit(&mut self) -> Result<Kit, LibraryError> {
        let kit = demo::demo_kit().map_err(|source| LibraryError::Format {
            path: self.root.join("demo-kit"),
            source,
        })?;
        let dir = self.unique_dir(&dir_name_for(&kit.metadata.name, kit.id));
        demo::install_demo_kit(&dir)?;
        self.set_active(kit.id)?;
        Ok(kit)
    }

    /// Load the active kit, if one is selected.
    pub fn load_active(&self) -> Result<Option<Kit>, LibraryError> {
        self.active.map(|id| self.load(id)).transpose()
    }

    /// Rename a kit, moving its folder so the on-disk name stays meaningful.
    pub fn rename(&mut self, id: KitId, new_name: &str) -> Result<Kit, LibraryError> {
        let new_name = validate_name(new_name)?;
        let mut kit = self.load(id)?;
        let old_dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let desired = dir_name_for(&new_name, id);
        kit.metadata.name = new_name;
        kit.touch(unix_time_ms());
        validate_kit(&kit)?;

        if old_dir.file_name().and_then(|name| name.to_str()) == Some(desired.as_str()) {
            save_kit_with_backup(
                &old_dir.join(MANIFEST),
                &kit,
                &old_dir.join(BACKUP_DIR),
                DEFAULT_BACKUP_COUNT,
            )?;
            return Ok(kit);
        }

        let new_dir = self.unique_dir(&desired);
        fs::rename(&old_dir, &new_dir).map_err(|error| io_error(&old_dir, error))?;
        if let Err(error) = save_kit_atomic(&new_dir.join(MANIFEST), &kit) {
            let _ = fs::rename(&new_dir, &old_dir);
            return Err(error.into());
        }
        Ok(kit)
    }

    /// Duplicate a kit, copying every referenced sample.
    pub fn duplicate(&mut self, id: KitId, new_name: Option<&str>) -> Result<Kit, LibraryError> {
        let source = self.load(id)?;
        let source_dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let name = match new_name {
            Some(name) => validate_name(name)?,
            None => validate_name(&format!("{} Copy", source.metadata.name))?,
        };

        let mut copy = Kit {
            format_version: source.format_version,
            id: KitId::new(),
            metadata: KitMetadata {
                name,
                created_unix_ms: None,
                modified_unix_ms: None,
                ..source.metadata.clone()
            },
            pads: source.pads.clone(),
        };
        copy.touch(unix_time_ms());
        validate_kit(&copy)?;

        let target_dir = self.write_new_kit(&copy)?;
        for sample in copy.samples() {
            let from = sample.resolve(&source_dir);
            if from.is_file() {
                copy_file(&from, &sample.resolve(&target_dir))?;
            }
        }
        Ok(copy)
    }

    /// Delete a kit. When it is active, select a safe fallback first.
    pub fn delete(&mut self, id: KitId) -> Result<DeleteResult, LibraryError> {
        let dir = self.find_dir(id)?.ok_or(LibraryError::NotFound(id))?;
        let was_active = self.active == Some(id);
        let mut fallback = None;
        let mut active_changed = false;

        if was_active {
            let candidates: Vec<_> = self
                .list()?
                .into_iter()
                .filter(|summary| summary.id != id)
                .collect();
            if let Some(summary) = candidates.first() {
                self.set_active(summary.id)?;
                fallback = Some(summary.clone());
            } else {
                let kit = self.create("Default Kit")?;
                self.set_active(kit.id)?;
                fallback = self
                    .list()?
                    .into_iter()
                    .find(|summary| summary.id == kit.id);
            }
            active_changed = true;
        }

        fs::remove_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
        Ok(DeleteResult {
            deleted: id,
            fallback,
            active_changed,
        })
    }

    /// Select the active kit and persist the choice.
    pub fn set_active(&mut self, id: KitId) -> Result<(), LibraryError> {
        if self.find_dir(id)?.is_none() {
            return Err(LibraryError::NotFound(id));
        }
        self.active = Some(id);
        persistence::atomic_write(&self.root.join(ACTIVE_FILE), id.as_string().as_bytes())?;
        Ok(())
    }

    /// Folder for a kit id.
    ///
    /// Returns [`LibraryError::NotFound`] when the kit folder is missing. This
    /// is the folder that [`super::model::SampleRef::resolve`] joins to.
    pub fn kit_dir(&self, id: KitId) -> Result<PathBuf, LibraryError> {
        self.find_dir(id)?.ok_or(LibraryError::NotFound(id))
    }

    fn write_new_kit(&self, kit: &Kit) -> Result<PathBuf, LibraryError> {
        let dir = self.unique_dir(&dir_name_for(&kit.metadata.name, kit.id));
        fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
        save_kit_atomic(&dir.join(MANIFEST), kit)?;
        Ok(dir)
    }

    fn unique_dir(&self, preferred: &str) -> PathBuf {
        let mut candidate = self.root.join(preferred);
        if !candidate.exists() {
            return candidate;
        }
        for suffix in 2..=9999 {
            candidate = self.root.join(format!("{preferred}-{suffix}"));
            if !candidate.exists() {
                return candidate;
            }
        }
        self.root.join(format!("{preferred}-{}", unix_time_ms()))
    }

    fn find_dir(&self, id: KitId) -> Result<Option<PathBuf>, LibraryError> {
        let suffix = format!("-{}", short_id(id));
        let entries = fs::read_dir(&self.root).map_err(|error| io_error(&self.root, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&self.root, error))?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == QUARANTINE_DIR {
                continue;
            }
            let manifest = entry.path().join(MANIFEST);
            if !manifest.is_file() {
                continue;
            }
            if name.ends_with(&suffix) {
                return Ok(Some(entry.path()));
            }
            let bytes = match fs::read(&manifest) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            if let Ok(parsed) = format::parse_kit_bytes(&bytes)
                && parsed.kit.id == id
            {
                return Ok(Some(entry.path()));
            }
        }
        Ok(None)
    }

    fn quarantine(&self, dir: &Path) -> Result<PathBuf, LibraryError> {
        let root = self.root.join(QUARANTINE_DIR);
        fs::create_dir_all(&root).map_err(|error| io_error(&root, error))?;
        let stem = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "kit".to_owned());
        let destination = root.join(format!("{stem}-{}", unix_time_ms()));
        fs::rename(dir, &destination).map_err(|error| io_error(dir, error))?;
        let reason = destination.join("quarantine-reason.txt");
        let _ = fs::write(
            &reason,
            "manifest failed to parse; moved aside by KitLibrary\n",
        );
        Ok(destination)
    }

    fn read_active_file(&self) -> Result<Option<KitId>, LibraryError> {
        let path = self.root.join(ACTIVE_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(text.trim().parse::<KitId>().ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(&path, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{ContentHash, Layer, RelativeSamplePath, SampleRef};
    use std::fs;

    fn library() -> (tempfile::TempDir, KitLibrary) {
        let dir = tempfile::tempdir().unwrap();
        let library = KitLibrary::open(dir.path()).unwrap();
        (dir, library)
    }

    #[test]
    fn first_run_creates_and_activates_a_default_kit() {
        let (dir, library) = library();
        let summaries = library.list().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "Default Kit");
        assert_eq!(library.active_kit_id(), Some(summaries[0].id));
        assert!(dir.path().join(ACTIVE_FILE).is_file());
        assert!(library.load_active().unwrap().is_some());
    }

    #[test]
    fn crud_round_trip_and_backups() {
        let (_dir, mut library) = library();
        let mut kit = library.create("Rock Kit").unwrap();
        assert_eq!(library.list().unwrap().len(), 2);
        for index in 0..4 {
            kit.metadata.description = Some(format!("edit {index}"));
            library.save(&mut kit).unwrap();
        }
        let dir = library.kit_dir(kit.id).unwrap();
        assert!(dir.join(BACKUP_DIR).join("kit.bak1.json").is_file());
        let reloaded = library.load(kit.id).unwrap();
        assert_eq!(reloaded.metadata.description.as_deref(), Some("edit 3"));

        let duplicate = library.duplicate(kit.id, Some("Rock Kit 2")).unwrap();
        assert_ne!(duplicate.id, kit.id);
        assert_eq!(library.list().unwrap().len(), 3);

        let renamed = library.rename(kit.id, "Metal Kit").unwrap();
        assert_eq!(renamed.metadata.name, "Metal Kit");
        assert!(
            library
                .kit_dir(kit.id)
                .unwrap()
                .ends_with("metal-kit-".to_owned() + &short_id(kit.id))
        );
    }

    #[test]
    fn deleting_active_kit_selects_fallback() {
        let (_dir, mut library) = library();
        let second = library.create("Second").unwrap();
        library.set_active(second.id).unwrap();
        let result = library.delete(second.id).unwrap();
        assert!(result.active_changed);
        assert!(result.fallback.is_some());
        assert_ne!(library.active_kit_id(), Some(second.id));
        assert!(library.load(second.id).is_err());
    }

    #[test]
    fn deleting_the_only_kit_creates_a_default() {
        let (_dir, mut library) = library();
        let active = library.active_kit_id().unwrap();
        let result = library.delete(active).unwrap();
        assert!(result.active_changed);
        assert!(result.fallback.is_some());
        assert!(library.load_active().unwrap().is_some());
    }

    #[test]
    fn corrupt_manifest_is_quarantined_not_fatal() {
        let (dir, mut library) = library();
        let kit = library.create("Fragile").unwrap();
        let kit_dir = library.kit_dir(kit.id).unwrap();
        fs::write(kit_dir.join(MANIFEST), b"{not json").unwrap();
        let error = library.load(kit.id).unwrap_err();
        assert!(matches!(error, LibraryError::Quarantined { .. }));
        assert!(!kit_dir.exists());
        let quarantine = dir.path().join(QUARANTINE_DIR);
        assert!(quarantine.is_dir());
        // The library keeps working and still lists other kits.
        assert!(!library.list().unwrap().is_empty());
    }

    #[test]
    fn future_version_is_refused_without_quarantine() {
        let (_dir, mut library) = library();
        let kit = library.create("Future").unwrap();
        let kit_dir = library.kit_dir(kit.id).unwrap();
        let manifest = kit_dir.join(MANIFEST);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(2);
        fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let error = library.load(kit.id).unwrap_err();
        assert!(matches!(error, LibraryError::Format { .. }));
        assert!(manifest.is_file(), "a newer kit must not be quarantined");
        assert!(!library.root().join(QUARANTINE_DIR).exists());
    }

    #[test]
    fn duplicate_copies_samples() {
        let (_dir, mut library) = library();
        let mut kit = library.create("Sample Kit").unwrap();
        let kit_dir = library.kit_dir(kit.id).unwrap();
        let sample_path = kit_dir.join("samples/kick.wav");
        fs::create_dir_all(sample_path.parent().unwrap()).unwrap();
        fs::write(&sample_path, b"fake wav").unwrap();
        kit.pads[0].layers.push(Layer::with_sample(
            Default::default(),
            SampleRef::new(
                RelativeSamplePath::new("samples/kick.wav").unwrap(),
                ContentHash::of_bytes(b"fake wav"),
            ),
        ));
        library.save(&mut kit).unwrap();

        let copy = library.duplicate(kit.id, Some("Copy Kit")).unwrap();
        let copy_dir = library.kit_dir(copy.id).unwrap();
        assert_eq!(
            fs::read(copy_dir.join("samples/kick.wav")).unwrap(),
            b"fake wav"
        );
    }

    #[test]
    fn active_choice_survives_reopen() {
        let (dir, mut library) = library();
        let kit = library.create("Chosen").unwrap();
        library.set_active(kit.id).unwrap();
        let reopened = KitLibrary::open(dir.path()).unwrap();
        assert_eq!(reopened.active_kit_id(), Some(kit.id));
    }

    #[test]
    fn invalid_manifest_is_rejected_and_repairable() {
        let (_dir, mut library) = library();
        let kit = library.create("Needs Repair").unwrap();
        let kit_dir = library.kit_dir(kit.id).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(kit_dir.join(MANIFEST)).unwrap()).unwrap();
        value["pads"][0]["pan"] = serde_json::json!(4.0);
        fs::write(
            kit_dir.join(MANIFEST),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();

        let error = library.load(kit.id).unwrap_err();
        assert!(matches!(error, LibraryError::Invalid { .. }));
        let (repaired, report) = library.load_repairing(kit.id).unwrap();
        assert_eq!(repaired.pads[0].pan, 1.0);
        assert!(!report.is_clean());
        assert!(library.load(kit.id).is_ok());
    }

    #[test]
    fn backup_can_be_restored() {
        let (_dir, mut library) = library();
        let mut kit = library.create("Restore Me").unwrap();
        for revision in 0..4 {
            kit.metadata.description = Some(format!("revision {revision}"));
            library.save(&mut kit).unwrap();
        }
        let backups = library.list_backups(kit.id).unwrap();
        assert!(backups.len() >= 3);
        assert_eq!(backups[0].index, 1);
        let restored = library.restore_backup(kit.id, 2).unwrap();
        assert_eq!(restored.metadata.description.as_deref(), Some("revision 1"));
        assert_eq!(
            library
                .load(kit.id)
                .unwrap()
                .metadata
                .description
                .as_deref(),
            Some("revision 1")
        );
    }

    #[test]
    fn demo_kit_installs_into_the_library() {
        let (_dir, mut library) = library();
        let kit = library.install_demo_kit().unwrap();
        assert_eq!(kit.metadata.name, "Drumpad Demo");
        assert_eq!(library.active_kit_id(), Some(kit.id));
        let loaded = library.load_active().unwrap().unwrap();
        assert_eq!(loaded.pads.len(), 16);
        assert!(
            library
                .kit_dir(kit.id)
                .unwrap()
                .join("samples/kick.wav")
                .is_file()
        );
    }

    #[test]
    fn invalid_names_are_rejected() {
        let (_dir, mut library) = library();
        assert!(matches!(
            library.create("   "),
            Err(LibraryError::InvalidName(_))
        ));
        assert!(matches!(
            library.create(&"x".repeat(129)),
            Err(LibraryError::InvalidName(_))
        ));
    }

    #[test]
    fn v0_kit_is_migrated_and_backed_up() {
        let (dir, mut library) = library();
        let kit = library.create("Legacy Target").unwrap();
        let kit_dir = library.kit_dir(kit.id).unwrap();
        let legacy = serde_json::json!({
            "version": 0,
            "name": "Legacy Target",
            "pads": [{"slot": 0, "sample_path": "samples/kick.wav"}]
        });
        fs::write(
            kit_dir.join(MANIFEST),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        let (migrated, report) = library.load_with_report(kit.id).unwrap();
        assert_eq!(migrated.format_version, 1);
        let report = report.unwrap();
        assert_eq!(report.from_version, 0);
        assert_eq!(
            migrated.pads[0].layers[0]
                .sample
                .as_ref()
                .unwrap()
                .path
                .as_str(),
            "samples/kick.wav"
        );
        assert!(
            kit_dir
                .join(BACKUP_DIR)
                .join("pre-migration-v0.json")
                .is_file()
        );

        // Reopening a fresh library still finds the kit by its folder prefix.
        drop(library);
        let reopened = KitLibrary::open(dir.path()).unwrap();
        assert!(reopened.load(kit.id).is_ok());
    }
}
