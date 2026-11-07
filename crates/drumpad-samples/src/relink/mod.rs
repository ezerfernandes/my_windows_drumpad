//! Missing-sample detection and relinking.
//!
//! Kit paths are relative to the kit folder, so a moved kit keeps working as
//! long as its samples travel with it. When audio is missing or changed, this
//! module verifies decoded content hashes, lists affected pads, scans a folder
//! for candidates, and repairs the manifest paths without hand editing.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use drumpad_model::kit::{ContentHash, Kit, PadId, RelativeSamplePath};
use thiserror::Error;

use crate::decode::{AudioFormat, DecodeError, decode_file};

/// Current state of one referenced sample.
#[derive(Clone, PartialEq, Debug)]
pub enum SampleStatus {
    /// The file exists and its decoded content matches the manifest.
    Ok {
        /// Resolved file path.
        path: PathBuf,
    },
    /// The file does not exist.
    Missing {
        /// Path that was checked.
        expected: PathBuf,
    },
    /// The file exists but does not decode.
    Undecodable {
        /// Resolved file path.
        path: PathBuf,
        /// Decoder explanation.
        error: String,
    },
    /// The file decoded but its content hash differs from the manifest.
    HashMismatch {
        /// Resolved file path.
        path: PathBuf,
        /// Hash found on disk.
        found: ContentHash,
        /// Hash recorded in the kit.
        expected: ContentHash,
    },
    /// The manifest has no hash, so content cannot be verified.
    Unverified {
        /// Resolved file path.
        path: PathBuf,
    },
    /// The sample has no assigned path (should not happen for a SampleRef).
    NotAssigned,
}

/// Resolution of one distinct sample path.
#[derive(Clone, PartialEq, Debug)]
pub struct ResolvedSample {
    /// Manifest-relative path.
    pub path: RelativeSamplePath,
    /// Pads that reference it.
    pub pad_ids: Vec<PadId>,
    /// Hash recorded in the manifest.
    pub expected_hash: Option<ContentHash>,
    /// Current status.
    pub status: SampleStatus,
}

/// Resolution for every distinct sample in a kit.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SampleResolution {
    /// Per-path state, ordered by path.
    pub samples: Vec<ResolvedSample>,
}

impl SampleResolution {
    /// True when every sample exists and verifies.
    pub fn is_complete(&self) -> bool {
        self.samples.iter().all(|sample| {
            matches!(
                sample.status,
                SampleStatus::Ok { .. } | SampleStatus::Unverified { .. }
            )
        })
    }

    /// Missing samples with their affected pads.
    pub fn missing(&self) -> Vec<&ResolvedSample> {
        self.samples
            .iter()
            .filter(|sample| matches!(sample.status, SampleStatus::Missing { .. }))
            .collect()
    }

    /// Samples whose content hash changed, with affected pads.
    pub fn mismatched(&self) -> Vec<&ResolvedSample> {
        self.samples
            .iter()
            .filter(|sample| matches!(sample.status, SampleStatus::HashMismatch { .. }))
            .collect()
    }

