//! Sample pipeline: decode, resample, peaks, content cache, async import, and
//! missing-sample relinking.
//!
//! Everything here runs off the audio thread. Decoded buffers are immutable
//! once returned, which lets the engine publish them safely.

pub mod cache;
pub mod decode;
pub mod error;
pub mod import;
pub mod kit_import;
pub mod peaks;
pub mod relink;
pub mod resample;

pub use cache::{CacheError, CacheMetadata, CleanupReport, SampleCache, StoredSample};
pub use decode::{
    AudioData, AudioDataError, AudioFormat, AudioMetadata, DecodeError, DecodedAudio,
    DecodedSample, MAX_SAMPLE_CHANNELS, SampleBuffer, SampleBufferError, decode_file, probe_file,
};
pub use import::{
    DEFAULT_IMPORT_WORKERS, DEFAULT_MAX_CHANNELS, DEFAULT_MAX_DURATION_SECONDS,
    DEFAULT_MAX_FILE_SIZE_BYTES, DEFAULT_MAX_SAMPLE_RATE, DEFAULT_MIN_SAMPLE_RATE, DropScan,
    ImportBatch, ImportCandidate, ImportEvent, ImportLimits, ImportOptions, ImportProgress,
    ImportService, ImportSummary, ImportValidationError, ProgressSnapshot, RejectedFile,
    scan_dropped_paths, validate_import_file,
};
pub use kit_import::{ImportedKit, KitImportError, import_folder_into_new_kit};
pub use peaks::{
    DEFAULT_BASE_BUCKET, DEFAULT_LEVEL_COUNT, DEFAULT_MEMORY_BUDGET_BYTES, MinMaxPeak, PeakCache,
    PeakData, PeakLevel, PeakOptions, generate_peaks,
};
pub use relink::{
    RelinkError, RelinkIndex, RelinkRepair, RelinkReport, ResolvedSample, SampleResolution,
    SampleStatus, UnresolvedSample, relink_kit, relink_kit_from_folder, resolve_kit_samples,
};
pub use resample::{
    Downmix, ResampleError, content_hash, downmix_to_mono, prepare_for_device, resample,
    resample_with_chunk,
};
