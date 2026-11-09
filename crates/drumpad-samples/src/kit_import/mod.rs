//! Create a new kit from a folder of audio samples.
//!
//! This is the MVP import path: scan a folder recursively, sort the supported
//! files alphabetically, assign the first 16 valid files to pads 1-16, copy the
//! originals into the kit's `samples/` folder, and save the manifest. The
//! resulting layout is exactly what `SampleRef::resolve(kit_dir)` expects, so
//! the audio runtime can load it unchanged.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use drumpad_model::kit::{
    ContentHash, DomainError, KeyMapping, Kit, KitId, KitLibrary, Layer, LayerId, LibraryError,
    MidiMapping, PAD_COUNT, Pad, PadId, PlaybackMode, RelativeSamplePath, SampleRef,
};

use crate::decode::{AudioFormat, decode_file};
use crate::import::{ImportLimits, RejectedFile, validate_import_file};

/// Maximum directory depth followed while scanning a sample folder.
const MAX_SCAN_DEPTH: usize = 32;
/// Longest sanitized stem used in a copied file name.
const MAX_COPY_STEM_CHARS: usize = 48;

/// A kit created from a folder, including rejected and skipped files.
#[derive(Clone, PartialEq, Debug)]
pub struct ImportedKit {
    /// Saved, active kit.
    pub kit: Kit,
    /// Folder that `SampleRef::resolve` joins to.
    pub kit_dir: PathBuf,
    /// Number of files assigned to pads.
    pub imported: usize,
    /// Files that could not be used, with actionable reasons.
    pub rejected: Vec<RejectedFile>,
    /// Valid files beyond the 16-pad limit.
    pub skipped: usize,
}

