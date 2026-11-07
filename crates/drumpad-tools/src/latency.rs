//! Latency budget, measurement statistics, and baseline reporting (E11-T01).
//!
//! This module is deliberately dependency-free apart from `serde`: it is the
//! contract between the loopback measurement protocol in
//! `docs/qa/latency-protocol.md` and the report generator that writes
//! `docs/qa/latency-baseline.md`.
//!
//! All statistical functions use *nearest-rank* percentiles over the sorted
//! sample set. That is the definition written into the protocol, so the same
//! numbers can be reproduced by hand from a raw capture.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Sample rate used for every budget number unless a run states otherwise.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Audio block size used for every budget number unless a run states otherwise.
pub const DEFAULT_BLOCK_FRAMES: u32 = 128;

/// Key or keypad event to engine queue budget, in milliseconds.
pub const KEY_TO_QUEUE_BUDGET_MS: f64 = 2.0;

/// MIDI input to audio callback budget, expressed in audio blocks.
pub const MIDI_TO_CALLBACK_BUDGET_BLOCKS: f64 = 1.0;

/// End-to-end instrument-to-analog-output budget, in milliseconds.
pub const END_TO_END_BUDGET_MS: f64 = 10.0;

/// Hard ceiling for a single key/queue hit, in milliseconds. A single worse
/// sample fails the run even when p99 passes.
pub const KEY_TO_QUEUE_CEILING_MS: f64 = 4.0;

/// Hard ceiling for a single end-to-end hit, in milliseconds.
pub const END_TO_END_CEILING_MS: f64 = 15.0;

/// Minimum number of retained hits for the software-timestamp metrics.
pub const SOFTWARE_MIN_HITS: usize = 1_000;

/// Number of leading hits discarded as warm-up for the software metrics.
pub const SOFTWARE_WARMUP_HITS: usize = 100;

/// Minimum number of retained hits for the loopback end-to-end metric.
pub const END_TO_END_MIN_HITS: usize = 200;

/// Number of leading hits discarded as warm-up for the loopback metric.
pub const END_TO_END_WARMUP_HITS: usize = 20;

/// Maximum allowed absolute deviation of a measured audio block duration from
/// the nominal duration, as a fraction (10 %).
pub const AUDIO_BLOCK_TOLERANCE_FRACTION: f64 = 0.10;

/// The four metrics the latency protocol reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    /// Keyboard/keypad event until the trigger is visible on the engine queue.
    KeyToQueue,
    /// MIDI event until the audio callback renders the first sample of the hit.
    MidiToCallback,
    /// Physical input until the analog marker crosses the loopback capture.
    EndToEnd,
    /// Measured callback block duration against its nominal duration.
    AudioBlock,
}

impl Metric {
    /// Every metric, in report order.
    pub const ALL: [Self; 4] = [
        Self::KeyToQueue,
        Self::MidiToCallback,
        Self::EndToEnd,
        Self::AudioBlock,
    ];

    /// Stable human-readable name used in reports.
    pub const fn label(self) -> &'static str {
        match self {
            Self::KeyToQueue => "key_to_queue",
            Self::MidiToCallback => "midi_to_callback",
            Self::EndToEnd => "end_to_end",
            Self::AudioBlock => "audio_block",
        }
    }
}

impl fmt::Display for Metric {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Percentile used to decide whether a metric passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Percentile {
    /// Median.
    P50,
    /// 95th percentile.
    P95,
    /// 99th percentile.
    P99,
}

impl Percentile {
    /// Numeric percentile in `0..=100`.
    pub const fn value(self) -> f64 {
        match self {
            Self::P50 => 50.0,
            Self::P95 => 95.0,
            Self::P99 => 99.0,
        }
    }

    /// Stable short label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::P99 => "p99",
        }
    }
}

impl fmt::Display for Percentile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A latency budget entry as written in the protocol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyTarget {
    /// Metric this budget covers.
    pub metric: Metric,
    /// Percentile that must meet the budget.
    pub percentile: Percentile,
    /// Budget in milliseconds. `None` for metrics measured in blocks.
    pub budget_ms: Option<f64>,
    /// Budget in audio blocks. `None` for metrics measured in milliseconds.
    pub budget_blocks: Option<f64>,
    /// Hits that must remain after warm-up removal.
    pub min_hits: usize,
    /// Leading hits discarded before statistics.
    pub warmup_hits: usize,
    /// Optional per-hit hard ceiling in milliseconds.
    pub ceiling_ms: Option<f64>,
}

