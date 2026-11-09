//! Content-addressed sample cache.
//!
//! Decoded PCM is stored by SHA-256 of its canonical content and survives
//! restarts. Resampled variants are cached separately by source hash and
//! target rate. Cleanup only removes entries that no kit references.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use drumpad_model::kit::ContentHash;
use drumpad_model::kit::persistence::atomic_write;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::decode::{AudioData, AudioFormat};
use crate::resample::{self, ResampleError};

/// Metadata stored beside a raw PCM blob.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// Hash of the stored decoded PCM.
    pub hash: ContentHash,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Frame count.
    pub frames: u64,
    /// Duration in seconds.
    pub duration_seconds: f64,
    /// Original container when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<AudioFormat>,
}

impl CacheMetadata {
    fn new(audio: &AudioData) -> Self {
        Self {
            hash: audio.content_hash(),
            sample_rate: audio.sample_rate,
            channels: audio.channels,
            frames: audio.frames() as u64,
            duration_seconds: audio.duration_seconds(),
            source_format: None,
        }
    }
}

/// A cached sample loaded into memory.
#[derive(Clone, PartialEq, Debug)]
pub struct StoredSample {
    /// Decoded PCM.
    pub audio: AudioData,
    /// Stored metadata.
    pub metadata: CacheMetadata,
    /// Path of the raw PCM file.
    pub path: PathBuf,
}

/// Result of orphan cleanup.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CleanupReport {
    /// Removed object hashes.
    pub removed: Vec<ContentHash>,
    /// Removed resampled variants.
    pub removed_variants: usize,
    /// Bytes reclaimed.
    pub bytes_freed: u64,
}