/// Errors from folder-to-kit import.
#[derive(Debug, Error)]
pub enum KitImportError {
    /// Kit creation, lookup, or saving failed.
    #[error(transparent)]
    Library(#[from] LibraryError),
    /// The supplied path is not a directory.
    #[error("import folder does not exist or is not a directory: {}", .0.display())]
    NotAFolder(PathBuf),
    /// The top-level folder could not be read.
    #[error("cannot read import folder {}: {source}", path.display())]
    Folder {
        /// Folder that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// No usable audio files were found.
    #[error(
        "no valid audio files found in {} ({} rejected)",
        folder.display(),
        rejected.len()
    )]
    NoValidFiles {
        /// Folder that was scanned.
        folder: PathBuf,
        /// Why candidate files were rejected.
        rejected: Vec<RejectedFile>,
    },
    /// Copying a source file into the kit failed.
    #[error("failed to copy {} to {}: {source}", from.display(), to.display())]
    Copy {
        /// Source file.
        from: PathBuf,
        /// Destination file.
        to: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The kit's sample folder could not be created.
    #[error("failed to create sample folder {}: {source}", path.display())]
    SamplesDir {
        /// Folder that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A generated relative path was rejected by the model.
    #[error("invalid generated sample path: {0}")]
    SamplePath(#[from] DomainError),
}

struct Candidate {
    path: PathBuf,
    sort_key: String,
}

struct Scan {
    candidates: Vec<Candidate>,
    rejected: Vec<RejectedFile>,
}

struct ValidFile {
    path: PathBuf,
    hash: ContentHash,
    stem: String,
    file_name: String,
}

/// Import a folder of audio files as a new kit.
///
/// The scan is deterministic (alphabetical by relative path, case-insensitive
/// with a case-sensitive tiebreak). Hidden and temporary files are ignored.
/// Unsupported, oversized, or corrupt files are reported in
/// [`ImportedKit::rejected`] and never abort the batch. Valid files beyond the
/// first 16 are counted in [`ImportedKit::skipped`].
pub fn import_folder_into_new_kit(
    library: &mut KitLibrary,
    kit_name: &str,
    folder: &Path,
) -> Result<ImportedKit, KitImportError> {
    let scan = scan_folder(folder)?;
    let mut rejected = scan.rejected;
    let mut valid = Vec::new();
    for candidate in scan.candidates {
        if let Some(file) = inspect_candidate(candidate, &mut rejected) {
            valid.push(file);
        }
    }
    rejected.sort_by(|a, b| a.path.cmp(&b.path));

    if valid.is_empty() {
        return Err(KitImportError::NoValidFiles {
            folder: folder.to_owned(),
            rejected,
        });
    }

    let imported_count = valid.len().min(PAD_COUNT);
    let skipped = valid.len().saturating_sub(imported_count);

    let mut kit = library.create(kit_name)?;
    let kit_dir = match library.kit_dir(kit.id) {
        Ok(dir) => dir,
        Err(error) => {
            cleanup_created_kit(library, kit.id);
            return Err(error.into());
        }
    };

    if let Err(error) = copy_samples_and_build_pads(&mut kit, &kit_dir, &valid[..imported_count]) {
        cleanup_created_kit(library, kit.id);
        return Err(error);
    }
    if let Err(error) = library.save(&mut kit) {
        cleanup_created_kit(library, kit.id);
        return Err(error.into());
    }

    Ok(ImportedKit {
        kit,
        kit_dir,
        imported: imported_count,
        rejected,
        skipped,
    })
}

fn scan_folder(folder: &Path) -> Result<Scan, KitImportError> {
    if !folder.is_dir() {
        return Err(KitImportError::NotAFolder(folder.to_owned()));
    }

    let mut scan = Scan {
        candidates: Vec::new(),
        rejected: Vec::new(),
    };
    let mut pending = vec![(folder.to_owned(), 0usize)];
    let mut visited = HashSet::new();

    while let Some((dir, depth)) = pending.pop() {
        let canonical = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !visited.insert(canonical) || depth > MAX_SCAN_DEPTH {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(source) if depth == 0 => {
                return Err(KitImportError::Folder { path: dir, source });
            }
            Err(source) => {
                scan.rejected.push(RejectedFile {
                    path: dir,
                    reason: format!("cannot read folder: {source}"),
                });
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) => {
                    scan.rejected.push(RejectedFile {
                        path: dir.clone(),
                        reason: format!("cannot read folder entry: {source}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if is_ignored_name(&name) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            // Symlinks can escape the folder or form cycles; they are not part
            // of a portable kit bundle.
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if !is_ignored_directory(&name) {
                    pending.push((path, depth + 1));
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !AudioFormat::is_supported_extension(&extension) {
                scan.rejected.push(RejectedFile {
                    path,
                    reason: unsupported_reason(&extension),
                });
                continue;
            }

            let relative = path.strip_prefix(folder).unwrap_or(&path);
            let relative = relative.to_string_lossy().replace('\\', "/");
            let sort_key = format!("{}\0{relative}", relative.to_ascii_lowercase());
            scan.candidates.push(Candidate { path, sort_key });
        }
    }

    scan.candidates.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    Ok(scan)
}

fn inspect_candidate(candidate: Candidate, rejected: &mut Vec<RejectedFile>) -> Option<ValidFile> {
    if let Err(error) = validate_import_file(&candidate.path, ImportLimits::default()) {
        rejected.push(RejectedFile {
            path: candidate.path,
            reason: error.to_string(),
        });
        return None;
    }
    match decode_file(&candidate.path) {
        Ok(decoded) => {
            let file_name = candidate
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sample".to_owned());
            let stem = candidate
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sample".to_owned());
            Some(ValidFile {
                path: candidate.path,
                hash: decoded.audio.content_hash(),
                stem,
                file_name,
            })
        }
        Err(error) => {
            rejected.push(RejectedFile {
                path: candidate.path,
                reason: error.to_string(),
            });
            None
        }
    }
}

fn copy_samples_and_build_pads(
    kit: &mut Kit,
    kit_dir: &Path,
    files: &[ValidFile],
) -> Result<(), KitImportError> {
    let samples_dir = kit_dir.join("samples");
    fs::create_dir_all(&samples_dir).map_err(|source| KitImportError::SamplesDir {
        path: samples_dir.clone(),
        source,
    })?;

    let mut used_names = HashSet::new();
    let mut pads = Vec::with_capacity(PAD_COUNT);
    for slot in 0..PAD_COUNT {
        let pad_id = PadId::new(slot as u8).expect("pad slot is within 0..16");
        let Some(file) = files.get(slot) else {
            pads.push(Pad::silent(pad_id));
            continue;
        };

        let extension = file
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("wav")
            .to_ascii_lowercase();
        let file_name =
            unique_copy_name(slot, &file.stem, &extension, &samples_dir, &mut used_names);
        let destination = samples_dir.join(&file_name);
        fs::copy(&file.path, &destination).map_err(|source| KitImportError::Copy {
            from: file.path.clone(),
            to: destination,
            source,
        })?;

        let relative = RelativeSamplePath::new(format!("samples/{file_name}"))?;
        let sample = SampleRef {
            path: relative,
            hash: Some(file.hash),
            original_name: Some(truncate_chars(&file.file_name, 255)),
        };

        let mut pad = Pad::silent(pad_id);
        pad.name = Some(display_name(&file.stem, slot));
        pad.layers.push(Layer::with_sample(LayerId::FIRST, sample));
        pad.playback = PlaybackMode::OneShot;
        pad.key = KeyMapping::default_for(slot);
        pad.midi = MidiMapping::default_for(slot);
        pads.push(pad);
    }
    kit.pads = pads;
    Ok(())
}

fn unique_copy_name(
    slot: usize,
    stem: &str,
    extension: &str,
    samples_dir: &Path,
    used: &mut HashSet<String>,
) -> String {
    let base = format!("pad-{:02}-{}", slot + 1, sanitize_stem(stem));
    let mut candidate = format!("{base}.{extension}");
    let mut counter = 2;
    loop {
        let key = candidate.to_ascii_lowercase();
        if used.insert(key) && !samples_dir.join(&candidate).exists() {
            return candidate;
        }
        candidate = format!("{base}-{counter}.{extension}");
        counter += 1;
    }
}

fn sanitize_stem(stem: &str) -> String {
    let mut sanitized = String::new();
    let mut last_separator = false;
    for character in stem.chars() {
        if character.is_ascii_alphanumeric() {
            sanitized.push(character.to_ascii_lowercase());
            last_separator = false;
        } else if !last_separator && !sanitized.is_empty() {
            sanitized.push('_');
            last_separator = true;
        }
        if sanitized.chars().count() >= MAX_COPY_STEM_CHARS {
            break;
        }
    }
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "sample".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn display_name(stem: &str, slot: usize) -> String {
    let trimmed = stem.trim();
    if trimmed.is_empty() {
        format!("Pad {}", slot + 1)
    } else {
        truncate_chars(trimmed, 64)
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_owned()
    } else {
        value.chars().take(max).collect()
    }
}

fn is_ignored_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with('.')
        || name.starts_with("~$")
        || name.starts_with('~')
        || name.ends_with('~')
        || lower.ends_with(".tmp")
        || lower.ends_with(".temp")
        || matches!(lower.as_str(), "thumbs.db" | "desktop.ini")
}

fn is_ignored_directory(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with('.')
        || matches!(
            lower.as_str(),
            "__macosx" | "$recycle.bin" | "system volume information"
        )
}

fn unsupported_reason(extension: &str) -> String {
    let supported = AudioFormat::SUPPORTED_EXTENSIONS.join(", ");
    if extension.is_empty() {
        format!("file has no extension; supported: {supported}")
    } else {
        format!("unsupported format `.{extension}`; supported: {supported}")
    }
}

fn cleanup_created_kit(library: &mut KitLibrary, kit_id: KitId) {
    // Never remove a kit that is currently active; the caller can retry or
    // delete it explicitly from the UI.
    if library.active_kit_id() != Some(kit_id) {
        let _ = library.delete(kit_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_stems_are_filesystem_safe() {
        assert_eq!(sanitize_stem("Kick Drum"), "kick_drum");
        assert_eq!(sanitize_stem("snare.01"), "snare_01");
        assert_eq!(sanitize_stem("   "), "sample");
        assert_eq!(sanitize_stem("Héllo"), "h_llo");
        assert!(sanitize_stem(&"x".repeat(100)).len() <= MAX_COPY_STEM_CHARS);
    }

    #[test]
    fn hidden_and_temp_names_are_ignored() {
        for name in [
            ".hidden.wav",
            "~$lock.wav",
            "backup.wav~",
            "note.tmp",
            "Thumbs.db",
            "desktop.ini",
        ] {
            assert!(is_ignored_name(name), "{name}");
        }
        assert!(!is_ignored_name("kick.wav"));
    }

    #[test]
    fn unsupported_reason_lists_formats() {
        let reason = unsupported_reason("txt");
        assert!(reason.contains("txt"));
        assert!(reason.contains("wav"));
        assert!(reason.contains("flac"));
        assert!(unsupported_reason("").contains("no extension"));
    }

    #[test]
    fn copy_names_are_unique_per_pad() {
        let dir = tempfile::tempdir().unwrap();
        let mut used = HashSet::new();
        let first = unique_copy_name(0, "kick", "wav", dir.path(), &mut used);
        let second = unique_copy_name(1, "kick", "wav", dir.path(), &mut used);
        assert_eq!(first, "pad-01-kick.wav");
        assert_eq!(second, "pad-02-kick.wav");
        assert_ne!(first, second);
    }

    #[test]
    fn existing_copy_names_advance_to_a_free_suffix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pad-01-kick.wav"), b"existing").unwrap();
        let mut used = HashSet::new();
        let name = unique_copy_name(0, "kick", "wav", dir.path(), &mut used);
        assert_eq!(name, "pad-01-kick-2.wav");
    }

    #[test]
    fn ignored_directories_are_recognised() {
        for name in [
            ".git",
            ".hidden",
            "__MACOSX",
            "$RECYCLE.BIN",
            "System Volume Information",
        ] {
            assert!(is_ignored_directory(name), "{name}");
        }
        assert!(!is_ignored_directory("samples"));
    }

    #[test]
    fn display_names_fall_back_and_truncate() {
        assert_eq!(display_name("   ", 3), "Pad 4");
        assert_eq!(display_name("Kick", 0), "Kick");
        assert_eq!(display_name(&"x".repeat(100), 0).chars().count(), 64);
    }

    #[test]
    fn ignored_names_cover_all_temp_patterns() {
        for name in ["file.wav~", "file.TEMP", "~backup.wav"] {
            assert!(is_ignored_name(name), "{name}");
        }
        assert!(!is_ignored_name("kick.WAV"));
    }
}