impl LatencyTarget {
    /// The budget in milliseconds for a concrete stream format.
    ///
    /// `budget_blocks` is converted with [`block_duration_ms`]; a zero sample
    /// rate or frame count falls back to the default 48 kHz / 128 frame
    /// format so the table can always be rendered.
    pub fn budget_ms_for(&self, sample_rate: u32, block_frames: u32) -> f64 {
        match (self.budget_ms, self.budget_blocks) {
            (Some(ms), _) => ms,
            (None, Some(blocks)) => blocks * block_duration_ms(block_frames, sample_rate),
            (None, None) => 0.0,
        }
    }
}

/// The protocol budget table.
pub const TARGETS: [LatencyTarget; 4] = [
    LatencyTarget {
        metric: Metric::KeyToQueue,
        percentile: Percentile::P99,
        budget_ms: Some(KEY_TO_QUEUE_BUDGET_MS),
        budget_blocks: None,
        min_hits: SOFTWARE_MIN_HITS,
        warmup_hits: SOFTWARE_WARMUP_HITS,
        ceiling_ms: Some(KEY_TO_QUEUE_CEILING_MS),
    },
    LatencyTarget {
        metric: Metric::MidiToCallback,
        percentile: Percentile::P99,
        budget_ms: None,
        budget_blocks: Some(MIDI_TO_CALLBACK_BUDGET_BLOCKS),
        min_hits: SOFTWARE_MIN_HITS,
        warmup_hits: SOFTWARE_WARMUP_HITS,
        ceiling_ms: None,
    },
    LatencyTarget {
        metric: Metric::EndToEnd,
        percentile: Percentile::P99,
        budget_ms: Some(END_TO_END_BUDGET_MS),
        budget_blocks: None,
        min_hits: END_TO_END_MIN_HITS,
        warmup_hits: END_TO_END_WARMUP_HITS,
        ceiling_ms: Some(END_TO_END_CEILING_MS),
    },
    LatencyTarget {
        metric: Metric::AudioBlock,
        percentile: Percentile::P50,
        budget_ms: None,
        budget_blocks: None,
        min_hits: 100,
        warmup_hits: 10,
        ceiling_ms: None,
    },
];

/// Look up the protocol target for a metric.
pub fn target_for(metric: Metric) -> LatencyTarget {
    TARGETS
        .into_iter()
        .find(|target| target.metric == metric)
        .expect("every metric has a protocol target")
}

/// Duration of one audio block in milliseconds.
pub fn block_duration_ms(frames: u32, sample_rate: u32) -> f64 {
    let frames = if frames == 0 {
        DEFAULT_BLOCK_FRAMES
    } else {
        frames
    };
    let rate = if sample_rate == 0 {
        DEFAULT_SAMPLE_RATE
    } else {
        sample_rate
    };
    frames as f64 * 1_000.0 / rate as f64
}

/// Nearest-rank percentile over an already sorted slice. `percentile` is in
/// `0..=100`. An empty slice yields `None`.
pub fn percentile_sorted(sorted: &[f64], percentile: Percentile) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percentile.value() / 100.0 * sorted.len() as f64).ceil();
    let index = (rank as usize).saturating_sub(1).min(sorted.len() - 1);
    Some(sorted[index])
}

/// Summary statistics for one metric.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatencyStats {
    /// Retained hit count after warm-up removal.
    pub count: usize,
    /// Smallest sample.
    pub min_ms: f64,
    /// Median.
    pub p50_ms: f64,
    /// 95th percentile.
    pub p95_ms: f64,
    /// 99th percentile.
    pub p99_ms: f64,
    /// Largest sample.
    pub max_ms: f64,
    /// Arithmetic mean.
    pub mean_ms: f64,
    /// Population standard deviation.
    pub std_dev_ms: f64,
}