    /// Human-readable list of problems, including pad numbers.
    pub fn problem_summary(&self) -> Vec<String> {
        self.samples
            .iter()
            .filter_map(|sample| {
                let problem = match &sample.status {
                    SampleStatus::Ok { .. } | SampleStatus::Unverified { .. } => return None,
                    SampleStatus::Missing { expected } => {
                        format!("{} is missing ({})", sample.path, expected.display())
                    }
                    SampleStatus::Undecodable { path, error } => {
                        format!(
                            "{} cannot be decoded ({}): {error}",
                            sample.path,
                            path.display()
                        )
                    }
                    SampleStatus::HashMismatch { path, .. } => {
                        format!("{} changed on disk ({})", sample.path, path.display())
                    }
                    SampleStatus::NotAssigned => "unassigned sample".to_owned(),
                };
                let pads = sample
                    .pad_ids
                    .iter()
                    .map(|pad| (pad.get() + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("{problem}; affected pads: {pads}"))
            })
            .collect()
    }

    /// Distinct pads affected by any problem.
    pub fn affected_pads(&self) -> Vec<PadId> {
        let mut pads = std::collections::BTreeSet::new();
        for sample in &self.samples {
            if matches!(
                sample.status,
                SampleStatus::Ok { .. } | SampleStatus::Unverified { .. }
            ) {
                continue;
            }
            pads.extend(sample.pad_ids.iter().copied());
        }
        pads.into_iter().collect()
    }
}

/// Resolve every sample path of a kit against its folder.
pub fn resolve_kit_samples(kit: &Kit, kit_dir: &Path) -> SampleResolution {
    let mut by_path: BTreeMap<String, (RelativeSamplePath, Vec<PadId>, Option<ContentHash>)> =
        BTreeMap::new();
    for pad in &kit.pads {
        for layer in &pad.layers {
            let Some(sample) = &layer.sample else {
                continue;
            };
            let entry = by_path
                .entry(sample.path.as_str().to_owned())
                .or_insert_with(|| (sample.path.clone(), Vec::new(), sample.hash));
            if !entry.1.contains(&pad.id) {
                entry.1.push(pad.id);
            }
        }
    }

    let mut samples = Vec::with_capacity(by_path.len());
    for (_, (path, pad_ids, expected_hash)) in by_path {
        let resolved = path.resolve(kit_dir);
        let status = if !resolved.is_file() {
            SampleStatus::Missing {
                expected: resolved.clone(),
            }
        } else {
            match decode_file(&resolved) {
                Ok(decoded) => {
                    let found = decoded.audio.content_hash();
                    match expected_hash {
                        None => SampleStatus::Unverified { path: resolved },
                        Some(expected) if expected == found => SampleStatus::Ok { path: resolved },
                        Some(expected) => SampleStatus::HashMismatch {
                            path: resolved,
                            found,
                            expected,
                        },
                    }
                }
                Err(error) => SampleStatus::Undecodable {
                    path: resolved,
                    error: error.to_string(),
                },
            }
        };
        samples.push(ResolvedSample {
            path,
            pad_ids,
            expected_hash,
            status,
        });
    }
    SampleResolution { samples }
}

/// Index of audio files found under a folder, keyed by content hash and name.
#[derive(Clone, Debug, Default)]
pub struct RelinkIndex {
    by_hash: BTreeMap<ContentHash, Vec<PathBuf>>,
    by_name: BTreeMap<String, Vec<PathBuf>>,
    /// Files that decoded successfully.
    pub indexed: usize,
    /// Files skipped because they were unsupported or undecodable.
    pub skipped: usize,
}

impl RelinkIndex {
    /// Scan a folder recursively. Undecodable files are counted, not fatal.
    pub fn scan(root: &Path) -> Result<Self, RelinkError> {
        let mut index = Self::default();
        let mut pending = vec![root.to_owned()];
        let mut visited = 0usize;
        while let Some(path) = pending.pop() {
            if path.is_file() {
                let extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                if !AudioFormat::is_supported_extension(extension) {
                    index.skipped += 1;
                    continue;
                }
                match decode_file(&path) {
                    Ok(decoded) => {
                        let hash = decoded.audio.content_hash();
                        index.by_hash.entry(hash).or_default().push(path.clone());
                        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                            index
                                .by_name
                                .entry(name.to_ascii_lowercase())
                                .or_default()
                                .push(path.clone());
                        }
                        index.indexed += 1;
                    }
                    Err(_) => index.skipped += 1,
                }
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            visited += 1;
            if visited > 64 {
                continue;
            }
            let entries = fs::read_dir(&path).map_err(|source| RelinkError::Io {
                path: path.clone(),
                source,
            })?;
            for entry in entries {
                let entry = entry.map_err(|source| RelinkError::Io {
                    path: path.clone(),
                    source,
                })?;
                pending.push(entry.path());
            }
        }
        Ok(index)
    }

    /// Number of distinct content hashes indexed.
    pub fn hash_count(&self) -> usize {
        self.by_hash.len()
    }

    /// True when the index found nothing.
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty() && self.by_name.is_empty()
    }

    /// Candidate paths for a hash.
    pub fn candidates_for_hash(&self, hash: &ContentHash) -> &[PathBuf] {
        self.by_hash.get(hash).map_or(&[], Vec::as_slice)
    }