/// Cache failures.
#[derive(Debug, Error)]
pub enum CacheError {
    /// Filesystem operation failed.
    #[error("sample cache I/O error at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The requested hash is not cached.
    #[error("sample {0} is not in the cache")]
    NotFound(ContentHash),
    /// Stored data does not match its metadata.
    #[error("cached sample {hash} is corrupt: {detail}")]
    Corrupt {
        /// Content hash.
        hash: ContentHash,
        /// Explanation.
        detail: String,
    },
    /// Metadata JSON could not be read or written.
    #[error("cached sample {hash} metadata is invalid: {detail}")]
    Metadata {
        /// Content hash.
        hash: ContentHash,
        /// Explanation.
        detail: String,
    },
    /// Resampling failed.
    #[error(transparent)]
    Resample(#[from] ResampleError),
}

fn io_error(path: &Path, source: std::io::Error) -> CacheError {
    CacheError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Content-addressed sample cache rooted at a folder.
#[derive(Clone, Debug)]
pub struct SampleCache {
    root: PathBuf,
}

impl SampleCache {
    /// Open (or create) a cache at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, CacheError> {
        let root = root.into();
        fs::create_dir_all(root.join("objects")).map_err(|error| io_error(&root, error))?;
        fs::create_dir_all(root.join("resampled")).map_err(|error| io_error(&root, error))?;
        Ok(Self { root })
    }

    /// Cache root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory for one content hash.
    pub fn object_dir(&self, hash: ContentHash) -> PathBuf {
        self.root.join("objects").join(hash.to_hex())
    }

    /// Directory for a resampled variant.
    pub fn variant_dir(&self, hash: ContentHash, target_rate: u32) -> PathBuf {
        self.root
            .join("resampled")
            .join(hash.to_hex())
            .join(target_rate.to_string())
    }

    /// Insert a decoded sample, reusing an existing identical entry.
    pub fn insert(&self, audio: &AudioData) -> Result<StoredSample, CacheError> {
        let hash = audio.content_hash();
        let dir = self.object_dir(hash);
        if let Ok(existing) = self.load_object(hash) {
            return Ok(existing);
        }
        fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
        let pcm_path = dir.join("pcm.f32");
        atomic_write(&pcm_path, &encode_pcm(&audio.samples)).map_err(|error| CacheError::Io {
            path: pcm_path.clone(),
            source: std::io::Error::other(error.to_string()),
        })?;
        let metadata = CacheMetadata::new(audio);
        let metadata_path = dir.join("meta.json");
        let json = serde_json::to_vec_pretty(&metadata).map_err(|error| CacheError::Metadata {
            hash,
            detail: error.to_string(),
        })?;
        atomic_write(&metadata_path, &json).map_err(|error| CacheError::Io {
            path: metadata_path,
            source: std::io::Error::other(error.to_string()),
        })?;
        Ok(StoredSample {
            audio: audio.clone(),
            metadata,
            path: pcm_path,
        })
    }

    /// Load a cached sample by content hash.
    pub fn get(&self, hash: ContentHash) -> Result<StoredSample, CacheError> {
        self.load_object(hash)
    }

    /// True when the sample exists and its metadata is readable.
    pub fn contains(&self, hash: ContentHash) -> bool {
        self.load_object(hash).is_ok()
    }

    /// Insert a resampled variant under a source hash and target rate.
    pub fn insert_resampled(
        &self,
        source_hash: ContentHash,
        target_rate: u32,
        audio: &AudioData,
    ) -> Result<StoredSample, CacheError> {
        let dir = self.variant_dir(source_hash, target_rate);
        fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
        let pcm_path = dir.join("pcm.f32");
        atomic_write(&pcm_path, &encode_pcm(&audio.samples)).map_err(|error| CacheError::Io {
            path: pcm_path.clone(),
            source: std::io::Error::other(error.to_string()),
        })?;
        let mut metadata = CacheMetadata::new(audio);
        metadata.source_format = None;
        let metadata_path = dir.join("meta.json");
        let json = serde_json::to_vec_pretty(&metadata).map_err(|error| CacheError::Metadata {
            hash: source_hash,
            detail: error.to_string(),
        })?;
        atomic_write(&metadata_path, &json).map_err(|error| CacheError::Io {
            path: metadata_path,
            source: std::io::Error::other(error.to_string()),
        })?;
        Ok(StoredSample {
            audio: audio.clone(),
            metadata,
            path: pcm_path,
        })
    }

    /// Load a cached resampled variant.
    pub fn get_resampled(
        &self,
        source_hash: ContentHash,
        target_rate: u32,
    ) -> Result<Option<StoredSample>, CacheError> {
        let dir = self.variant_dir(source_hash, target_rate);
        if !dir.is_dir() {
            return Ok(None);
        }
        match self.load_dir(&dir, source_hash) {
            Ok(stored) => Ok(Some(stored)),
            Err(CacheError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Return a cached resample, generating it on first use.
    pub fn resample_cached(
        &self,
        source_hash: ContentHash,
        target_rate: u32,
    ) -> Result<StoredSample, CacheError> {
        let source = self.get(source_hash)?;
        if source.audio.sample_rate == target_rate {
            return Ok(source);
        }
        if let Some(cached) = self.get_resampled(source_hash, target_rate)? {
            return Ok(cached);
        }
        let resampled = resample::resample(&source.audio, target_rate)?;
        self.insert_resampled(source_hash, target_rate, &resampled)
    }

    /// List all cached source hashes.
    pub fn list_hashes(&self) -> Result<Vec<ContentHash>, CacheError> {
        let dir = self.root.join("objects");
        let entries = fs::read_dir(&dir).map_err(|error| io_error(&dir, error))?;
        let mut hashes = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&dir, error))?;
            if !entry.path().is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && let Ok(hash) = ContentHash::from_hex(name)
            {
                hashes.push(hash);
            }
        }
        Ok(hashes)
    }

    /// Remove objects and variants not present in `referenced`.
    pub fn cleanup(&self, referenced: &HashSet<ContentHash>) -> Result<CleanupReport, CacheError> {
        let mut report = CleanupReport::default();
        for hash in self.list_hashes()? {
            if !referenced.contains(&hash) {
                let dir = self.object_dir(hash);
                report.bytes_freed = report.bytes_freed.saturating_add(dir_size(&dir));
                fs::remove_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
                report.removed.push(hash);
            }
        }
        let resampled_root = self.root.join("resampled");
        if resampled_root.is_dir() {
            let entries =
                fs::read_dir(&resampled_root).map_err(|error| io_error(&resampled_root, error))?;
            for entry in entries {
                let entry = entry.map_err(|error| io_error(&resampled_root, error))?;
                if !entry.path().is_dir() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Ok(hash) = ContentHash::from_hex(&name) else {
                    continue;
                };
                if referenced.contains(&hash) {
                    continue;
                }
                let count = fs::read_dir(entry.path())
                    .map(|children| children.count())
                    .unwrap_or(0);
                report.removed_variants += count;
                report.bytes_freed = report.bytes_freed.saturating_add(dir_size(&entry.path()));
                fs::remove_dir_all(entry.path()).map_err(|error| io_error(&entry.path(), error))?;
            }
        }
        Ok(report)
    }

    /// Delete temporary files left behind by an interrupted write.
    pub fn cleanup_temp_files(&self) -> Result<usize, CacheError> {
        fn walk(dir: &Path, removed: &mut usize) -> Result<(), CacheError> {
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(io_error(dir, error)),
            };
            for entry in entries {
                let entry = entry.map_err(|error| io_error(dir, error))?;
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, removed)?;
                } else if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".tmp-"))
                {
                    fs::remove_file(&path).map_err(|error| io_error(&path, error))?;
                    *removed += 1;
                }
            }
            Ok(())
        }
        let mut removed = 0;
        walk(&self.root, &mut removed)?;
        Ok(removed)
    }

    fn load_object(&self, hash: ContentHash) -> Result<StoredSample, CacheError> {
        let dir = self.object_dir(hash);
        if !dir.is_dir() {
            return Err(CacheError::NotFound(hash));
        }
        self.load_dir(&dir, hash)
    }

    fn load_dir(&self, dir: &Path, source_hash: ContentHash) -> Result<StoredSample, CacheError> {
        let metadata_path = dir.join("meta.json");
        let metadata_bytes = fs::read(&metadata_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CacheError::NotFound(source_hash)
            } else {
                io_error(&metadata_path, error)
            }
        })?;
        let metadata: CacheMetadata =
            serde_json::from_slice(&metadata_bytes).map_err(|error| CacheError::Metadata {
                hash: source_hash,
                detail: error.to_string(),
            })?;
        let pcm_path = dir.join("pcm.f32");
        let pcm_bytes = fs::read(&pcm_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CacheError::NotFound(source_hash)
            } else {
                io_error(&pcm_path, error)
            }
        })?;
        let samples = decode_pcm(&pcm_bytes).map_err(|detail| CacheError::Corrupt {
            hash: source_hash,
            detail,
        })?;
        let expected = metadata.frames as usize * usize::from(metadata.channels);
        if samples.len() != expected {
            return Err(CacheError::Corrupt {
                hash: source_hash,
                detail: format!("expected {expected} samples, found {}", samples.len()),
            });
        }
        let audio =
            AudioData::new(metadata.sample_rate, metadata.channels, samples).map_err(|error| {
                CacheError::Corrupt {
                    hash: source_hash,
                    detail: error.to_string(),
                }
            })?;
        if audio.content_hash() != metadata.hash {
            return Err(CacheError::Corrupt {
                hash: source_hash,
                detail: "content hash does not match the stored metadata".into(),
            });
        }
        Ok(StoredSample {
            audio,
            metadata,
            path: pcm_path,
        })
    }
}