/// Errors from latency statistics and verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatencyError {
    /// No samples were supplied after warm-up removal.
    NoSamples,
    /// A sample was NaN or infinite.
    NonFinite {
        /// Index of the offending sample.
        index: usize,
        /// The offending value rendered for the message.
        value: String,
    },
    /// A sample was negative; latency cannot be negative.
    Negative {
        /// Index of the offending sample.
        index: usize,
        /// The offending value rendered for the message.
        value: String,
    },
    /// Fewer retained hits than the protocol requires.
    InsufficientHits {
        /// Retained hit count.
        got: usize,
        /// Required hit count.
        required: usize,
    },
}

impl fmt::Display for LatencyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSamples => write!(formatter, "no latency samples after warm-up removal"),
            Self::NonFinite { index, value } => {
                write!(formatter, "latency sample {index} is not finite: {value}")
            }
            Self::Negative { index, value } => {
                write!(formatter, "latency sample {index} is negative: {value}")
            }
            Self::InsufficientHits { got, required } => write!(
                formatter,
                "only {got} retained hits; the protocol requires at least {required}"
            ),
        }
    }
}

impl Error for LatencyError {}

fn validate(samples: &[f64]) -> Result<(), LatencyError> {
    if samples.is_empty() {
        return Err(LatencyError::NoSamples);
    }
    for (index, sample) in samples.iter().enumerate() {
        if !sample.is_finite() {
            return Err(LatencyError::NonFinite {
                index,
                value: format!("{sample}"),
            });
        }
        if *sample < 0.0 {
            return Err(LatencyError::Negative {
                index,
                value: format!("{sample}"),
            });
        }
    }
    Ok(())
}

/// Summarize a set of latency samples in milliseconds.
///
/// The input order does not matter. The slice is copied and sorted with
/// `f64::total_cmp`, which is a total order over the validated finite values.
pub fn summarize(samples: &[f64]) -> Result<LatencyStats, LatencyError> {
    validate(samples)?;
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let count = sorted.len();
    let sum: f64 = sorted.iter().sum();
    let mean = sum / count as f64;
    let variance = sorted
        .iter()
        .map(|sample| {
            let delta = sample - mean;
            delta * delta
        })
        .sum::<f64>()
        / count as f64;
    Ok(LatencyStats {
        count,
        min_ms: sorted[0],
        p50_ms: percentile_sorted(&sorted, Percentile::P50).unwrap_or(0.0),
        p95_ms: percentile_sorted(&sorted, Percentile::P95).unwrap_or(0.0),
        p99_ms: percentile_sorted(&sorted, Percentile::P99).unwrap_or(0.0),
        max_ms: sorted[count - 1],
        mean_ms: mean,
        std_dev_ms: variance.sqrt(),
    })
}

/// A raw measurement set for one metric.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// Metric measured.
    pub metric: Metric,
    /// Leading hits discarded as warm-up before statistics.
    pub warmup_samples: usize,
    /// Samples in milliseconds, in capture order.
    pub samples_ms: Vec<f64>,
}

impl Measurement {
    /// Build a measurement, rejecting an empty sample list.
    pub fn new(
        metric: Metric,
        warmup_samples: usize,
        samples_ms: Vec<f64>,
    ) -> Result<Self, LatencyError> {
        if samples_ms.is_empty() {
            return Err(LatencyError::NoSamples);
        }
        Ok(Self {
            metric,
            warmup_samples,
            samples_ms,
        })
    }

    /// Samples that remain after warm-up removal.
    pub fn retained(&self) -> &[f64] {
        let start = self.warmup_samples.min(self.samples_ms.len());
        &self.samples_ms[start..]
    }

    /// Statistics over the retained samples.
    pub fn stats(&self) -> Result<LatencyStats, LatencyError> {
        summarize(self.retained())
    }
}

/// Verdict for one metric against its protocol target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetVerdict {
    /// Metric measured.
    pub metric: Metric,
    /// Percentile that decided the verdict.
    pub percentile: Percentile,
    /// Budget in milliseconds for the concrete stream format.
    pub budget_ms: f64,
    /// Retained hit count.
    pub hits: usize,
    /// Summary statistics.
    pub stats: LatencyStats,
    /// True when every pass condition holds.
    pub pass: bool,
    /// Human-readable explanation naming the failing condition.
    pub reason: String,
}