    /// Candidate paths for a file name.
    pub fn candidates_for_name(&self, name: &str) -> &[PathBuf] {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }
}

/// One repaired sample reference.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RelinkRepair {
    /// Previous manifest path.
    pub old_path: RelativeSamplePath,
    /// New manifest path.
    pub new_path: RelativeSamplePath,
    /// Pads that now resolve.
    pub pad_ids: Vec<PadId>,
}

/// One sample that could not be repaired.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnresolvedSample {
    /// Manifest path.
    pub path: RelativeSamplePath,
    /// Affected pads.
    pub pad_ids: Vec<PadId>,
    /// Why it was not repaired.
    pub reason: String,
}

/// Result of a relink pass.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RelinkReport {
    /// Repaired references.
    pub repaired: Vec<RelinkRepair>,
    /// References that remain broken.
    pub unresolved: Vec<UnresolvedSample>,
}

impl RelinkReport {
    /// True when every problem was repaired.
    pub fn is_complete(&self) -> bool {
        self.unresolved.is_empty()
    }
}

/// Relink broken samples from an index, copying audio into the kit when needed.
pub fn relink_kit(
    kit: &mut Kit,
    kit_dir: &Path,
    index: &RelinkIndex,
) -> Result<RelinkReport, RelinkError> {
    let resolution = resolve_kit_samples(kit, kit_dir);
    let mut report = RelinkReport::default();
    // Old manifest path -> (new path, verified content hash when known).
    let mut rewrites: BTreeMap<String, (RelativeSamplePath, Option<ContentHash>)> = BTreeMap::new();
    let mut used_paths: std::collections::BTreeSet<String> = kit
        .samples()
        .iter()
        .map(|sample| sample.path.as_str().to_ascii_lowercase())
        .collect();

    for sample in resolution.samples {
        let (needs_repair, expected_hash) = match &sample.status {
            SampleStatus::Ok { .. } | SampleStatus::Unverified { .. } => continue,
            SampleStatus::Missing { .. } | SampleStatus::Undecodable { .. } => {
                (true, sample.expected_hash)
            }
            SampleStatus::HashMismatch { expected, .. } => (true, Some(*expected)),
            SampleStatus::NotAssigned => (true, sample.expected_hash),
        };
        if !needs_repair {
            continue;
        }

        let name = sample
            .path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or("sample.wav")
            .to_owned();
        let candidates = match expected_hash {
            Some(hash) => index.candidates_for_hash(&hash).to_vec(),
            None => index.candidates_for_name(&name).to_vec(),
        };
        let Some(candidate) = candidates.first() else {
            report.unresolved.push(UnresolvedSample {
                path: sample.path,
                pad_ids: sample.pad_ids,
                reason: if expected_hash.is_some() {
                    "no file with a matching content hash was found".to_owned()
                } else {
                    "no file with a matching name was found".to_owned()
                },
            });
            continue;
        };

        // Prefer a path that is already inside the kit folder.
        if let Some(relative) = relative_inside(kit_dir, candidate) {
            if let Ok(path) = RelativeSamplePath::new(relative) {
                if used_paths.insert(path.as_str().to_ascii_lowercase()) {
                    let hash = expected_hash.or_else(|| hash_of(candidate));
                    report.repaired.push(RelinkRepair {
                        old_path: sample.path.clone(),
                        new_path: path.clone(),
                        pad_ids: sample.pad_ids.clone(),
                    });
                    rewrites.insert(sample.path.to_string(), (path, hash));
                    continue;
                }
            }
        }

        let target_relative = unique_sample_path("samples", &name, &used_paths);
        let target = target_relative.resolve(kit_dir);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| RelinkError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        fs::copy(candidate, &target).map_err(|source| RelinkError::Io {
            path: candidate.clone(),
            source,
        })?;
        used_paths.insert(target_relative.as_str().to_ascii_lowercase());
        let hash = expected_hash.or_else(|| hash_of(&target));
        report.repaired.push(RelinkRepair {
            old_path: sample.path.clone(),
            new_path: target_relative.clone(),
            pad_ids: sample.pad_ids.clone(),
        });
        rewrites.insert(sample.path.to_string(), (target_relative, hash));
    }

    for pad in &mut kit.pads {
        for layer in &mut pad.layers {
            if let Some(sample) = &mut layer.sample
                && let Some((replacement, hash)) = rewrites.get(sample.path.as_str())
            {
                sample.path = replacement.clone();
                // A name-based relink has no manifest hash to inherit; record
                // the verified hash so the kit is no longer "unverified".
                if sample.hash.is_none() {
                    sample.hash = *hash;
                }
            }
        }
    }

    Ok(report)
}

