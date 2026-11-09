//! Per-device MIDI latency and jitter reporting.
//!
//! midir delivers a driver timestamp in microseconds with every message. The
//! capture path pairs that timestamp with its own monotonic timestamp at the
//! moment the message is routed, which is the callback-to-queue leg. Jitter is
//! the absolute difference between consecutive leg measurements for the same
//! device.
//!
//! The tracker is diagnostics only: safe to update from the MIDI callback
//! (fixed-capacity samples, no logging; a new device name allocates once) and
//! read from the UI between frames.

use std::collections::BTreeMap;

use crate::trigger::{LatencyPercentiles, LatencyRecorder};

/// One device's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiLatencyReport {
    /// Port name.
    pub device: String,
    /// Callback-to-queue latency.
    pub latency: LatencyPercentiles,
    /// Absolute difference between consecutive samples, when at least two
    /// messages were seen.
    pub jitter: Option<LatencyPercentiles>,
}

#[derive(Debug)]
struct DeviceSamples {
    latency: LatencyRecorder,
    jitter: LatencyRecorder,
    last_latency: Option<u32>,
}

/// Latency tracker keyed by device name.
#[derive(Debug)]
pub struct MidiLatencyTracker {
    devices: BTreeMap<String, DeviceSamples>,
    capacity: usize,
}

impl MidiLatencyTracker {
    /// Track up to `capacity` samples per device.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            devices: BTreeMap::new(),
            capacity,
        }
    }

    /// Default capacity of 2,000 messages per device.
    #[must_use]
    pub fn default_capacity() -> Self {
        Self::new(2_000)
    }

    /// Record one message.
    ///
    /// `driver_timestamp_us` is midir's timestamp; `now_us` is the capture
    /// path's monotonic timestamp. A driver timestamp in the future (clock
    /// domain mismatch) clamps to zero instead of underflowing.
    pub fn record(&mut self, device: &str, driver_timestamp_us: u64, now_us: u64) {
        let latency = u32::try_from(now_us.saturating_sub(driver_timestamp_us)).unwrap_or(u32::MAX);
        let samples = self
            .devices
            .entry(device.to_owned())
            .or_insert_with(|| DeviceSamples {
                latency: LatencyRecorder::new(self.capacity),
                jitter: LatencyRecorder::new(self.capacity),
                last_latency: None,
            });
        samples.latency.record(latency);
        if let Some(previous) = samples.last_latency {
            samples.jitter.record(latency.abs_diff(previous));
        }
        samples.last_latency = Some(latency);
    }

    /// Reports for every device with samples, sorted by name.
    #[must_use]
    pub fn reports(&self) -> Vec<MidiLatencyReport> {
        self.devices
            .iter()
            .filter_map(|(device, samples)| {
                Some(MidiLatencyReport {
                    device: device.clone(),
                    latency: samples.latency.percentiles()?,
                    jitter: samples.jitter.percentiles(),
                })
            })
            .collect()
    }

    /// Report for one device.
    #[must_use]
    pub fn report(&self, device: &str) -> Option<MidiLatencyReport> {
        let samples = self.devices.get(device)?;
        Some(MidiLatencyReport {
            device: device.to_owned(),
            latency: samples.latency.percentiles()?,
            jitter: samples.jitter.percentiles(),
        })
    }

    /// Drop all samples.
    pub fn clear(&mut self) {
        self.devices.clear();
    }

    /// Device names with samples.
    #[must_use]
    pub fn devices(&self) -> Vec<&str> {
        self.devices.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_latency_and_jitter_per_device() {
        let mut tracker = MidiLatencyTracker::new(64);
        tracker.record("Nitro Max", 1_000, 1_200);
        tracker.record("Nitro Max", 2_000, 2_300);
        tracker.record("Nitro Max", 3_000, 3_250);
        let report = tracker.report("Nitro Max").unwrap();
        assert_eq!(report.latency.count, 3);
        assert_eq!(report.latency.min, 200);
        assert_eq!(report.latency.max, 300);
        assert_eq!(report.latency.p50, 250);
        let jitter = report.jitter.unwrap();
        assert_eq!(jitter.count, 2);
        assert_eq!(jitter.max, 100);
    }

    #[test]
    fn future_driver_timestamps_clamp_to_zero() {
        let mut tracker = MidiLatencyTracker::new(8);
        tracker.record("Loopback", 10_000, 9_000);
        assert_eq!(tracker.report("Loopback").unwrap().latency.max, 0);
    }

    #[test]
    fn devices_are_independent() {
        let mut tracker = MidiLatencyTracker::new(8);
        tracker.record("A", 0, 100);
        tracker.record("B", 0, 200);
        assert_eq!(tracker.report("A").unwrap().latency.max, 100);
        assert_eq!(tracker.report("B").unwrap().latency.max, 200);
        assert_eq!(tracker.devices(), vec!["A", "B"]);
        assert!(tracker.reports().len() == 2);
    }

    #[test]
    fn clearing_removes_reports() {
        let mut tracker = MidiLatencyTracker::new(8);
        tracker.record("A", 0, 100);
        tracker.clear();
        assert!(tracker.reports().is_empty());
        assert!(tracker.report("A").is_none());
    }

    #[test]
    fn default_capacity_tracker_starts_empty_and_reports_samples() {
        let mut tracker = MidiLatencyTracker::default_capacity();
        assert!(tracker.reports().is_empty());
        tracker.record("Nitro Max", 0, 250);
        let report = tracker.report("Nitro Max").unwrap();
        assert_eq!(report.latency.count, 1);
        assert_eq!(report.latency.max, 250);
        assert_eq!(report.jitter, None);
    }
}