/// Evaluate a measurement against the protocol target for its metric.
///
/// Pass conditions, in order:
/// 1. at least `min_hits` retained samples,
/// 2. the chosen percentile is at or below the budget,
/// 3. when a ceiling is configured, no single hit exceeds it.
///
/// `AudioBlock` is special: the measured p50 must stay within
/// [`AUDIO_BLOCK_TOLERANCE_FRACTION`] of the nominal block duration. This is a
/// sanity check on the stream configuration, not a latency budget.
pub fn evaluate(
    measurement: &Measurement,
    sample_rate: u32,
    block_frames: u32,
) -> Result<TargetVerdict, LatencyError> {
    let target = target_for(measurement.metric);
    let stats = measurement.stats()?;
    let budget_ms = if measurement.metric == Metric::AudioBlock {
        block_duration_ms(block_frames, sample_rate)
    } else {
        target.budget_ms_for(sample_rate, block_frames)
    };

    let mut pass = true;
    let mut failures: Vec<String> = Vec::new();

    if stats.count < target.min_hits {
        pass = false;
        failures.push(format!(
            "{} retained hits < required {}",
            stats.count, target.min_hits
        ));
    }

    if measurement.metric == Metric::AudioBlock {
        let deviation = (stats.p50_ms - budget_ms).abs() / budget_ms.max(f64::MIN_POSITIVE);
        if deviation > AUDIO_BLOCK_TOLERANCE_FRACTION {
            pass = false;
            failures.push(format!(
                "block duration p50 {:.3} ms differs from nominal {:.3} ms by {:.1} % (limit {:.0} %)",
                stats.p50_ms,
                budget_ms,
                deviation * 100.0,
                AUDIO_BLOCK_TOLERANCE_FRACTION * 100.0
            ));
        }
    } else {
        let measured = match target.percentile {
            Percentile::P50 => stats.p50_ms,
            Percentile::P95 => stats.p95_ms,
            Percentile::P99 => stats.p99_ms,
        };
        if measured > budget_ms {
            pass = false;
            failures.push(format!(
                "{} {:.3} ms > budget {:.3} ms",
                target.percentile, measured, budget_ms
            ));
        }
    }

    if let Some(ceiling) = target.ceiling_ms
        && stats.max_ms > ceiling
    {
        pass = false;
        failures.push(format!(
            "max {:.3} ms > hard ceiling {:.3} ms",
            stats.max_ms, ceiling
        ));
    }

    let reason = if pass {
        "pass".to_owned()
    } else {
        failures.join("; ")
    };

    Ok(TargetVerdict {
        metric: measurement.metric,
        percentile: target.percentile,
        budget_ms,
        hits: stats.count,
        stats,
        pass,
        reason,
    })
}

/// WASAPI mode used by a measurement run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    /// WASAPI shared mode; the v1 default.
    #[default]
    Shared,
    /// WASAPI exclusive mode; only if the spike justifies it.
    Exclusive,
}

impl fmt::Display for AudioMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared => formatter.write_str("shared"),
            Self::Exclusive => formatter.write_str("exclusive"),
        }
    }
}

/// Whether a baseline row was actually measured or is still pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineStatus {
    /// All metrics were measured on the named hardware.
    Measured,
    /// The row documents the required hardware but no run has happened yet.
    ///
    /// This is the default so a future `#[serde(default)]` can never turn an
    /// unmeasured row into evidence.
    #[default]
    Pending,
}

/// Everything needed to reproduce and interpret one hardware configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareProfile {
    /// Machine name or model.
    pub machine: String,
    /// CPU model.
    pub cpu: String,
    /// Installed RAM in GiB.
    pub ram_gib: f64,
    /// Operating system name.
    pub os: String,
    /// Operating system build.
    pub os_build: String,
    /// Playback device.
    pub audio_device: String,
    /// Driver and version.
    pub audio_driver: String,
    /// WASAPI mode.
    pub audio_mode: AudioMode,
    /// Keyboard or keypad used for the key/queue metric.
    pub keyboard: String,
    /// MIDI interface and module.
    pub midi_interface: String,
    /// Analog monitoring path.
    pub monitoring_path: String,
    /// Person or script that ran the measurement.
    pub operator: String,
    /// ISO-8601 date of the run.
    pub date_utc: String,
}

