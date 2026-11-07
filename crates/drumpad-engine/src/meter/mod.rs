//! Peak/RMS metering and voice-count diagnostics.
//!
//! The audio thread accumulates peak and mean-square sums per block into a
//! preallocated [`Meter`], then publishes the finished values into atomic
//! cells. The UI reads those atomics; it never shares a lock with the audio
//! thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Linear peak/RMS values for one finished block.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterValues {
    /// Peak absolute sample on the left channel.
    pub peak_left: f32,
    /// Peak absolute sample on the right channel.
    pub peak_right: f32,
    /// RMS of the left channel.
    pub rms_left: f32,
    /// RMS of the right channel.
    pub rms_right: f32,
}

impl MeterValues {
    /// Peak of both channels in dBFS (worst channel).
    pub fn peak_db(&self) -> f32 {
        linear_to_db(self.peak_left.max(self.peak_right))
    }

    /// RMS of both channels in dBFS (worst channel).
    pub fn rms_db(&self) -> f32 {
        linear_to_db(self.rms_left.max(self.rms_right))
    }
}

/// Convert linear amplitude to dBFS, floored at -120 dB.
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        -120.0
    } else {
        (20.0 * linear.log10()).max(-120.0)
    }
}

/// Convert dBFS to linear amplitude.
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Block accumulator. Lives on the audio thread.
#[derive(Debug, Clone)]
pub struct Meter {
    peak_left: f32,
    peak_right: f32,
    sum_left: f64,
    sum_right: f64,
    frames: u32,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl Meter {
    /// Create a fresh accumulator.
    pub const fn new() -> Self {
        Self {
            peak_left: 0.0,
            peak_right: 0.0,
            sum_left: 0.0,
            sum_right: 0.0,
            frames: 0,
        }
    }

    /// Reset the accumulator at the start of a block.
    pub fn begin_block(&mut self) {
        self.peak_left = 0.0;
        self.peak_right = 0.0;
        self.sum_left = 0.0;
        self.sum_right = 0.0;
        self.frames = 0;
    }

    /// Add one finished stereo frame.
    #[inline]
    pub fn accumulate(&mut self, left: f32, right: f32) {
        let l = left.abs();
        let r = right.abs();
        if l > self.peak_left {
            self.peak_left = l;
        }
        if r > self.peak_right {
            self.peak_right = r;
        }
        self.sum_left += (left as f64) * (left as f64);
        self.sum_right += (right as f64) * (right as f64);
        self.frames += 1;
    }

    /// Finish the block and return its values.
    pub fn finish_block(&mut self) -> MeterValues {
        let frames = self.frames.max(1) as f64;
        let values = MeterValues {
            peak_left: self.peak_left,
            peak_right: self.peak_right,
            rms_left: (self.sum_left / frames).sqrt() as f32,
            rms_right: (self.sum_right / frames).sqrt() as f32,
        };
        self.begin_block();
        values
    }
}

/// Atomically published meters plus voice count for UI and diagnostics.
#[derive(Debug, Default)]
pub struct MeterShared {
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    rms_left: AtomicU32,
    rms_right: AtomicU32,
    voice_count: AtomicU32,
    active_voices: AtomicU32,
    blocks: AtomicU32,
}

impl MeterShared {
    /// Create a zeroed shared meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish finished values from the audio thread.
    pub fn store(&self, values: MeterValues) {
        self.peak_left
            .store(values.peak_left.to_bits(), Ordering::Relaxed);
        self.peak_right
            .store(values.peak_right.to_bits(), Ordering::Relaxed);
        self.rms_left
            .store(values.rms_left.to_bits(), Ordering::Relaxed);
        self.rms_right
            .store(values.rms_right.to_bits(), Ordering::Relaxed);
        self.blocks.fetch_add(1, Ordering::Relaxed);
    }

    /// Publish the active voice count and the configured limit.
    pub fn store_voice_counts(&self, active: usize, limit: usize) {
        self.active_voices.store(active as u32, Ordering::Relaxed);
        self.voice_count.store(limit as u32, Ordering::Relaxed);
    }

