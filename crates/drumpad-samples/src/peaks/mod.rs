//! Multi-resolution waveform peaks.
//!
//! Peaks are min/max pairs over buckets of frames. Several resolutions are
//! generated so a waveform view can pick the cheapest level for the current
//! zoom. Generation is bounded by a per-sample memory budget, and the cache is
//! invalidated by content hash plus edit revision.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use drumpad_model::kit::ContentHash;

use crate::decode::AudioData;

/// Default finest bucket size in frames.
pub const DEFAULT_BASE_BUCKET: usize = 64;
/// Default number of resolution levels.
pub const DEFAULT_LEVEL_COUNT: usize = 4;
/// Default peak memory budget per sample (64 KiB).
pub const DEFAULT_MEMORY_BUDGET_BYTES: usize = 64 * 1024;

/// Minimum and maximum sample value inside one bucket.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct MinMaxPeak {
    /// Lowest (most negative) sample.
    pub min: f32,
    /// Highest (most positive) sample.
    pub max: f32,
}

impl MinMaxPeak {
    /// Empty bucket sentinel.
    pub const SILENT: Self = Self { min: 0.0, max: 0.0 };

    /// Peak absolute amplitude, useful for meters.
    pub fn amplitude(&self) -> f32 {
        self.min.abs().max(self.max.abs())
    }
}

/// One resolution level of peaks.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PeakLevel {
    /// Frames represented by each bucket.
    pub samples_per_bucket: usize,
    /// Buckets, in time order.
    pub peaks: Vec<MinMaxPeak>,
}

impl PeakLevel {
    /// Number of buckets at this level.
    pub fn len(&self) -> usize {
        self.peaks.len()
    }

    /// True when the level has no buckets.
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }

    /// Bytes used by this level.
    pub fn memory_bytes(&self) -> usize {
        self.peaks.len() * std::mem::size_of::<MinMaxPeak>()
    }
}

/// All resolution levels for one sample.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PeakData {
    /// Source frame count.
    pub frames: usize,
    /// Levels from finest to coarsest.
    pub levels: Vec<PeakLevel>,
}

impl PeakData {
    /// Total in-memory size of the bucket arrays.
    pub fn memory_bytes(&self) -> usize {
        self.levels.iter().map(PeakLevel::memory_bytes).sum()
    }

    /// Pick the coarsest level that still resolves one bucket per pixel.
    pub fn level_for_zoom(&self, frames_per_pixel: f64) -> &PeakLevel {
        self.levels
            .iter()
            .rev()
            .find(|level| level.samples_per_bucket as f64 <= frames_per_pixel)
            .or_else(|| self.levels.first())
            .expect("peak data always has at least one level")
    }
}

/// Peak generation knobs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PeakOptions {
    /// Finest bucket size in frames.
    pub base_bucket: usize,
    /// Maximum number of levels.
    pub levels: usize,
    /// Memory budget in bytes.
    pub memory_budget_bytes: usize,
}

impl Default for PeakOptions {
    fn default() -> Self {
        Self {
            base_bucket: DEFAULT_BASE_BUCKET,
            levels: DEFAULT_LEVEL_COUNT,
            memory_budget_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
        }
    }
}

/// Generate bounded multi-resolution peaks for a sample.
pub fn generate_peaks(audio: &AudioData, options: PeakOptions) -> PeakData {
    let frames = audio.frames();
    if frames == 0 {
        return PeakData {
            frames: 0,
            levels: vec![PeakLevel {
                samples_per_bucket: options.base_bucket.max(1),
                peaks: Vec::new(),
            }],
        };
    }

    let channel_count = usize::from(audio.channels.max(1));
    let levels_wanted = options.levels.max(1);
    let bucket_bytes = std::mem::size_of::<MinMaxPeak>();
    let budget_buckets = if options.memory_budget_bytes == 0 {
        1
    } else {
        (options.memory_budget_bytes / bucket_bytes).max(1)
    };
    // The total bucket count is at most base * (1 + 1/4 + 1/16 + ...) = 4/3.
    let min_base = frames
        .saturating_mul(levels_wanted)
        .div_ceil(budget_buckets.max(1))
        .max(1);
    let mut bucket = options.base_bucket.max(1).max(min_base);

    // Mono display signal: average the channels frame by frame.
    let mono: Vec<f32> = if channel_count == 1 {
        audio.samples.clone()
    } else {
        let scale = 1.0 / channel_count as f32;
        (0..frames)
            .map(|frame| {
                let start = frame * channel_count;
                audio.samples[start..start + channel_count]
                    .iter()
                    .sum::<f32>()
                    * scale
            })
            .collect()
    };

    let mut levels = Vec::with_capacity(levels_wanted);
    for _ in 0..levels_wanted {
        let peaks = bucket_min_max(&mono, bucket);
        levels.push(PeakLevel {
            samples_per_bucket: bucket,
            peaks,
        });
        if bucket >= frames {
            break;
        }
        bucket = bucket.saturating_mul(4);
    }

    PeakData { frames, levels }
}