impl HardwareProfile {
    /// The reference hardware named in the design document. No run has been
    /// recorded against it yet, so baseline rows using it are `pending`.
    pub fn reference_pending() -> Self {
        Self {
            machine: "Reference desktop (TBD)".to_owned(),
            cpu: "TBD".to_owned(),
            ram_gib: 0.0,
            os: "Windows 11 x64".to_owned(),
            os_build: "TBD".to_owned(),
            audio_device: "PC onboard 3.5 mm output".to_owned(),
            audio_driver: "Realtek onboard driver (TBD)".to_owned(),
            audio_mode: AudioMode::Shared,
            keyboard: "Finding 35-key single-hand USB keyboard (ASIN B09HF5WDR2)".to_owned(),
            midi_interface: "Alesis Nitro Max over USB-MIDI (Local Mode OFF)".to_owned(),
            monitoring_path: "PC onboard 3.5 mm -> Nitro Max Aux In -> phones".to_owned(),
            operator: "pending".to_owned(),
            date_utc: "pending".to_owned(),
        }
    }
}

/// One row of the baseline table: a hardware/format configuration and its
/// verdicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineRow {
    /// Measurement status.
    pub status: BaselineStatus,
    /// Hardware and driver details.
    pub hardware: HardwareProfile,
    /// Stream sample rate in Hz.
    pub sample_rate: u32,
    /// Stream block size in frames.
    pub block_frames: u32,
    /// Verdicts for the metrics that were measured. Empty while pending.
    pub verdicts: Vec<TargetVerdict>,
    /// Path to the archived raw capture, relative to the repository root.
    pub raw_capture: String,
    /// Free-form notes; must explain any deviation from the protocol.
    pub notes: String,
}

impl BaselineRow {
    /// A pending row for the reference hardware at the default format.
    pub fn reference_pending_row() -> Self {
        Self {
            status: BaselineStatus::Pending,
            hardware: HardwareProfile::reference_pending(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            block_frames: DEFAULT_BLOCK_FRAMES,
            verdicts: Vec::new(),
            raw_capture: "pending".to_owned(),
            notes: "No physical loopback run recorded yet; see docs/qa/latency-protocol.md."
                .to_owned(),
        }
    }

    /// True when every verdict passes and the row was measured.
    pub fn passes(&self) -> bool {
        self.status == BaselineStatus::Measured
            && !self.verdicts.is_empty()
            && self.verdicts.iter().all(|verdict| verdict.pass)
    }
}

/// Top-level baseline document stored in the repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineReport {
    /// Schema version of this document.
    pub schema_version: u32,
    /// Generator name and version.
    pub generated_by: String,
    /// Protocol document this report follows.
    pub protocol: String,
    /// Rows, in a stable order defined by the generator.
    pub rows: Vec<BaselineRow>,
    /// Free-form notes about the report as a whole.
    pub notes: String,
}

/// Current baseline document schema version.
pub const BASELINE_SCHEMA_VERSION: u32 = 1;

impl BaselineReport {
    /// An empty report for the current schema.
    pub fn new() -> Self {
        Self {
            schema_version: BASELINE_SCHEMA_VERSION,
            generated_by: concat!("drumpad-tools ", env!("CARGO_PKG_VERSION")).to_owned(),
            protocol: "docs/qa/latency-protocol.md".to_owned(),
            rows: Vec::new(),
            notes: String::new(),
        }
    }

    /// True when every row was measured and passed.
    pub fn all_pass(&self) -> bool {
        !self.rows.is_empty() && self.rows.iter().all(BaselineRow::passes)
    }