    /// Read the latest values. Safe from any thread.
    pub fn load(&self) -> MeterValues {
        MeterValues {
            peak_left: f32::from_bits(self.peak_left.load(Ordering::Relaxed)),
            peak_right: f32::from_bits(self.peak_right.load(Ordering::Relaxed)),
            rms_left: f32::from_bits(self.rms_left.load(Ordering::Relaxed)),
            rms_right: f32::from_bits(self.rms_right.load(Ordering::Relaxed)),
        }
    }

    /// Active voice count.
    pub fn active_voices(&self) -> u32 {
        self.active_voices.load(Ordering::Relaxed)
    }

    /// Configured voice limit.
    pub fn voice_limit(&self) -> u32 {
        self.voice_count.load(Ordering::Relaxed)
    }

    /// Number of blocks published since construction.
    pub fn blocks(&self) -> u32 {
        self.blocks.load(Ordering::Relaxed)
    }

    /// Reset the published meters without counting it as a new block.
    pub fn reset(&self) {
        self.peak_left.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.peak_right.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.rms_left.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.rms_right.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

/// Cloneable reader handed to the UI thread.
#[derive(Debug, Clone)]
pub struct MeterReader {
    shared: Arc<MeterShared>,
}

impl MeterReader {
    /// Wrap a shared meter.
    pub fn new(shared: Arc<MeterShared>) -> Self {
        Self { shared }
    }

    /// Latest block values.
    pub fn values(&self) -> MeterValues {
        self.shared.load()
    }

    /// Active voice count.
    pub fn active_voices(&self) -> u32 {
        self.shared.active_voices()
    }

    /// Configured voice limit.
    pub fn voice_limit(&self) -> u32 {
        self.shared.voice_limit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn peak_and_rms_of_a_sine_are_within_one_db() {
        let rate = 48_000.0;
        let amplitude = 0.5;
        let mut meter = Meter::new();
        meter.begin_block();
        for i in 0..48_000 {
            let s = (2.0 * PI * 1000.0 * i as f32 / rate).sin() * amplitude;
            meter.accumulate(s, s);
        }
        let values = meter.finish_block();
        // True peak of a discretely sampled sine is close to the amplitude.
        assert!((values.peak_left - 0.5).abs() < 0.01);
        // RMS of a sine is amplitude / sqrt(2).
        let expected_rms = amplitude / 2f32.sqrt();
        assert!((values.rms_left - expected_rms).abs() < 0.01);
        assert!(
            (values.peak_db() - (-6.0206)).abs() < 0.1,
            "peak dB {}",
            values.peak_db()
        );
        assert!(
            (values.rms_db() - (-9.0309)).abs() < 0.1,
            "rms dB {}",
            values.rms_db()
        );
    }

    #[test]
    fn silence_meters_floor_at_minus_120_db() {
        let mut meter = Meter::new();
        meter.begin_block();
        for _ in 0..128 {
            meter.accumulate(0.0, 0.0);
        }
        let values = meter.finish_block();
        assert_eq!(values.peak_db(), -120.0);
        assert_eq!(values.rms_db(), -120.0);
    }

    #[test]
    fn shared_meter_round_trips_and_is_thread_safe() {
        let shared = Arc::new(MeterShared::new());
        shared.store(MeterValues {
            peak_left: 0.25,
            peak_right: 0.5,
            rms_left: 0.1,
            rms_right: 0.2,
        });
        shared.store_voice_counts(17, 64);
        let reader = MeterReader::new(Arc::clone(&shared));
        let handle = std::thread::spawn(move || {
            let v = reader.values();
            (
                v.peak_left,
                v.peak_right,
                reader.active_voices(),
                reader.voice_limit(),
            )
        });
        let (pl, pr, active, limit) = handle.join().unwrap();
        assert_eq!((pl, pr), (0.25, 0.5));
        assert_eq!((active, limit), (17, 64));
        assert_eq!(shared.blocks(), 1);
    }

    #[test]
    fn db_conversions_round_trip() {
        for db in [-60.0, -12.0, -6.0, 0.0] {
            let linear = db_to_linear(db);
            assert!((linear_to_db(linear) - db).abs() < 1e-4);
        }
    }
}
