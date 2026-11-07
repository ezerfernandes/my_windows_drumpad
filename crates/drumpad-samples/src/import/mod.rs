//! Drag-and-drop scanning, import validation, and the asynchronous import
//! worker.
//!
//! The UI thread only submits paths and consumes [`ImportEvent`]s. Decoding,
//! resampling, hashing, and cache writes happen on worker threads.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use thiserror::Error;

use crate::cache::{CacheError, SampleCache, StoredSample};
use crate::decode::{AudioFormat, AudioMetadata, DecodeError, decode_file, probe_file};
use crate::resample::{self, ResampleError};

/// Default maximum accepted sample duration.
pub const DEFAULT_MAX_DURATION_SECONDS: f64 = 600.0;
/// Default maximum accepted source file size (200 MiB).
pub const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 200 * 1024 * 1024;
/// Default minimum accepted sample rate.
pub const DEFAULT_MIN_SAMPLE_RATE: u32 = 8_000;
/// Default maximum accepted sample rate.
pub const DEFAULT_MAX_SAMPLE_RATE: u32 = 192_000;
/// Default maximum accepted channel count.
pub const DEFAULT_MAX_CHANNELS: u16 = 2;
/// Default worker count for batch imports.
pub const DEFAULT_IMPORT_WORKERS: usize = 2;
/// Hard cap on files considered in one drop, to keep summaries manageable.
pub const MAX_DROP_FILES: usize = 2_048;

/// Import validation limits.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ImportLimits {
    /// Longest accepted sample in seconds.
    pub max_duration_seconds: f64,
    /// Largest accepted source file in bytes.
    pub max_file_size_bytes: u64,
    /// Lowest accepted sample rate.
    pub min_sample_rate: u32,
    /// Highest accepted sample rate.
    pub max_sample_rate: u32,
    /// Highest accepted channel count.
    pub max_channels: u16,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_duration_seconds: DEFAULT_MAX_DURATION_SECONDS,
            max_file_size_bytes: DEFAULT_MAX_FILE_SIZE_BYTES,
            min_sample_rate: DEFAULT_MIN_SAMPLE_RATE,
            max_sample_rate: DEFAULT_MAX_SAMPLE_RATE,
            max_channels: DEFAULT_MAX_CHANNELS,
        }
    }
}

/// A file that passed validation and is ready to import.
#[derive(Clone, PartialEq, Debug)]
pub struct ImportCandidate {
    /// Source path.
    pub path: PathBuf,
    /// Probed metadata.
    pub metadata: AudioMetadata,
    /// Source size in bytes.
    pub file_size: u64,
}

/// Why a dropped file was rejected.
#[derive(Debug, Error)]
pub enum ImportValidationError {
    /// The extension is not one of the supported formats.
    #[error("unsupported format `.{}`; supported: {}", extension, AudioFormat::SUPPORTED_EXTENSIONS.join(", "))]
    UnsupportedExtension {
        /// Extension without the dot.
        extension: String,
    },
    /// The path is not a regular file.
    #[error("{0} is not a regular file")]
    NotAFile(PathBuf),
    /// The file is larger than the configured limit.
    #[error("{path} is {size} bytes; the limit is {limit} bytes")]
    TooLarge {
        /// Source path.
        path: PathBuf,
        /// Actual size.
        size: u64,
        /// Configured limit.
        limit: u64,
    },
    /// The sample is longer than the configured limit.
    #[error("{path} is {duration:.1} s; the limit is {limit:.1} s")]
    TooLong {
        /// Source path.
        path: PathBuf,
        /// Actual duration.
        duration: f64,
        /// Configured limit.
        limit: f64,
    },
    /// The sample rate is outside the configured range.
    #[error("{path} is {rate} Hz; supported range is {min}..={max} Hz")]
    RateOutOfRange {
        /// Source path.
        path: PathBuf,
        /// Actual rate.
        rate: u32,
        /// Minimum.
        min: u32,
        /// Maximum.
        max: u32,
    },
    /// The channel count is above the limit.
    #[error("{path} has {channels} channels; the limit is {limit}")]
    TooManyChannels {
        /// Source path.
        path: PathBuf,
        /// Actual channels.
        channels: u16,
        /// Configured limit.
        limit: u16,
    },
    /// The file could not be decoded.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

/// A rejected dropped file with an actionable reason.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RejectedFile {
    /// Source path.
    pub path: PathBuf,
    /// Human-readable reason.
    pub reason: String,
}