fn bucket_min_max(mono: &[f32], bucket: usize) -> Vec<MinMaxPeak> {
    if bucket == 0 || mono.is_empty() {
        return Vec::new();
    }
    mono.chunks(bucket)
        .map(|chunk| {
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            for sample in chunk {
                if *sample < min {
                    min = *sample;
                }
                if *sample > max {
                    max = *sample;
                }
            }
            if min.is_finite() && max.is_finite() {
                MinMaxPeak { min, max }
            } else {
                MinMaxPeak::SILENT
            }
        })
        .collect()
}

#[derive(Debug)]
struct CacheEntry {
    revision: u64,
    data: Arc<PeakData>,
    bytes: usize,
}

/// Bounded in-memory peak cache keyed by content hash.
#[derive(Debug)]
pub struct PeakCache {
    entries: HashMap<ContentHash, CacheEntry>,
    order: VecDeque<ContentHash>,
    used_bytes: usize,
    budget_bytes: usize,
}

impl PeakCache {
    /// Create a cache with a total byte budget.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            used_bytes: 0,
            budget_bytes,
        }
    }

    /// Cache budget in bytes.
    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    /// Bytes currently used by cached peaks.
    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Number of samples with cached peaks.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the cache has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return cached peaks, or generate and cache them for this revision.
    ///
    /// This hashes the whole buffer on every call. Waveform code that already
    /// has the content hash (for example after a cache lookup) should call
    /// [`Self::get_or_generate_hashed`] to avoid re-hashing a long sample on
    /// every frame.
    pub fn get_or_generate(
        &mut self,
        audio: &AudioData,
        revision: u64,
        options: PeakOptions,
    ) -> Arc<PeakData> {
        let hash = audio.content_hash();
        self.get_or_generate_hashed(hash, audio, revision, options)
    }

    /// Return cached peaks using a content hash the caller already knows.
    pub fn get_or_generate_hashed(
        &mut self,
        hash: ContentHash,
        audio: &AudioData,
        revision: u64,
        options: PeakOptions,
    ) -> Arc<PeakData> {
        if let Some(entry) = self.entries.get(&hash)
            && entry.revision == revision
        {
            return entry.data.clone();
        }
        if let Some(entry) = self.entries.remove(&hash) {
            self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
            self.order.retain(|key| *key != hash);
        }

        let data = Arc::new(generate_peaks(audio, options));
        let bytes = data.memory_bytes();
        self.entries.insert(
            hash,
            CacheEntry {
                revision,
                data: data.clone(),
                bytes,
            },
        );
        self.order.push_back(hash);
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.evict();
        data
    }

    /// Drop every cached revision of a sample.
    pub fn invalidate(&mut self, hash: &ContentHash) -> usize {
        if let Some(entry) = self.entries.remove(hash) {
            self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
            self.order.retain(|key| key != hash);
            1
        } else {
            0
        }
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.used_bytes = 0;
    }

    fn evict(&mut self) {
        while self.used_bytes > self.budget_bytes && self.order.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

impl Default for PeakCache {
    fn default() -> Self {
        Self::new(4 * 1024 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize) -> AudioData {
        let samples = (0..frames)
            .map(|frame| {
                if frame % 7 == 0 {
                    -1.0
                } else if frame % 5 == 0 {
                    1.0
                } else {
                    (frame as f32 / frames as f32) - 0.5
                }
            })
            .collect();
        AudioData::new(48_000, 1, samples).unwrap()
    }

    #[test]
    fn peaks_match_the_source_buckets() {
        let audio =
            AudioData::new(48_000, 1, vec![0.1, -0.2, 0.3, -0.4, 0.5, -0.6, 0.7, -0.8]).unwrap();
        let peaks = generate_peaks(
            &audio,
            PeakOptions {
                base_bucket: 2,
                levels: 2,
                memory_budget_bytes: 1 << 20,
            },
        );
        assert_eq!(
            peaks.levels[0].peaks[0],
            MinMaxPeak {
                min: -0.2,
                max: 0.1
            }
        );
        assert_eq!(
            peaks.levels[0].peaks[1],
            MinMaxPeak {
                min: -0.4,
                max: 0.3
            }
        );
        assert_eq!(
            peaks.levels[0].peaks[3],
            MinMaxPeak {
                min: -0.8,
                max: 0.7
            }
        );
        assert_eq!(peaks.levels[1].samples_per_bucket, 8);
        assert_eq!(
            peaks.levels[1].peaks[0],
            MinMaxPeak {
                min: -0.8,
                max: 0.7
            }
        );
    }

    #[test]
    fn memory_budget_is_respected() {
        let audio = ramp(1_000_000);
        let budget = 16 * 1024;
        let peaks = generate_peaks(
            &audio,
            PeakOptions {
                base_bucket: 1,
                levels: 4,
                memory_budget_bytes: budget,
            },
        );
        assert!(
            peaks.memory_bytes() <= budget,
            "peaks used {} bytes with a {budget} budget",
            peaks.memory_bytes()
        );
        assert!(!peaks.levels.is_empty());
    }

    #[test]
    fn cache_reuses_and_invalidates_by_revision() {
        let audio = ramp(10_000);
        let mut cache = PeakCache::new(1 << 20);
        let first = cache.get_or_generate(&audio, 1, PeakOptions::default());
        let again = cache.get_or_generate(&audio, 1, PeakOptions::default());
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(cache.len(), 1);

        // A new revision forces regeneration even when the hash is unchanged.
        let revised = cache.get_or_generate(&audio, 2, PeakOptions::default());
        assert!(!Arc::ptr_eq(&first, &revised));
        assert_eq!(cache.len(), 1);

        // An edit that changes the content adds a new hash; invalidating the
        // old hash is how the caller drops stale peaks.
        let mut edited = audio.clone();
        edited.samples[0] = 1.0;
        let second = cache.get_or_generate(&edited, 3, PeakOptions::default());
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.invalidate(&audio.content_hash()), 1);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.invalidate(&edited.content_hash()), 1);
        assert!(cache.is_empty());
        assert_eq!(cache.used_bytes(), 0);
    }

    #[test]
    fn hashed_lookup_avoids_rehashing_on_every_frame() {
        let audio = ramp(10_000);
        let hash = audio.content_hash();
        let mut cache = PeakCache::new(1 << 20);
        let first = cache.get_or_generate_hashed(hash, &audio, 1, PeakOptions::default());
        let again = cache.get_or_generate_hashed(hash, &audio, 1, PeakOptions::default());
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_evicts_oldest_when_over_budget() {
        let mut cache = PeakCache::new(1);
        cache.get_or_generate(&ramp(1_000), 1, PeakOptions::default());
        cache.get_or_generate(&ramp(2_000), 1, PeakOptions::default());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn zoom_selection_picks_a_usable_level() {
        let peaks = generate_peaks(
            &ramp(100_000),
            PeakOptions {
                base_bucket: 4,
                memory_budget_bytes: 1 << 20,
                ..PeakOptions::default()
            },
        );
        let fine = peaks.level_for_zoom(16.0);
        let coarse = peaks.level_for_zoom(10_000.0);
        assert!(fine.samples_per_bucket <= 16);
        assert!(coarse.samples_per_bucket >= 64);
        assert!(coarse.samples_per_bucket <= 10_000);
    }

    #[test]
    fn empty_audio_yields_an_empty_level() {
        let audio = AudioData::new(48_000, 1, Vec::new()).unwrap();
        let peaks = generate_peaks(&audio, PeakOptions::default());
        assert_eq!(peaks.frames, 0);
        assert!(peaks.levels[0].is_empty());
    }
}