    /// Render the baseline table as Markdown.
    ///
    /// Every cell is escaped so a pipe in a machine name cannot break the
    /// table. Pending rows show `pending` in each metric column and must not
    /// be counted as evidence by a reviewer.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Latency baseline results\n\n");
        out.push_str(
            "Generated from the JSON baseline by `latency_report`. Do not edit by hand.\n\
             Protocol: `docs/qa/latency-protocol.md`. A row marked `pending` is a required \
             hardware configuration that has **not** been measured and is not evidence of \
             compliance.\n\n",
        );
        out.push_str(&format!(
            "Schema: v{} - generated by: {} - rows: {}\n\n",
            self.schema_version,
            escape_markdown(&self.generated_by),
            self.rows.len()
        ));
        out.push_str(
            "| Status | Hardware | OS build | Driver | Device | Mode | Rate / buffer | Metric | p50 ms | p95 ms | p99 ms | max ms | Hits | Verdict | Date | Operator | Raw capture |\n",
        );
        out.push_str(
            "| --- | --- | --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- | --- | --- |\n",
        );
        for row in &self.rows {
            let hardware = &row.hardware;
            let status = match row.status {
                BaselineStatus::Measured => "measured",
                BaselineStatus::Pending => "pending",
            };
            let format = format!("{}/{}", row.sample_rate, row.block_frames);
            if row.verdicts.is_empty() {
                out.push_str(&format!(
                    "| {status} | {} | {} | {} | {} | {} | {} | - | - | - | - | - | - | pending | {} | {} | {} |\n",
                    escape_markdown(&hardware.machine),
                    escape_markdown(&hardware.os_build),
                    escape_markdown(&hardware.audio_driver),
                    escape_markdown(&hardware.audio_device),
                    hardware.audio_mode,
                    format,
                    escape_markdown(&hardware.date_utc),
                    escape_markdown(&hardware.operator),
                    escape_markdown(&row.raw_capture),
                ));
                continue;
            }
            for verdict in &row.verdicts {
                out.push_str(&format!(
                    "| {status} | {} | {} | {} | {} | {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {} | {} | {} | {} |\n",
                    escape_markdown(&hardware.machine),
                    escape_markdown(&hardware.os_build),
                    escape_markdown(&hardware.audio_driver),
                    escape_markdown(&hardware.audio_device),
                    hardware.audio_mode,
                    format,
                    verdict.metric,
                    verdict.stats.p50_ms,
                    verdict.stats.p95_ms,
                    verdict.stats.p99_ms,
                    verdict.stats.max_ms,
                    verdict.hits,
                    if verdict.pass { "pass" } else { "FAIL" },
                    escape_markdown(&hardware.date_utc),
                    escape_markdown(&hardware.operator),
                    escape_markdown(&row.raw_capture),
                ));
            }
        }
        if !self.notes.is_empty() {
            out.push_str(&format!("\n## Notes\n\n{}\n", self.notes));
        }
        out
    }
}

impl Default for BaselineReport {
    fn default() -> Self {
        Self::new()
    }
}