/// Scan a folder and relink in one call.
pub fn relink_kit_from_folder(
    kit: &mut Kit,
    kit_dir: &Path,
    search_root: &Path,
) -> Result<RelinkReport, RelinkError> {
    let index = RelinkIndex::scan(search_root)?;
    relink_kit(kit, kit_dir, &index)
}

fn hash_of(path: &Path) -> Option<ContentHash> {
    decode_file(path)
        .ok()
        .map(|decoded| decoded.audio.content_hash())
}

fn relative_inside(root: &Path, file: &Path) -> Option<String> {
    let root = fs::canonicalize(root).ok()?;
    let file = fs::canonicalize(file).ok()?;
    let relative = file.strip_prefix(&root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn unique_sample_path(
    folder: &str,
    file_name: &str,
    used: &std::collections::BTreeSet<String>,
) -> RelativeSamplePath {
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (file_name.to_owned(), String::new()),
    };
    for attempt in 0..10_000 {
        let candidate = if attempt == 0 {
            format!("{folder}/{stem}{extension}")
        } else {
            format!("{folder}/{stem}-{attempt}{extension}")
        };
        if !used.contains(&candidate.to_ascii_lowercase()) {
            if let Ok(path) = RelativeSamplePath::new(candidate) {
                return path;
            }
        }
    }
    RelativeSamplePath::new(format!("{folder}/sample.wav")).expect("static fallback is valid")
}

/// Relink failures.
#[derive(Debug, Error)]
pub enum RelinkError {
    /// Filesystem operation failed.
    #[error("relink I/O error at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A sample could not be decoded during resolution.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use drumpad_model::kit::{ContentHash, Kit, Layer, SampleRef};
    use std::fs;

    fn kit_with_sample(sample_path: &str) -> (tempfile::TempDir, Kit) {
        let dir = tempfile::tempdir().unwrap();
        let audio = b"sample-bytes";
        let path = dir.path().join(sample_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, audio).unwrap();
        let mut kit = Kit::silent_default();
        let reference = SampleRef::new(
            RelativeSamplePath::new(sample_path).unwrap(),
            ContentHash::of_bytes(audio),
        );
        kit.pads[0]
            .layers
            .push(Layer::with_sample(Default::default(), reference.clone()));
        kit.pads[1]
            .layers
            .push(Layer::with_sample(Default::default(), reference));
        (dir, kit)
    }

    fn wav_bytes() -> Vec<u8> {
        wav_bytes_with_frequency(0.1)
    }

    fn wav_bytes_with_frequency(phase_step: f32) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for frame in 0..4_800 {
            writer
                .write_sample(((frame as f32 * phase_step).sin() * 8000.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
        fs::read(&path).unwrap()
    }

    fn kit_with_wav() -> (tempfile::TempDir, Kit, ContentHash) {
        let dir = tempfile::tempdir().unwrap();
        let bytes = wav_bytes();
        let path = dir.path().join("samples/kick.wav");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &bytes).unwrap();
        let decoded = decode_file(&path).unwrap();
        let hash = decoded.audio.content_hash();
        let mut kit = Kit::silent_default();
        let reference = SampleRef::new(RelativeSamplePath::new("samples/kick.wav").unwrap(), hash);
        kit.pads[0]
            .layers
            .push(Layer::with_sample(Default::default(), reference));
        (dir, kit, hash)
    }

    #[test]
    fn portable_kit_still_resolves_after_moving_the_folder() {
        let (dir, kit, hash) = kit_with_wav();
        assert!(resolve_kit_samples(&kit, dir.path()).is_complete());
        let moved = tempfile::tempdir().unwrap();
        let target = moved.path().join("moved-kit");
        fs::rename(dir.path(), &target).unwrap();
        let resolution = resolve_kit_samples(&kit, &target);
        assert!(
            resolution.is_complete(),
            "{:?}",
            resolution.problem_summary()
        );
        let entry = &resolution.samples[0];
        assert_eq!(entry.pad_ids, vec![kit.pads[0].id]);
        assert!(matches!(entry.status, SampleStatus::Ok { .. }));
        assert_eq!(entry.expected_hash, Some(hash));
    }

    #[test]
    fn missing_file_lists_affected_pads() {
        let (dir, kit) = kit_with_sample("samples/kick.wav");
        fs::remove_file(dir.path().join("samples/kick.wav")).unwrap();
        let resolution = resolve_kit_samples(&kit, dir.path());
        assert!(!resolution.is_complete());
        let missing = resolution.missing();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].pad_ids.len(), 2);
        assert_eq!(missing[0].pad_ids, vec![kit.pads[0].id, kit.pads[1].id]);
        assert!(resolution.problem_summary()[0].contains("affected pads"));
    }

    #[test]
    fn relink_from_folder_restores_playback() {
        let (dir, mut kit, _) = kit_with_wav();
        let source = dir.path().join("samples/kick.wav");
        let elsewhere = tempfile::tempdir().unwrap();
        let moved = elsewhere.path().join("kick.wav");
        fs::rename(&source, &moved).unwrap();
        let before = resolve_kit_samples(&kit, dir.path());
        assert_eq!(before.missing().len(), 1);

        let index = RelinkIndex::scan(elsewhere.path()).unwrap();
        assert_eq!(index.indexed, 1);
        let report = relink_kit(&mut kit, dir.path(), &index).unwrap();
        assert!(report.is_complete(), "{report:?}");
        assert_eq!(report.repaired.len(), 1);
        let after = resolve_kit_samples(&kit, dir.path());
        assert!(after.is_complete(), "{:?}", after.problem_summary());
        assert!(dir.path().join(kit.samples()[0].path.as_str()).is_file());
    }

    #[test]
    fn changed_file_is_detected_and_relinked_by_hash() {
        let (dir, mut kit, _) = kit_with_wav();
        let source = dir.path().join("samples/kick.wav");
        let elsewhere = tempfile::tempdir().unwrap();
        let good_copy = elsewhere.path().join("kick.wav");
        fs::copy(&source, &good_copy).unwrap();
        fs::write(&source, wav_bytes_with_frequency(0.4)).unwrap();
        let resolution = resolve_kit_samples(&kit, dir.path());
        assert_eq!(resolution.mismatched().len(), 1);

        let index = RelinkIndex::scan(elsewhere.path()).unwrap();
        let report = relink_kit(&mut kit, dir.path(), &index).unwrap();
        assert!(report.is_complete());
        assert!(resolve_kit_samples(&kit, dir.path()).is_complete());
    }

    #[test]
    fn relink_by_name_fills_the_missing_hash() {
        let (dir, mut kit, _) = kit_with_wav();
        let source = dir.path().join("samples/kick.wav");
        let mut reference = kit.pads[0].layers[0].sample.clone().unwrap();
        reference.hash = None;
        kit.pads[0].layers[0].sample = Some(reference);
        let bytes = fs::read(&source).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(elsewhere.path().join("kick.wav"), bytes).unwrap();
        fs::remove_file(&source).unwrap();

        let report = relink_kit_from_folder(&mut kit, dir.path(), elsewhere.path()).unwrap();
        assert!(report.is_complete(), "{report:?}");
        let repaired = kit.pads[0].layers[0].sample.as_ref().unwrap();
        assert!(repaired.hash.is_some(), "relink must verify the new file");
        assert!(resolve_kit_samples(&kit, dir.path()).is_complete());
    }

    #[test]
    fn relink_reports_unresolved_samples() {
        let (dir, mut kit) = kit_with_sample("samples/kick.wav");
        fs::remove_file(dir.path().join("samples/kick.wav")).unwrap();
        let empty = tempfile::tempdir().unwrap();
        let report = relink_kit_from_folder(&mut kit, dir.path(), empty.path()).unwrap();
        assert!(!report.is_complete());
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.unresolved[0].reason.contains("matching"));
    }
}