fn encode_pcm(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn decode_pcm(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "raw PCM length {} is not 4-byte aligned",
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(metadata) = entry.metadata() {
            total += metadata.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(rate: u32, frames: usize) -> AudioData {
        let samples = (0..frames)
            .map(|frame| ((frame as f32) * 0.01).sin() * 0.5)
            .collect();
        AudioData::new(rate, 1, samples).unwrap()
    }

    #[test]
    fn insert_is_content_addressed_and_deduped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let sample = audio(48_000, 1_000);
        let first = cache.insert(&sample).unwrap();
        let second = cache.insert(&sample).unwrap();
        assert_eq!(first.path, second.path);
        assert_eq!(cache.list_hashes().unwrap().len(), 1);
        assert_eq!(cache.get(sample.content_hash()).unwrap().audio, sample);
    }

    #[test]
    fn cache_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let sample = audio(44_100, 500);
        {
            let cache = SampleCache::new(dir.path()).unwrap();
            cache.insert(&sample).unwrap();
        }
        let reopened = SampleCache::new(dir.path()).unwrap();
        let stored = reopened.get(sample.content_hash()).unwrap();
        assert_eq!(stored.audio, sample);
        assert_eq!(stored.metadata.sample_rate, 44_100);
    }

    #[test]
    fn resampled_cache_is_reused_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let sample = audio(44_100, 4_410);
        let source_hash = sample.content_hash();
        let first_path;
        {
            let cache = SampleCache::new(dir.path()).unwrap();
            cache.insert(&sample).unwrap();
            let resampled = cache.resample_cached(source_hash, 48_000).unwrap();
            assert_eq!(resampled.audio.sample_rate, 48_000);
            first_path = resampled.path;
        }
        let reopened = SampleCache::new(dir.path()).unwrap();
        let cached = reopened
            .get_resampled(source_hash, 48_000)
            .unwrap()
            .expect("resampled variant should survive restart");
        assert_eq!(cached.path, first_path);
        assert_eq!(cached.audio.sample_rate, 48_000);
        let again = reopened.resample_cached(source_hash, 48_000).unwrap();
        assert_eq!(again.path, first_path);
    }

    #[test]
    fn cleanup_never_removes_referenced_samples() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let keep = audio(48_000, 300);
        let drop = audio(48_000, 600);
        cache.insert(&keep).unwrap();
        cache.insert(&drop).unwrap();
        cache.resample_cached(drop.content_hash(), 44_100).unwrap();

        let referenced: HashSet<_> = [keep.content_hash()].into_iter().collect();
        let report = cache.cleanup(&referenced).unwrap();
        assert_eq!(report.removed, vec![drop.content_hash()]);
        assert!(cache.contains(keep.content_hash()));
        assert!(!cache.contains(drop.content_hash()));
        assert!(report.bytes_freed > 0);
    }

    #[test]
    fn corrupt_pcm_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let sample = audio(48_000, 100);
        let stored = cache.insert(&sample).unwrap();
        let mut bytes = fs::read(&stored.path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&stored.path, bytes).unwrap();
        let error = cache.get(sample.content_hash()).unwrap_err();
        assert!(matches!(error, CacheError::Corrupt { .. }));
    }

    #[test]
    fn temp_files_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let temp = cache.root().join("objects/.pcm.f32.tmp-1-2");
        fs::write(&temp, b"junk").unwrap();
        assert_eq!(cache.cleanup_temp_files().unwrap(), 1);
        assert!(!temp.exists());
    }

    #[test]
    fn cold_misses_and_same_rate_short_circuit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let sample = audio(44_100, 100);
        let hash = sample.content_hash();
        assert!(cache.get_resampled(hash, 48_000).unwrap().is_none());
        assert!(!cache.contains(hash));
        cache.insert(&sample).unwrap();
        let same = cache.resample_cached(hash, 44_100).unwrap();
        assert_eq!(same.audio, sample);
        assert_eq!(same.path, cache.object_dir(hash).join("pcm.f32"));
    }

    #[test]
    fn list_hashes_ignores_foreign_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        fs::create_dir_all(cache.root().join("objects/not-a-hash")).unwrap();
        fs::write(cache.root().join("objects/stray.txt"), b"x").unwrap();
        let sample = audio(48_000, 10);
        cache.insert(&sample).unwrap();
        assert_eq!(cache.list_hashes().unwrap(), vec![sample.content_hash()]);
    }

    #[test]
    fn missing_and_corrupt_cache_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let sample = audio(48_000, 100);
        let stored = cache.insert(&sample).unwrap();
        let hash = sample.content_hash();
        let metadata_path = stored.path.with_file_name("meta.json");

        fs::remove_file(&metadata_path).unwrap();
        assert!(matches!(cache.get(hash), Err(CacheError::NotFound(_))));
        cache.insert(&sample).unwrap();

        fs::write(&metadata_path, b"{not json").unwrap();
        assert!(matches!(cache.get(hash), Err(CacheError::Metadata { .. })));
        cache.insert(&sample).unwrap();

        fs::remove_file(&stored.path).unwrap();
        assert!(matches!(cache.get(hash), Err(CacheError::NotFound(_))));
        cache.insert(&sample).unwrap();

        let mut bytes = fs::read(&stored.path).unwrap();
        bytes.extend_from_slice(&0.0f32.to_le_bytes());
        fs::write(&stored.path, bytes).unwrap();
        assert!(matches!(cache.get(hash), Err(CacheError::Corrupt { .. })));
        cache.insert(&sample).unwrap();

        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        metadata["sample_rate"] = serde_json::json!(0);
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(matches!(cache.get(hash), Err(CacheError::Corrupt { .. })));
    }

    #[test]
    fn unaligned_pcm_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let sample = audio(48_000, 100);
        let stored = cache.insert(&sample).unwrap();
        fs::write(&stored.path, [1u8, 2, 3]).unwrap();
        assert!(matches!(
            cache.get(sample.content_hash()),
            Err(CacheError::Corrupt { .. })
        ));
    }

    #[test]
    fn missing_variant_files_are_a_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SampleCache::new(dir.path()).unwrap();
        let sample = audio(44_100, 10);
        let hash = sample.content_hash();
        fs::create_dir_all(cache.variant_dir(hash, 48_000)).unwrap();
        assert!(cache.get_resampled(hash, 48_000).unwrap().is_none());
    }

    #[test]
    fn temp_cleanup_tolerates_a_missing_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let cache = SampleCache::new(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(cache.cleanup_temp_files().unwrap(), 0);
    }
}
