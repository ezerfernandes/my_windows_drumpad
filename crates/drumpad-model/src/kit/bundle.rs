//! Portable kit bundles: folder and ZIP export/import.
//!
//! A bundle is a kit manifest plus every referenced sample. Export refuses to
//! silently omit missing audio. Import validates the manifest and paths,
//! reports missing files, and resolves destination name collisions by content
//! instead of overwriting.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;
use zip::write::SimpleFileOptions;

use super::format::{self, FormatError};
use super::library::KitLibrary;
use super::model::{ContentHash, Kit, KitId, RelativeSamplePath};
use super::validation::{ValidationError, validate_kit, validate_kit_report};

const MANIFEST: &str = "kit.json";
const MAX_ARCHIVE_BYTES: u64 = 1 << 30;

/// Summary of a completed export.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExportReport {
    /// Kit that was exported.
    pub kit_id: KitId,
    /// Number of sample files copied.
    pub samples_copied: usize,
    /// Total bytes of sample data copied.
    pub bytes_copied: u64,
    /// Relative paths that were written.
    pub written: Vec<String>,
}

/// Summary of a completed import.
#[derive(Clone, PartialEq, Debug)]
pub struct ImportReport {
    /// Imported kit, with paths rewritten if collisions were resolved.
    pub kit: Kit,
    /// Number of sample files copied.
    pub samples_copied: usize,
    /// Samples referenced by the manifest but absent from the bundle.
    pub missing: Vec<RelativeSamplePath>,
    /// Destination renames applied because of name collisions.
    pub renamed: Vec<(String, String)>,
}

