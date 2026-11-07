//! Latency sampling for the keyboard/MIDI capture path.
//!
//! The recorder measures capture latency: from the moment the platform event
//! reaches our handler to the moment the resulting event is in the queue that
//! the audio callback drains. It deliberately does not try to measure the OS
//! or driver delay; the spike harness in `examples/keyboard_latency_spike.rs`
//! documents how the full budget is assembled.
//!
//! Recording happens off the audio thread. The data is a plain `Vec<u32>` so
//! the harness can print percentiles after a run and tests can assert exact
//! numbers.

use std::time::Instant;

/// Percentile summary of a latency run, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyPercentiles {
    /// Number of samples.
    pub count: usize,
    /// Smallest sample.
    pub min: u32,
    /// Median sample.
    pub p50: u32,
    /// 90th percentile.
    pub p90: u32,
    /// 95th percentile.
    pub p95: u32,
    /// 99th percentile.
    pub p99: u32,
    /// Largest sample.
    pub max: u32,
    /// Arithmetic mean, rounded down.
    pub mean: u32,
}

impl LatencyPercentiles {
    /// True when the 99th percentile is below the keyboard budget of 2000 µs.
    #[must_use]
    pub fn within_key_budget(self) -> bool {
        self.p99 < 2_000
    }
}

/// Bounded collector of microsecond latency samples.
#[derive(Debug, Clone)]
pub struct LatencyRecorder {
    samples_us: Vec<u32>,
    capacity: usize,
    overflowed: u64,
}

impl LatencyRecorder {
    /// Create a recorder that retains at most `capacity` samples.
    ///
    /// Extra samples are counted in [`LatencyRecorder::overflowed`] but not
    /// retained, so a long soak test cannot grow without bound; percentile
    /// output then covers the first window.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            samples_us: Vec::with_capacity(capacity),
            capacity,
            overflowed: 0,
        }
    }

    /// Default capacity: 10_000 hits, enough for stable p99.
    #[must_use]
    pub fn default_capacity() -> Self {
        Self::new(10_000)
    }

    /// Record one sample.
    pub fn record(&mut self, micros: u32) {
        if self.samples_us.len() < self.capacity {
            self.samples_us.push(micros);
        } else {
            self.overflowed += 1;
        }
    }

    /// Record the elapsed time of `work` and return its value.
    pub fn measure<T>(&mut self, work: impl FnOnce() -> T) -> T {
        let probe = LatencyProbe::start();
        let value = work();
        probe.finish(self);
        value
    }

    /// Drop all samples and reset the overflow counter.
    pub fn clear(&mut self) {
        self.samples_us.clear();
        self.overflowed = 0;
    }

    /// Samples discarded because the recorder was full.
    #[must_use]
    pub fn overflowed(&self) -> u64 {
        self.overflowed
    }

    /// Samples collected so far.
    #[must_use]
    pub fn samples(&self) -> &[u32] {
        &self.samples_us
    }

    /// Number of retained samples.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples_us.len()
    }

    /// True when nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples_us.is_empty()
    }

    /// Percentile summary, or `None` when no samples exist.
    #[must_use]
    pub fn percentiles(&self) -> Option<LatencyPercentiles> {
        if self.samples_us.is_empty() {
            return None;
        }
        let mut sorted = self.samples_us.clone();
        sorted.sort_unstable();
        let sum: u64 = sorted.iter().map(|value| u64::from(*value)).sum();
        Some(LatencyPercentiles {
            count: sorted.len(),
            min: sorted[0],
            p50: percentile(&sorted, 50),
            p90: percentile(&sorted, 90),
            p95: percentile(&sorted, 95),
            p99: percentile(&sorted, 99),
            max: *sorted.last().expect("non-empty"),
            mean: (sum / sorted.len() as u64) as u32,
        })
    }
}

/// Nearest-rank percentile over an ascending slice.
///
/// `percentile` is clamped to `1..=100`; the result is always an element of
/// the slice, which keeps the p99 of a small run honest.
#[must_use]
pub fn percentile(sorted_ascending: &[u32], percentile: usize) -> u32 {
    assert!(!sorted_ascending.is_empty(), "percentile of empty slice");
    let rank = (percentile.clamp(1, 100) * sorted_ascending.len()).div_ceil(100);
    sorted_ascending[rank.saturating_sub(1).min(sorted_ascending.len() - 1)]
}

/// Measures one span with the platform's monotonic clock.
#[derive(Debug, Clone, Copy)]
pub struct LatencyProbe {
    started: Instant,
}

impl LatencyProbe {
    /// Begin a measurement.
    #[must_use]
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    /// Elapsed microseconds, rounded up so sub-microsecond work is not
    /// reported as zero, saturating at `u32::MAX`.
    #[must_use]
    pub fn elapsed_us(self) -> u32 {
        let nanos = self.started.elapsed().as_nanos();
        u32::try_from(nanos.div_ceil(1_000)).unwrap_or(u32::MAX)
    }

    /// Finish the measurement, record it, and return the elapsed value.
    pub fn finish(self, recorder: &mut LatencyRecorder) -> u32 {
        let micros = self.elapsed_us();
        recorder.record(micros);
        micros
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_recorder_has_no_percentiles() {
        let recorder = LatencyRecorder::new(8);
        assert!(recorder.is_empty());
        assert_eq!(recorder.percentiles(), None);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let values: Vec<u32> = (1..=100).collect();
        assert_eq!(percentile(&values, 50), 50);
        assert_eq!(percentile(&values, 95), 95);
        assert_eq!(percentile(&values, 99), 99);
        assert_eq!(percentile(&values, 100), 100);
        assert_eq!(percentile(&values, 0), 1);
        assert_eq!(percentile(&[7], 99), 7);
    }

    #[test]
    fn summary_reports_every_field() {
        let mut recorder = LatencyRecorder::new(16);
        for value in [400, 100, 300, 200, 500] {
            recorder.record(value);
        }
        let summary = recorder.percentiles().unwrap();
        assert_eq!(summary.count, 5);
        assert_eq!(summary.min, 100);
        assert_eq!(summary.max, 500);
        assert_eq!(summary.p50, 300);
        assert_eq!(summary.p90, 500);
        assert_eq!(summary.p95, 500);
        assert_eq!(summary.p99, 500);
        assert_eq!(summary.mean, 300);
        assert!(summary.within_key_budget());
    }

    #[test]
    fn recorder_is_bounded() {
        let mut recorder = LatencyRecorder::new(2);
        recorder.record(1);
        recorder.record(2);
        recorder.record(3);
        assert_eq!(recorder.samples(), &[1, 2]);
        assert_eq!(recorder.overflowed(), 1);
        recorder.clear();
        assert!(recorder.is_empty());
        assert_eq!(recorder.overflowed(), 0);
    }

    #[test]
    fn measure_records_a_sample() {
        let mut recorder = LatencyRecorder::new(4);
        let value = recorder.measure(|| 42);
        assert_eq!(value, 42);
        assert_eq!(recorder.len(), 1);
        assert!(recorder.percentiles().unwrap().max < 1_000_000);
    }

    #[test]
    fn budget_gate_rejects_slow_runs() {
        let mut recorder = LatencyRecorder::new(4);
        for value in [1_900, 2_000, 2_500, 3_000] {
            recorder.record(value);
        }
        assert!(!recorder.percentiles().unwrap().within_key_budget());
    }
}