fn escape_markdown(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(metric: Metric, samples: Vec<f64>) -> TargetVerdict {
        let measurement = Measurement::new(metric, 0, samples).expect("measurement");
        evaluate(&measurement, DEFAULT_SAMPLE_RATE, DEFAULT_BLOCK_FRAMES).expect("verdict")
    }

    #[test]
    fn block_duration_matches_the_design_document() {
        let ms = block_duration_ms(128, 48_000);
        assert!((ms - 2.666_666_666_7).abs() < 1e-9, "{ms}");
        assert_eq!(block_duration_ms(0, 0), block_duration_ms(128, 48_000));
    }

    #[test]
    fn nearest_rank_percentiles_are_exact() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile_sorted(&samples, Percentile::P50), Some(50.0));
        assert_eq!(percentile_sorted(&samples, Percentile::P95), Some(95.0));
        assert_eq!(percentile_sorted(&samples, Percentile::P99), Some(99.0));
        assert_eq!(percentile_sorted(&[], Percentile::P99), None);
        assert_eq!(percentile_sorted(&[7.0], Percentile::P99), Some(7.0));
    }

    #[test]
    fn summarize_reports_mean_and_population_deviation() {
        let stats = summarize(&[1.0, 2.0, 3.0, 4.0]).expect("stats");
        assert_eq!(stats.count, 4);
        assert_eq!(stats.min_ms, 1.0);
        assert_eq!(stats.max_ms, 4.0);
        assert_eq!(stats.p50_ms, 2.0);
        assert!((stats.mean_ms - 2.5).abs() < f64::EPSILON);
        assert!((stats.std_dev_ms - 1.118_033_988_749_895).abs() < 1e-12);
    }

    #[test]
    fn summarize_rejects_empty_non_finite_and_negative() {
        assert_eq!(summarize(&[]), Err(LatencyError::NoSamples));
        assert!(matches!(
            summarize(&[1.0, f64::NAN]),
            Err(LatencyError::NonFinite { .. })
        ));
        assert!(matches!(
            summarize(&[1.0, f64::INFINITY]),
            Err(LatencyError::NonFinite { .. })
        ));
        assert!(matches!(
            summarize(&[-0.5]),
            Err(LatencyError::Negative { .. })
        ));
    }

    #[test]
    fn warmup_hits_are_removed_before_statistics() {
        let measurement =
            Measurement::new(Metric::KeyToQueue, 2, vec![0.1, 0.2, 1.0, 2.0, 3.0]).expect("m");
        assert_eq!(measurement.retained(), &[1.0, 2.0, 3.0]);
        assert_eq!(measurement.stats().expect("stats").count, 3);
    }

    #[test]
    fn key_to_queue_passes_at_the_budget_and_fails_above_it() {
        let pass = verdict(Metric::KeyToQueue, vec![1.0; SOFTWARE_MIN_HITS]);
        assert!(pass.pass, "{}", pass.reason);
        assert_eq!(pass.budget_ms, KEY_TO_QUEUE_BUDGET_MS);

        // Nearest-rank p99 over 1000 samples is the 990th value, so more than
        // ten samples must move before p99 does.
        let mut slow = vec![1.0; SOFTWARE_MIN_HITS];
        slow[SOFTWARE_MIN_HITS - 20..].fill(2.5);
        let fail = verdict(Metric::KeyToQueue, slow);
        assert!(!fail.pass);
        assert!(fail.reason.contains("p99"), "{}", fail.reason);
    }

    #[test]
    fn a_single_hit_over_the_hard_ceiling_fails_even_when_p99_passes() {
        let mut samples = vec![1.0; SOFTWARE_MIN_HITS];
        samples[10] = KEY_TO_QUEUE_CEILING_MS + 0.5;
        let verdict = verdict(Metric::KeyToQueue, samples);
        assert!(!verdict.pass);
        assert!(
            verdict.reason.contains("hard ceiling"),
            "{}",
            verdict.reason
        );
    }

    #[test]
    fn too_few_hits_fails_the_protocol() {
        let verdict = verdict(Metric::KeyToQueue, vec![1.0; 10]);
        assert!(!verdict.pass);
        assert!(verdict.reason.contains("required"), "{}", verdict.reason);
    }

    #[test]
    fn midi_budget_is_one_block_at_the_configured_format() {
        let target = target_for(Metric::MidiToCallback);
        let budget = target.budget_ms_for(48_000, 128);
        assert!((budget - 2.666_666_666_7).abs() < 1e-9);
        let budget_96k = target.budget_ms_for(96_000, 128);
        assert!((budget_96k - 1.333_333_333_3).abs() < 1e-9);

        let verdict = verdict(Metric::MidiToCallback, vec![1.0; SOFTWARE_MIN_HITS]);
        assert!(verdict.pass, "{}", verdict.reason);
        assert!((verdict.budget_ms - 2.666_666_666_7).abs() < 1e-9);
    }

    #[test]
    fn audio_block_sanity_check_accepts_nominal_and_rejects_wrong_format() {
        let nominal = block_duration_ms(128, 48_000);
        let pass = verdict(Metric::AudioBlock, vec![nominal; 100]);
        assert!(pass.pass, "{}", pass.reason);

        let fail = verdict(Metric::AudioBlock, vec![nominal * 2.0; 100]);
        assert!(!fail.pass);
        assert!(fail.reason.contains("nominal"), "{}", fail.reason);
    }

    #[test]
    fn baseline_markdown_has_the_review_columns_and_marks_pending() {
        let mut report = BaselineReport::new();
        report.rows.push(BaselineRow::reference_pending_row());
        let markdown = report.to_markdown();
        for column in [
            "Hardware",
            "OS build",
            "Driver",
            "Mode",
            "p50 ms",
            "p95 ms",
            "p99 ms",
            "Date",
            "Operator",
            "Raw capture",
        ] {
            assert!(markdown.contains(column), "missing column {column}");
        }
        assert!(markdown.contains("| pending |"), "{markdown}");
        assert!(!report.all_pass(), "a pending row must not count as a pass");
    }

    #[test]
    fn baseline_json_round_trips() {
        let mut report = BaselineReport::new();
        report.rows.push(BaselineRow::reference_pending_row());
        let json = serde_json::to_string_pretty(&report).expect("serialize");
        let parsed: BaselineReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(report, parsed);
    }
}