/// Bundle export/import failures.
#[derive(Debug, Error)]
pub enum BundleError {
    /// Filesystem operation failed.
    #[error("bundle I/O error at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The manifest was invalid.
    #[error(transparent)]
    Format(#[from] FormatError),
    /// The manifest failed validation.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The kit references samples that are not on disk.
    #[error("bundle is missing {} sample(s): {}", .0.len(), .0.join(", "))]
    MissingSamples(Vec<String>),
    /// The destination already exists and is not empty.
    #[error("destination already exists: {}", .0.display())]
    DestinationExists(PathBuf),
    /// An archive entry tried to escape the extraction folder.
    #[error("unsafe archive entry: {0}")]
    UnsafeEntry(String),
    /// The archive could not be read or written.
    #[error("zip error: {0}")]
    Zip(String),
    /// A bundle manifest is not usable.
    #[error("invalid kit bundle: {0}")]
    InvalidBundle(String),
}

fn io_error(path: &Path, source: std::io::Error) -> BundleError {
    BundleError::Io {
        path: path.to_owned(),
        source,
    }
}

fn ensure_destination(dest: &Path) -> Result<(), BundleError> {
    if dest.exists() {
        let mut entries = fs::read_dir(dest).map_err(|error| io_error(dest, error))?;
        if entries.next().is_some() {
            return Err(BundleError::DestinationExists(dest.to_owned()));
        }
    } else {
        fs::create_dir_all(dest).map_err(|error| io_error(dest, error))?;
    }
    Ok(())
}

fn collect_sources(
    kit_dir: &Path,
    kit: &Kit,
) -> Result<Vec<(RelativeSamplePath, PathBuf)>, BundleError> {
    let mut missing = Vec::new();
    let mut sources = Vec::new();
    for sample in kit.samples() {
        let path = sample.resolve(kit_dir);
        if path.is_file() {
            sources.push((sample.path.clone(), path));
        } else {
            missing.push(sample.path.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(BundleError::MissingSamples(missing));
    }
    Ok(sources)
}

/// Export a kit folder into a portable folder bundle.
pub fn export_kit_folder(
    kit: &Kit,
    kit_dir: &Path,
    dest_dir: &Path,
) -> Result<ExportReport, BundleError> {
    validate_kit(kit)?;
    let sources = collect_sources(kit_dir, kit)?;
    ensure_destination(dest_dir)?;

    let mut written = Vec::new();
    let mut bytes_copied = 0u64;
    for (relative, source) in &sources {
        let target = relative.resolve(dest_dir);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::copy(source, &target).map_err(|error| io_error(source, error))?;
        bytes_copied += fs::metadata(&target)
            .map_err(|error| io_error(&target, error))?
            .len();
        written.push(relative.to_string());
    }

    let manifest = format::to_json_pretty(kit)?;
    fs::write(dest_dir.join(MANIFEST), manifest).map_err(|error| io_error(dest_dir, error))?;

    // Carry license/attribution files if the kit folder has them.
    for extra in ["LICENSES.md", "CREDITS.md"] {
        let source = kit_dir.join(extra);
        if source.is_file() {
            fs::copy(&source, dest_dir.join(extra)).map_err(|error| io_error(&source, error))?;
        }
    }

    Ok(ExportReport {
        kit_id: kit.id,
        samples_copied: sources.len(),
        bytes_copied,
        written,
    })
}

/// Export a kit into a ZIP archive.
pub fn export_kit_archive(
    kit: &Kit,
    kit_dir: &Path,
    dest_zip: &Path,
) -> Result<ExportReport, BundleError> {
    validate_kit(kit)?;
    let sources = collect_sources(kit_dir, kit)?;
    if let Some(parent) = dest_zip.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    let file = fs::File::create(dest_zip).map_err(|error| io_error(dest_zip, error))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let manifest = format::to_json_pretty(kit)?;
    writer
        .start_file(MANIFEST, options)
        .map_err(|error| BundleError::Zip(error.to_string()))?;
    writer
        .write_all(manifest.as_bytes())
        .map_err(|error| io_error(dest_zip, error))?;

    let mut written = Vec::new();
    let mut bytes_copied = 0u64;
    for (relative, source) in &sources {
        writer
            .start_file(relative.as_str(), options)
            .map_err(|error| BundleError::Zip(error.to_string()))?;
        let mut input = fs::File::open(source).map_err(|error| io_error(source, error))?;
        let copied =
            std::io::copy(&mut input, &mut writer).map_err(|error| io_error(source, error))?;
        bytes_copied += copied;
        written.push(relative.to_string());
    }
    for extra in ["LICENSES.md", "CREDITS.md"] {
        let source = kit_dir.join(extra);
        if source.is_file() {
            writer
                .start_file(extra, options)
                .map_err(|error| BundleError::Zip(error.to_string()))?;
            let mut input = fs::File::open(&source).map_err(|error| io_error(&source, error))?;
            std::io::copy(&mut input, &mut writer).map_err(|error| io_error(&source, error))?;
        }
    }
    writer
        .finish()
        .map_err(|error| BundleError::Zip(error.to_string()))?;

    Ok(ExportReport {
        kit_id: kit.id,
        samples_copied: sources.len(),
        bytes_copied,
        written,
    })
}

/// Import a folder bundle into `dest_dir`, creating the destination.
pub fn import_kit_folder(src_dir: &Path, dest_dir: &Path) -> Result<ImportReport, BundleError> {
    let manifest_path = src_dir.join(MANIFEST);
    if !manifest_path.is_file() {
        return Err(BundleError::InvalidBundle(format!(
            "{} is missing",
            manifest_path.display()
        )));
    }
    let bytes = fs::read(&manifest_path).map_err(|error| io_error(&manifest_path, error))?;
    let parsed = format::parse_kit_bytes(&bytes)?;
    let report = validate_kit_report(&parsed.kit);
    if !report.is_valid() {
        return Err(BundleError::Validation(ValidationError {
            issues: report.errors().cloned().collect(),
        }));
    }
    ensure_destination(dest_dir)?;

    let mut kit = parsed.kit;
    let mut missing = Vec::new();
    let mut renamed = Vec::new();
    let mut copied = 0usize;
    // Canonical lower-case destination path -> (actual relative path, raw hash).
    let mut destinations: BTreeMap<String, (RelativeSamplePath, ContentHash)> = BTreeMap::new();
    let mut rewrites: BTreeMap<String, RelativeSamplePath> = BTreeMap::new();

    for sample in kit.samples() {
        let source = sample.resolve(src_dir);
        if !source.is_file() {
            missing.push(sample.path.clone());
            continue;
        }
        let bytes = fs::read(&source).map_err(|error| io_error(&source, error))?;
        let raw_hash = ContentHash::of_bytes(&bytes);
        let key = sample.path.as_str().to_ascii_lowercase();

        let (relative, write) = match destinations.get(&key) {
            Some((existing, existing_hash)) if *existing_hash == raw_hash => {
                // Identical bytes: dedupe onto the existing destination.
                rewrites.insert(sample.path.to_string(), existing.clone());
                continue;
            }
            Some((existing, _)) => {
                let collision_name =
                    collision_path(existing.as_str(), &raw_hash, &destinations, &kit);
                (collision_name, true)
            }
            None => (sample.path.clone(), true),
        };

        if write {
            let target = relative.resolve(dest_dir);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
            }
            fs::write(&target, &bytes).map_err(|error| io_error(&target, error))?;
            copied += 1;
        }
        if relative != sample.path {
            renamed.push((sample.path.to_string(), relative.to_string()));
            rewrites.insert(sample.path.to_string(), relative.clone());
        }
        destinations.insert(relative.as_str().to_ascii_lowercase(), (relative, raw_hash));
    }

    // Apply path rewrites to every layer that referenced a renamed file.
    for pad in &mut kit.pads {
        for layer in &mut pad.layers {
            if let Some(sample) = &mut layer.sample
                && let Some(replacement) = rewrites.get(sample.path.as_str())
            {
                sample.path = replacement.clone();
            }
        }
    }

    let manifest = format::to_json_pretty(&kit)?;
    fs::write(dest_dir.join(MANIFEST), manifest).map_err(|error| io_error(dest_dir, error))?;

    Ok(ImportReport {
        kit,
        samples_copied: copied,
        missing,
        renamed,
    })
}

fn collision_path(
    original: &str,
    hash: &ContentHash,
    destinations: &BTreeMap<String, (RelativeSamplePath, ContentHash)>,
    _kit: &Kit,
) -> RelativeSamplePath {
    let suffix = &hash.to_hex()[..8];
    let (stem, extension) = match original.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (original, String::new()),
    };
    for attempt in 0..10_000 {
        let candidate = if attempt == 0 {
            format!("{stem}-{suffix}{extension}")
        } else {
            format!("{stem}-{suffix}-{attempt}{extension}")
        };
        if !destinations.contains_key(&candidate.to_ascii_lowercase()) {
            return RelativeSamplePath::new(candidate)
                .unwrap_or_else(|_| RelativeSamplePath::new("samples/imported.wav").unwrap());
        }
    }
    RelativeSamplePath::new(format!("samples/imported-{suffix}.wav")).unwrap()
}

/// Import a ZIP bundle into `dest_dir`.
pub fn import_kit_archive(src_zip: &Path, dest_dir: &Path) -> Result<ImportReport, BundleError> {
    let file = fs::File::open(src_zip).map_err(|error| io_error(src_zip, error))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| BundleError::Zip(error.to_string()))?;
    let staging = std::env::temp_dir().join(format!("drumpad-import-{}", KitId::new()));
    let guard = StagingGuard(staging.clone());
    fs::create_dir_all(&staging).map_err(|error| io_error(&staging, error))?;

    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| BundleError::Zip(error.to_string()))?;
        let name = entry.name().to_owned();
        let relative = RelativeSamplePath::new(name.clone())
            .map_err(|_| BundleError::UnsafeEntry(name.clone()))?;
        if entry.is_dir() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_ARCHIVE_BYTES {
            return Err(BundleError::InvalidBundle(
                "archive expands beyond the 1 GiB safety limit".into(),
            ));
        }
        let target = relative.resolve(&staging);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        let mut output = fs::File::create(&target).map_err(|error| io_error(&target, error))?;
        std::io::copy(&mut entry, &mut output).map_err(|error| io_error(&target, error))?;
    }

    let report = import_kit_folder(&staging, dest_dir);
    drop(guard);
    report
}