/// Result of scanning a drag-and-drop payload.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct DropScan {
    /// Files ready to import.
    pub accepted: Vec<ImportCandidate>,
    /// Files that were skipped, with reasons.
    pub rejected: Vec<RejectedFile>,
    /// True when the file cap stopped the scan.
    pub truncated: bool,
}

impl DropScan {
    /// One-line summary for the UI.
    pub fn summary(&self) -> String {
        if self.rejected.is_empty() {
            format!("{} file(s) ready to import", self.accepted.len())
        } else {
            format!(
                "{} file(s) ready, {} skipped",
                self.accepted.len(),
                self.rejected.len()
            )
        }
    }
}

/// Validate one file against the import limits.
pub fn validate_import_file(
    path: &Path,
    limits: ImportLimits,
) -> Result<ImportCandidate, ImportValidationError> {
    if !path.is_file() {
        return Err(ImportValidationError::NotAFile(path.to_owned()));
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !AudioFormat::is_supported_extension(&extension) {
        return Err(ImportValidationError::UnsupportedExtension { extension });
    }
    let file_size = std::fs::metadata(path)
        .map_err(|source| {
            ImportValidationError::Decode(DecodeError::Io {
                path: path.to_owned(),
                source,
            })
        })?
        .len();
    if file_size > limits.max_file_size_bytes {
        return Err(ImportValidationError::TooLarge {
            path: path.to_owned(),
            size: file_size,
            limit: limits.max_file_size_bytes,
        });
    }
    let metadata = probe_file(path)?;
    if let Some(duration) = metadata.duration_seconds
        && duration > limits.max_duration_seconds
    {
        return Err(ImportValidationError::TooLong {
            path: path.to_owned(),
            duration,
            limit: limits.max_duration_seconds,
        });
    }
    if metadata.sample_rate < limits.min_sample_rate
        || metadata.sample_rate > limits.max_sample_rate
    {
        return Err(ImportValidationError::RateOutOfRange {
            path: path.to_owned(),
            rate: metadata.sample_rate,
            min: limits.min_sample_rate,
            max: limits.max_sample_rate,
        });
    }
    if metadata.channels > limits.max_channels {
        return Err(ImportValidationError::TooManyChannels {
            path: path.to_owned(),
            channels: metadata.channels,
            limit: limits.max_channels,
        });
    }
    Ok(ImportCandidate {
        path: path.to_owned(),
        metadata,
        file_size,
    })
}

/// Scan files and folders from a drag-and-drop payload.
pub fn scan_dropped_paths(paths: &[PathBuf], limits: ImportLimits) -> DropScan {
    let mut scan = DropScan::default();
    let mut seen = HashSet::new();
    let mut pending: Vec<PathBuf> = paths.to_vec();
    let mut visited_dirs = 0usize;

    while let Some(path) = pending.pop() {
        if scan.accepted.len() + scan.rejected.len() >= MAX_DROP_FILES {
            scan.truncated = true;
            break;
        }
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(canonical) {
            continue;
        }
        if path.is_dir() {
            visited_dirs += 1;
            if visited_dirs > 16 {
                scan.truncated = true;
                continue;
            }
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        pending.push(entry.path());
                    }
                }
                Err(error) => scan.rejected.push(RejectedFile {
                    path,
                    reason: format!("cannot read folder: {error}"),
                }),
            }
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !AudioFormat::is_supported_extension(&extension) {
            // Files inside a dropped folder are summarized; loose unknown
            // files are reported too, so the user sees why nothing happened.
            scan.rejected.push(RejectedFile {
                path,
                reason: format!("unsupported format `.{extension}`"),
            });
            continue;
        }
        match validate_import_file(&path, limits) {
            Ok(candidate) => scan.accepted.push(candidate),
            Err(error) => scan.rejected.push(RejectedFile {
                path,
                reason: error.to_string(),
            }),
        }
    }

    scan.accepted.sort_by(|a, b| a.path.cmp(&b.path));
    scan.rejected.sort_by(|a, b| a.path.cmp(&b.path));
    scan
}

/// Options for the import worker.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ImportOptions {
    /// Validation limits applied to each file.
    pub limits: ImportLimits,
    /// Resample to this rate before caching. `None` keeps the source rate.
    pub target_rate: Option<u32>,
    /// Downmix stereo (or wider) to mono before caching.
    pub downmix_mono: bool,
    /// Worker thread count.
    pub workers: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            limits: ImportLimits::default(),
            target_rate: None,
            downmix_mono: false,
            workers: DEFAULT_IMPORT_WORKERS,
        }
    }
}

/// Progress counters that can be polled from the UI thread.
#[derive(Debug, Default)]
pub struct ImportProgress {
    total: AtomicUsize,
    completed: AtomicUsize,
    succeeded: AtomicUsize,
    failed: AtomicUsize,
}

/// A snapshot of import progress.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProgressSnapshot {
    /// Total files in the batch.
    pub total: usize,
    /// Files processed so far.
    pub completed: usize,
    /// Successful files.
    pub succeeded: usize,
    /// Failed files.
    pub failed: usize,
}

impl ProgressSnapshot {
    /// Completion in the range 0.0..=1.0.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            self.completed as f32 / self.total as f32
        }
    }
}

impl ImportProgress {
    /// Current counters.
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            total: self.total.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

/// Events emitted by an import batch.
#[derive(Clone, PartialEq, Debug)]
pub enum ImportEvent {
    /// A worker started a file.
    Started {
        /// File being processed.
        path: PathBuf,
    },
    /// A file was decoded and cached.
    Imported {
        /// Source file.
        path: PathBuf,
        /// Source metadata.
        metadata: AudioMetadata,
        /// Cached result.
        stored: StoredSample,
    },
    /// A file failed; the rest of the batch continues.
    Failed {
        /// Source file.
        path: PathBuf,
        /// Actionable reason.
        error: String,
    },
    /// The batch finished.
    Finished(ImportSummary),
}

/// Final result of an import batch.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ImportSummary {
    /// Files submitted.
    pub total: usize,
    /// Files cached successfully.
    pub succeeded: usize,
    /// Files that failed.
    pub failed: usize,
    /// True when cancellation was requested.
    pub cancelled: bool,
    /// Per-file failures.
    pub failures: Vec<(PathBuf, String)>,
    /// Cached content hashes, in completion order.
    pub imported: Vec<drumpad_model::kit::ContentHash>,
}

#[derive(Default)]
struct ImportAccumulator {
    imported: Vec<drumpad_model::kit::ContentHash>,
    failures: Vec<(PathBuf, String)>,
}

/// A running import batch.
pub struct ImportBatch {
    /// Receiver for worker events.
    pub receiver: mpsc::Receiver<ImportEvent>,
    progress: Arc<ImportProgress>,
    cancel: Arc<AtomicBool>,
}

impl ImportBatch {
    /// Request cancellation. In-flight files finish their current cache write.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Poll progress without consuming events.
    pub fn progress(&self) -> ProgressSnapshot {
        self.progress.snapshot()
    }

    /// Receive one event, waiting up to `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ImportEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// Block until the batch finishes, returning its summary.
    pub fn wait(&self) -> ImportSummary {
        loop {
            match self.receiver.recv() {
                Ok(ImportEvent::Finished(summary)) => return summary,
                Ok(_) => continue,
                Err(_) => return ImportSummary::default(),
            }
        }
    }

    /// Collect every event until the batch finishes.
    pub fn collect(&self) -> (Vec<ImportEvent>, ImportSummary) {
        let mut events = Vec::new();
        loop {
            match self.receiver.recv() {
                Ok(event @ ImportEvent::Finished(_)) => {
                    let summary = match &event {
                        ImportEvent::Finished(summary) => summary.clone(),
                        _ => unreachable!(),
                    };
                    events.push(event);
                    return (events, summary);
                }
                Ok(event) => events.push(event),
                Err(_) => return (events, ImportSummary::default()),
            }
        }
    }
}

/// Import service: owns the cache and worker configuration.
#[derive(Clone)]
pub struct ImportService {
    cache: Arc<SampleCache>,
    options: ImportOptions,
}