struct StagingGuard(PathBuf);

impl Drop for StagingGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Convenience wrapper used by the app: export the active kit of a library.
pub fn export_active_kit(
    library: &KitLibrary,
    dest_dir: &Path,
) -> Result<ExportReport, BundleError> {
    let kit = match library.active_kit_id() {
        Some(id) => library
            .load(id)
            .map_err(|error| BundleError::InvalidBundle(error.to_string()))?,
        None => {
            return Err(BundleError::InvalidBundle("no active kit to export".into()));
        }
    };
    let kit_dir = library
        .kit_dir(kit.id)
        .map_err(|error| BundleError::InvalidBundle(error.to_string()))?;
    export_kit_folder(&kit, &kit_dir, dest_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{ContentHash, Kit, Layer, RelativeSamplePath, SampleRef};
    use std::fs;

    fn make_kit(dir: &Path) -> Kit {
        let mut kit = Kit::silent_default();
        kit.metadata.name = "Bundle Kit".into();
        let audio = b"RIFF....fake-wav-data";
        let path = dir.join("samples/kick.wav");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, audio).unwrap();
        kit.pads[0].layers.push(Layer::with_sample(
            Default::default(),
            SampleRef::new(
                RelativeSamplePath::new("samples/kick.wav").unwrap(),
                ContentHash::of_bytes(audio),
            ),
        ));
        kit
    }

    #[test]
    fn folder_round_trip_is_exact() {
        let work = tempfile::tempdir().unwrap();
        let kit_dir = work.path().join("source");
        fs::create_dir_all(&kit_dir).unwrap();
        let kit = make_kit(&kit_dir);

        let exported = export_kit_folder(&kit, &kit_dir, &work.path().join("bundle")).unwrap();
        assert_eq!(exported.samples_copied, 1);

        let report =
            import_kit_folder(&work.path().join("bundle"), &work.path().join("imported")).unwrap();
        assert!(report.missing.is_empty());
        assert_eq!(report.kit, kit);
        assert_eq!(
            fs::read(work.path().join("imported/samples/kick.wav")).unwrap(),
            b"RIFF....fake-wav-data"
        );
    }

    #[test]
    fn zip_round_trip_is_exact() {
        let work = tempfile::tempdir().unwrap();
        let kit_dir = work.path().join("source");
        fs::create_dir_all(&kit_dir).unwrap();
        let kit = make_kit(&kit_dir);
        let archive = work.path().join("kit.zip");
        export_kit_archive(&kit, &kit_dir, &archive).unwrap();

        let report = import_kit_archive(&archive, &work.path().join("imported")).unwrap();
        assert!(report.missing.is_empty());
        assert_eq!(report.kit, kit);
        assert_eq!(
            fs::read(work.path().join("imported/samples/kick.wav")).unwrap(),
            b"RIFF....fake-wav-data"
        );
    }

    #[test]
    fn export_refuses_missing_samples() {
        let work = tempfile::tempdir().unwrap();
        let kit_dir = work.path().join("source");
        fs::create_dir_all(&kit_dir).unwrap();
        let kit = make_kit(&kit_dir);
        fs::remove_file(kit_dir.join("samples/kick.wav")).unwrap();
        let error = export_kit_folder(&kit, &kit_dir, &work.path().join("bundle")).unwrap_err();
        assert!(matches!(error, BundleError::MissingSamples(_)));
    }

    #[test]
    fn corrupt_bundle_is_rejected() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("bad");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(MANIFEST), b"{ not json").unwrap();
        let error = import_kit_folder(&src, &work.path().join("dest")).unwrap_err();
        assert!(matches!(error, BundleError::Format(_)));
    }

    #[test]
    fn missing_files_are_reported_not_fatal() {
        let work = tempfile::tempdir().unwrap();
        let kit_dir = work.path().join("source");
        fs::create_dir_all(&kit_dir).unwrap();
        let kit = make_kit(&kit_dir);
        let bundle = work.path().join("bundle");
        export_kit_folder(&kit, &kit_dir, &bundle).unwrap();
        fs::remove_file(bundle.join("samples/kick.wav")).unwrap();
        let report = import_kit_folder(&bundle, &work.path().join("imported")).unwrap();
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.missing[0].as_str(), "samples/kick.wav");
    }

    #[test]
    fn collision_paths_include_content_hash() {
        let hash = ContentHash::of_bytes(b"different");
        let mut destinations = BTreeMap::new();
        let original = RelativeSamplePath::new("samples/hit.wav").unwrap();
        destinations.insert(
            "samples/hit.wav".to_owned(),
            (original.clone(), ContentHash::of_bytes(b"first")),
        );
        let kit = Kit::silent_default();
        let resolved = collision_path("samples/hit.wav", &hash, &destinations, &kit);
        assert_ne!(resolved, original);
        assert!(resolved.as_str().contains(&hash.to_hex()[..8]));
        assert!(resolved.as_str().ends_with(".wav"));
    }

    #[test]
    fn export_rejects_an_invalid_kit() {
        let work = tempfile::tempdir().unwrap();
        let mut kit = Kit::silent_default();
        kit.metadata.name.clear();
        let error = export_kit_folder(&kit, work.path(), &work.path().join("bundle")).unwrap_err();
        assert!(matches!(error, BundleError::Validation(_)));
        let error =
            export_kit_archive(&kit, work.path(), &work.path().join("kit.zip")).unwrap_err();
        assert!(matches!(error, BundleError::Validation(_)));
    }

    #[test]
    fn destination_must_be_empty() {
        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();
        let error = ensure_destination(&dest).unwrap_err();
        assert!(matches!(error, BundleError::DestinationExists(_)));
    }
}