impl fmt::Debug for ImportService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportService")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl ImportService {
    /// Create a service over a shared cache.
    pub fn new(cache: Arc<SampleCache>, options: ImportOptions) -> Self {
        Self { cache, options }
    }

    /// Cache handle.
    pub fn cache(&self) -> &Arc<SampleCache> {
        &self.cache
    }

    /// Worker options.
    pub fn options(&self) -> ImportOptions {
        self.options
    }

    /// Submit a batch and return immediately.
    pub fn submit(&self, paths: Vec<PathBuf>) -> ImportBatch {
        let total = paths.len();
        let progress = Arc::new(ImportProgress::default());
        progress.total.store(total, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let queue = Arc::new(Mutex::new(paths.into_iter().collect::<VecDeque<_>>()));
        let accumulator = Arc::new(Mutex::new(ImportAccumulator::default()));
        let workers = self.options.workers.max(1);

        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let cache = self.cache.clone();
            let options = self.options;
            let cancel = cancel.clone();
            let progress = progress.clone();
            let sender = sender.clone();
            let queue = queue.clone();
            let accumulator = accumulator.clone();
            handles.push(std::thread::spawn(move || {
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let next = {
                        let mut queue = queue.lock().expect("import queue poisoned");
                        queue.pop_front()
                    };
                    let Some(path) = next else {
                        break;
                    };
                    if sender
                        .send(ImportEvent::Started { path: path.clone() })
                        .is_err()
                    {
                        break;
                    }
                    let result = import_one(&cache, &options, &path);
                    match result {
                        Ok((metadata, stored)) => {
                            progress.succeeded.fetch_add(1, Ordering::Relaxed);
                            progress.completed.fetch_add(1, Ordering::Relaxed);
                            if let Ok(mut accumulator) = accumulator.lock() {
                                accumulator.imported.push(stored.metadata.hash);
                            }
                            if sender
                                .send(ImportEvent::Imported {
                                    path,
                                    metadata,
                                    stored,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(error) => {
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                            progress.completed.fetch_add(1, Ordering::Relaxed);
                            let message = error.to_string();
                            if let Ok(mut accumulator) = accumulator.lock() {
                                accumulator.failures.push((path.clone(), message.clone()));
                            }
                            if sender
                                .send(ImportEvent::Failed {
                                    path,
                                    error: message,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            }));
        }

        let coordinator_sender = sender.clone();
        let coordinator_cancel = cancel.clone();
        let coordinator_progress = progress.clone();
        let coordinator_cache = self.cache.clone();
        let coordinator_accumulator = accumulator.clone();
        std::thread::spawn(move || {
            for handle in handles {
                let _ = handle.join();
            }
            let cancelled = coordinator_cancel.load(Ordering::SeqCst);
            let _ = coordinator_cache.cleanup_temp_files();
            let snapshot = coordinator_progress.snapshot();
            let accumulator = coordinator_accumulator
                .lock()
                .map(|accumulator| ImportAccumulator {
                    imported: accumulator.imported.clone(),
                    failures: accumulator.failures.clone(),
                })
                .unwrap_or_default();
            let summary = ImportSummary {
                total: snapshot.total,
                succeeded: snapshot.succeeded,
                failed: snapshot.failed,
                cancelled,
                failures: accumulator.failures,
                imported: accumulator.imported,
            };
            let _ = coordinator_sender.send(ImportEvent::Finished(summary));
        });

        ImportBatch {
            receiver,
            progress,
            cancel,
        }
    }
}

fn import_one(
    cache: &SampleCache,
    options: &ImportOptions,
    path: &Path,
) -> Result<(AudioMetadata, StoredSample), ImportOneError> {
    validate_import_file(path, options.limits)?;
    let decoded = decode_file(path)?;
    let prepared = if options.downmix_mono || options.target_rate.is_some() {
        let target_rate = options.target_rate.unwrap_or(decoded.audio.sample_rate);
        resample::prepare_for_device(&decoded.audio, target_rate, options.downmix_mono)?
    } else {
        decoded.audio.clone()
    };
    let stored = cache.insert(&prepared)?;
    Ok((decoded.metadata, stored))
}

#[derive(Debug, Error)]
enum ImportOneError {
    #[error(transparent)]
    Validation(#[from] ImportValidationError),
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Resample(#[from] ResampleError),
    #[error(transparent)]
    Cache(#[from] CacheError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_wav(path: &Path, rate: u32, seconds: f64) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let frames = (seconds * f64::from(rate)) as usize;
        for frame in 0..frames {
            let value = ((frame as f32 * 0.1).sin() * 8000.0) as i16;
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn service(root: &Path, options: ImportOptions) -> ImportService {
        let cache = Arc::new(SampleCache::new(root).unwrap());
        ImportService::new(cache, options)
    }

    #[test]
    fn batch_import_reports_each_file_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("sources");
        fs::create_dir_all(&source).unwrap();
        write_wav(&source.join("a.wav"), 44_100, 0.05);
        write_wav(&source.join("b.wav"), 48_000, 0.05);
        fs::write(source.join("broken.wav"), b"not audio").unwrap();
        let paths = vec![
            source.join("a.wav"),
            source.join("broken.wav"),
            source.join("b.wav"),
        ];
        let service = service(&dir.path().join("cache"), ImportOptions::default());
        let batch = service.submit(paths);
        let (events, summary) = batch.collect();
        assert_eq!(summary.total, 3);
        assert_eq!(summary.succeeded, 2);
        assert_eq!(summary.failed, 1);
        assert!(matches!(events.last(), Some(ImportEvent::Finished(_))));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ImportEvent::Failed { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ImportEvent::Imported { .. }))
        );
        assert_eq!(service.cache().list_hashes().unwrap().len(), 2);
        assert_eq!(batch.progress().completed, 3);
    }

    #[test]
    fn target_rate_and_downmix_are_applied() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&source, spec).unwrap();
        for frame in 0..4_410 {
            let value = ((frame as f32 * 0.1).sin() * 8000.0) as i16;
            writer.write_sample(value).unwrap();
            writer.write_sample(-value).unwrap();
        }
        writer.finalize().unwrap();

        let service = service(
            &dir.path().join("cache"),
            ImportOptions {
                target_rate: Some(48_000),
                downmix_mono: true,
                ..ImportOptions::default()
            },
        );
        let batch = service.submit(vec![source]);
        let (_, summary) = batch.collect();
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.imported.len(), 1);
        let hashes = service.cache().list_hashes().unwrap();
        assert_eq!(hashes.len(), 1);
        let stored = service.cache().get(hashes[0]).unwrap();
        assert_eq!(stored.audio.sample_rate, 48_000);
        assert_eq!(stored.audio.channels, 1);
    }

    #[test]
    fn cancellation_stops_workers_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("many");
        fs::create_dir_all(&source).unwrap();
        let mut paths = Vec::new();
        for index in 0..64 {
            let path = source.join(format!("hit-{index}.wav"));
            write_wav(&path, 44_100, 0.02);
            paths.push(path);
        }
        let service = service(
            &dir.path().join("cache"),
            ImportOptions {
                workers: 1,
                ..ImportOptions::default()
            },
        );
        let batch = service.submit(paths);
        batch.cancel();
        let summary = batch.wait();
        assert!(summary.succeeded + summary.failed <= summary.total);
        assert_eq!(service.cache().cleanup_temp_files().unwrap(), 0);
    }

    #[test]
    fn folder_scan_separates_supported_and_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        write_wav(&dir.path().join("good.wav"), 44_100, 0.05);
        fs::write(dir.path().join("notes.txt"), b"hello").unwrap();
        let scan = scan_dropped_paths(&[dir.path().to_owned()], ImportLimits::default());
        assert_eq!(scan.accepted.len(), 1);
        assert_eq!(scan.rejected.len(), 1);
        assert_eq!(scan.rejected[0].path.file_name().unwrap(), "notes.txt");
        assert!(scan.rejected[0].reason.contains("unsupported"));
        assert!(scan.summary().contains("1 file(s) ready"));
    }

    #[test]
    fn limits_reject_actionable_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.wav");
        write_wav(&path, 44_100, 0.2);
        let limits = ImportLimits {
            max_duration_seconds: 0.1,
            ..ImportLimits::default()
        };
        let error = validate_import_file(&path, limits).unwrap_err();
        assert!(matches!(error, ImportValidationError::TooLong { .. }));
        assert!(error.to_string().contains("limit"));

        let limits = ImportLimits {
            max_channels: 0,
            ..ImportLimits::default()
        };
        let error = validate_import_file(&path, limits).unwrap_err();
        assert!(matches!(
            error,
            ImportValidationError::TooManyChannels { .. }
        ));
    }

    #[test]
    fn corrupt_file_validation_is_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.wav");
        fs::write(&path, b"RIFF\x00\x00").unwrap();
        let error = validate_import_file(&path, ImportLimits::default()).unwrap_err();
        assert!(matches!(error, ImportValidationError::Decode(_)));
    }
}
