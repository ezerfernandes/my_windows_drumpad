//! Non-destructive sample editing: parameters, a bounded undo/redo history, and
//! the render function that turns a source buffer plus parameters into a
//! playable buffer.
//!
//! The original file is never written. [`SourceRef`] is a read-only description
//! of the decoded source (path, hash, rate, channel count, length), and every
//! edit is a value inside [`EditParams`]. "Reset" is the default parameter set,
//! so resetting is indistinguishable from never having edited.
//!
//! Rendering is a pure function over an in-memory buffer. That keeps it
//! testable without I/O and keeps the samples crate free of UI types.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Lowest gain the editor accepts, in dB.
pub const MIN_GAIN_DB: f32 = -60.0;
/// Highest gain the editor accepts, in dB.
pub const MAX_GAIN_DB: f32 = 24.0;
/// Pitch range in semitones. v1 resamples, so this is also a length change.
pub const MIN_PITCH_SEMITONES: f32 = -24.0;
/// Highest pitch shift in semitones.
pub const MAX_PITCH_SEMITONES: f32 = 24.0;
/// Undo depth. Bounded so a long session cannot grow without limit.
pub const MAX_HISTORY: usize = 64;
/// Peak level normalization aims here, 0.1 dB below full scale.
pub const NORMALIZE_TARGET: f32 = 0.988_553_3;

/// Read-only description of the decoded source a pad points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Absolute or kit-relative path of the original file.
    pub path: PathBuf,
    /// SHA-256 of the original content, used by the cache and relink flow.
    pub content_hash: String,
    /// Source sample rate in hertz.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Frames per channel.
    pub frames: u64,
}

impl SourceRef {
    /// Source duration in seconds.
    pub fn duration_seconds(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames as f32 / self.sample_rate as f32
    }
}

/// Every editable parameter of one pad's sample.
///
/// All times are seconds. Trim and loop points are relative to the *source*
/// start, which keeps them stable when pitch changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditParams {
    /// Start of the audible region.
    pub trim_start: f32,
    /// End of the audible region. `None` means "to the end of the source".
    pub trim_end: Option<f32>,
    /// Fade-in length at the start of the trimmed region.
    pub fade_in: f32,
    /// Fade-out length at the end of the trimmed region.
    pub fade_out: f32,
    /// Linear gain in dB.
    pub gain_db: f32,
    /// Scale the result so its peak sits at [`NORMALIZE_TARGET`].
    pub normalize: bool,
    /// Play the trimmed region backwards.
    pub reverse: bool,
    /// Whether the engine loops the region.
    pub loop_enabled: bool,
    /// Loop start, in source seconds.
    pub loop_start: f32,
    /// Loop end, in source seconds.
    pub loop_end: f32,
    /// Equal-power crossfade length at the loop seam.
    pub loop_crossfade: f32,
    /// Playback pitch shift. Positive plays faster and shorter.
    pub pitch_semitones: f32,
}

impl Default for EditParams {
    fn default() -> Self {
        Self {
            trim_start: 0.0,
            trim_end: None,
            fade_in: 0.0,
            fade_out: 0.0,
            gain_db: 0.0,
            normalize: false,
            reverse: false,
            loop_enabled: false,
            loop_start: 0.0,
            loop_end: 0.0,
            loop_crossfade: 0.0,
            pitch_semitones: 0.0,
        }
    }
}

impl EditParams {
    /// The source region end for a given source.
    pub fn effective_trim_end(&self, source: &SourceRef) -> f32 {
        self.trim_end
            .unwrap_or_else(|| source.duration_seconds())
            .clamp(0.0, source.duration_seconds())
    }

    /// Trimmed duration in source seconds, before pitch.
    pub fn trimmed_duration(&self, source: &SourceRef) -> f32 {
        (self.effective_trim_end(source) - self.trim_start).max(0.0)
    }

    /// Audible duration after pitch resampling.
    pub fn rendered_duration(&self, source: &SourceRef) -> f32 {
        self.trimmed_duration(source) / pitch_factor(self.pitch_semitones)
    }

    /// Whether any parameter differs from the original.
    pub fn is_modified(&self) -> bool {
        *self != Self::default()
    }

    /// Check the ranges serde cannot express.
    pub fn validate(&self, source: &SourceRef) -> Result<(), EditError> {
        let duration = source.duration_seconds();
        for (field, value) in [
            ("trim start", self.trim_start),
            ("fade in", self.fade_in),
            ("fade out", self.fade_out),
            ("gain", self.gain_db),
            ("loop start", self.loop_start),
            ("loop end", self.loop_end),
            ("loop crossfade", self.loop_crossfade),
            ("pitch", self.pitch_semitones),
        ] {
            if !value.is_finite() {
                return Err(EditError::NotFinite { field });
            }
        }
        if let Some(end) = self.trim_end
            && !end.is_finite()
        {
            return Err(EditError::NotFinite { field: "trim end" });
        }

        if self.trim_start < 0.0 || self.trim_start >= duration {
            return Err(EditError::TrimRange {
                start: self.trim_start,
                end: self.effective_trim_end(source),
                duration,
            });
        }

        let trimmed = self.trimmed_duration(source);
        if trimmed <= 0.0 {
            return Err(EditError::TrimRange {
                start: self.trim_start,
                end: self.effective_trim_end(source),
                duration,
            });
        }

        if !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&self.gain_db) {
            return Err(EditError::GainOutOfRange {
                value: self.gain_db,
            });
        }
        if !(MIN_PITCH_SEMITONES..=MAX_PITCH_SEMITONES).contains(&self.pitch_semitones) {
            return Err(EditError::PitchOutOfRange {
                value: self.pitch_semitones,
            });
        }
        if self.fade_in < 0.0 || self.fade_out < 0.0 {
            return Err(EditError::NegativeFade);
        }
        if self.fade_in + self.fade_out > trimmed + f32::EPSILON {
            return Err(EditError::FadeTooLong {
                fade: self.fade_in + self.fade_out,
                available: trimmed,
            });
        }

        if self.loop_enabled {
            let start = self.loop_start.max(0.0);
            let end = self.loop_end.min(duration);
            if !(start < end) {
                return Err(EditError::LoopRange {
                    start: self.loop_start,
                    end: self.loop_end,
                });
            }
            if start < self.trim_start || end > self.effective_trim_end(source) + f32::EPSILON {
                return Err(EditError::LoopOutsideTrim {
                    start: self.loop_start,
                    end: self.loop_end,
                    trim_start: self.trim_start,
                    trim_end: self.effective_trim_end(source),
                });
            }
            if self.loop_crossfade < 0.0 || self.loop_crossfade * 2.0 > end - start {
                return Err(EditError::LoopCrossfadeTooLong {
                    crossfade: self.loop_crossfade,
                    available: end - start,
                });
            }
        }

        Ok(())
    }

    /// Timeline of fade-in and fade-out windows in rendered seconds, used by the
    /// waveform overlay. Returns `(fade_in, fade_out)` lengths.
    pub fn fade_lengths(&self, source: &SourceRef) -> (f32, f32) {
        let trimmed = self.trimmed_duration(source);
        (self.fade_in.min(trimmed), self.fade_out.min(trimmed))
    }

    /// Loop points in rendered seconds, when looping is enabled.
    pub fn loop_window(&self) -> Option<(f32, f32)> {
        if !self.loop_enabled {
            return None;
        }
        let factor = pitch_factor(self.pitch_semitones);
        Some((
            (self.loop_start - self.trim_start).max(0.0) / factor,
            (self.loop_end - self.trim_start).max(0.0) / factor,
        ))
    }
}

/// Pitch ratio for a semitone offset.
pub fn pitch_factor(semitones: f32) -> f32 {
    if semitones == 0.0 {
        1.0
    } else {
        (semitones / 12.0).exp2()
    }
}

/// Editing errors with user-facing text. Every variant names the value and the
/// range so the inspector can show a concrete reason.
#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    /// Trim range is empty or outside the source.
    TrimRange {
        /// Requested start.
        start: f32,
        /// Requested end.
        end: f32,
        /// Source duration.
        duration: f32,
    },
    /// A parameter is NaN or infinite.
    NotFinite {
        /// Parameter name.
        field: &'static str,
    },
    /// Gain is outside [`MIN_GAIN_DB`]..=[`MAX_GAIN_DB`].
    GainOutOfRange {
        /// Requested gain.
        value: f32,
    },
    /// Pitch is outside [`MIN_PITCH_SEMITONES`]..=[`MAX_PITCH_SEMITONES`].
    PitchOutOfRange {
        /// Requested semitones.
        value: f32,
    },
    /// A fade length is negative.
    NegativeFade,
    /// Fades together are longer than the trimmed region.
    FadeTooLong {
        /// Combined fade length.
        fade: f32,
        /// Available trimmed length.
        available: f32,
    },
    /// Loop start is not before loop end.
    LoopRange {
        /// Loop start.
        start: f32,
        /// Loop end.
        end: f32,
    },
    /// Loop points leave the trimmed region.
    LoopOutsideTrim {
        /// Loop start.
        start: f32,
        /// Loop end.
        end: f32,
        /// Trim start.
        trim_start: f32,
        /// Trim end.
        trim_end: f32,
    },
    /// The crossfade is longer than half the loop.
    LoopCrossfadeTooLong {
        /// Requested crossfade.
        crossfade: f32,
        /// Loop length.
        available: f32,
    },
    /// The source is empty or malformed.
    EmptySource,
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrimRange {
                start,
                end,
                duration,
            } => write!(
                formatter,
                "trim range {start:.3}s..{end:.3}s is outside the {duration:.3}s source"
            ),
            Self::NotFinite { field } => write!(formatter, "{field} must be a finite number"),
            Self::GainOutOfRange { value } => write!(
                formatter,
                "gain {value:.1} dB is outside {MIN_GAIN_DB:.0}..={MAX_GAIN_DB:.0} dB"
            ),
            Self::PitchOutOfRange { value } => write!(
                formatter,
                "pitch {value:.1} semitones is outside {MIN_PITCH_SEMITONES:.0}..={MAX_PITCH_SEMITONES:.0}"
            ),
            Self::NegativeFade => formatter.write_str("fade lengths cannot be negative"),
            Self::FadeTooLong { fade, available } => write!(
                formatter,
                "fades total {fade:.3}s but only {available:.3}s is trimmed"
            ),
            Self::LoopRange { start, end } => {
                write!(
                    formatter,
                    "loop start {start:.3}s must be before end {end:.3}s"
                )
            }
            Self::LoopOutsideTrim {
                start,
                end,
                trim_start,
                trim_end,
            } => write!(
                formatter,
                "loop {start:.3}s..{end:.3}s is outside the trimmed region {trim_start:.3}s..{trim_end:.3}s"
            ),
            Self::LoopCrossfadeTooLong {
                crossfade,
                available,
            } => write!(
                formatter,
                "crossfade {crossfade:.3}s is longer than half the {available:.3}s loop"
            ),
            Self::EmptySource => formatter.write_str("the sample has no audio frames"),
        }
    }
}

impl std::error::Error for EditError {}

/// Render one pad's buffer from the immutable source and the edit parameters.
///
/// The returned buffer is a fresh allocation; `source` is only read. This is the
/// single place that knows how parameters become samples, so the editor preview,
/// the offline renderer, and the engine bank agree by construction.
pub fn render(
    source: &[f32],
    channels: u16,
    sample_rate: u32,
    params: &EditParams,
) -> Result<Vec<f32>, EditError> {
    let channels = channels.max(1) as usize;
    if source.is_empty() || sample_rate == 0 || source.len() < channels {
        return Err(EditError::EmptySource);
    }
    let frames = source.len() / channels;
    let rate = sample_rate as f32;

    let start_frame = (params.trim_start.max(0.0) * rate).floor() as usize;
    let end_frame = params
        .trim_end
        .map(|end| (end.max(0.0) * rate).ceil() as usize)
        .unwrap_or(frames)
        .min(frames);
    let start_frame = start_frame.min(end_frame);
    if start_frame == end_frame {
        return Err(EditError::EmptySource);
    }

    let mut rendered = source[start_frame * channels..end_frame * channels].to_vec();
    let rendered_frames = rendered.len() / channels;

    if params.reverse {
        let mut reversed = vec![0.0; rendered.len()];
        for frame in 0..rendered_frames {
            let target = rendered_frames - 1 - frame;
            for channel in 0..channels {
                reversed[target * channels + channel] = rendered[frame * channels + channel];
            }
        }
        rendered = reversed;
    }

    apply_fade(&mut rendered, channels, rate, params.fade_in, FadeKind::In);
    apply_fade(
        &mut rendered,
        channels,
        rate,
        params.fade_out,
        FadeKind::Out,
    );

    let mut gain = 10.0_f32.powf(params.gain_db / 20.0);
    if params.normalize {
        let peak = rendered
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if peak > 0.0 {
            gain *= NORMALIZE_TARGET / peak;
        }
    }
    if (gain - 1.0).abs() > f32::EPSILON {
        for sample in &mut rendered {
            *sample *= gain;
        }
    }

    let factor = pitch_factor(params.pitch_semitones);
    if (factor - 1.0).abs() > f32::EPSILON {
        rendered = resample_linear(&rendered, channels, factor);
    }

    Ok(rendered)
}

enum FadeKind {
    In,
    Out,
}

fn apply_fade(buffer: &mut [f32], channels: usize, rate: f32, seconds: f32, kind: FadeKind) {
    let frames = buffer.len() / channels;
    let fade_frames = ((seconds.max(0.0) * rate).round() as usize).min(frames);
    if fade_frames == 0 {
        return;
    }
    for frame in 0..fade_frames {
        let fraction = frame as f32 / fade_frames as f32;
        // Fade-in walks forward from the head and gains `fraction`; fade-out
        // walks backward from the tail so the final sample is silent.
        let gain = fraction;
        let target = match kind {
            FadeKind::In => frame,
            FadeKind::Out => frames - 1 - frame,
        };
        for channel in 0..channels {
            buffer[target * channels + channel] *= gain;
        }
    }
}

fn resample_linear(buffer: &[f32], channels: usize, factor: f32) -> Vec<f32> {
    let frames = buffer.len() / channels;
    let output_frames = ((frames as f32 / factor).round() as usize).max(1);
    let mut output = vec![0.0; output_frames * channels];
    for frame in 0..output_frames {
        let position = frame as f32 * factor;
        let index = position.floor() as usize;
        let fraction = position - index as f32;
        let index = index.min(frames - 1);
        let next = (index + 1).min(frames - 1);
        for channel in 0..channels {
            let a = buffer[index * channels + channel];
            let b = buffer[next * channels + channel];
            output[frame * channels + channel] = a + (b - a) * fraction;
        }
    }
    output
}

/// One undoable step.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    /// What the change was called.
    pub label: String,
    /// Parameter values before the change.
    pub params: EditParams,
}

/// An editing session: current parameters plus bounded undo and redo stacks.
#[derive(Debug, Clone)]
pub struct EditSession {
    source: SourceRef,
    current: EditParams,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
}

impl EditSession {
    /// A session with no edits applied.
    pub fn new(source: SourceRef) -> Self {
        Self {
            source,
            current: EditParams::default(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The immutable source description.
    pub fn source(&self) -> &SourceRef {
        &self.source
    }

    /// The current parameter set.
    pub fn current(&self) -> &EditParams {
        &self.current
    }

    /// Whether the session differs from the untouched original.
    pub fn is_modified(&self) -> bool {
        self.current.is_modified()
    }

    /// Commit a new parameter set as one undoable step.
    ///
    /// Validation happens before anything is mutated, so a rejected edit leaves
    /// both the current parameters and the history untouched.
    pub fn commit(
        &mut self,
        label: impl Into<String>,
        params: EditParams,
    ) -> Result<(), EditError> {
        params.validate(&self.source)?;
        if params == self.current {
            return Ok(());
        }
        self.undo.push(HistoryEntry {
            label: label.into(),
            params: self.current.clone(),
        });
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.current = params;
        Ok(())
    }

    /// Undo the last edit. Returns the label of the undone change.
    pub fn undo(&mut self) -> Option<String> {
        let entry = self.undo.pop()?;
        self.redo.push(HistoryEntry {
            label: entry.label.clone(),
            params: self.current.clone(),
        });
        self.current = entry.params;
        Some(entry.label)
    }

    /// Redo the last undone edit. Returns the label of the redone change.
    pub fn redo(&mut self) -> Option<String> {
        let entry = self.redo.pop()?;
        self.undo.push(HistoryEntry {
            label: entry.label.clone(),
            params: self.current.clone(),
        });
        self.current = entry.params;
        Some(entry.label)
    }

    /// The label of the change [`Self::undo`] would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|entry| entry.label.as_str())
    }

    /// The label of the change [`Self::redo`] would restore.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }

    /// Whether undo is available.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Restore the original parameters as an undoable step.
    pub fn reset(&mut self) -> Result<(), EditError> {
        self.commit("Reset to original", EditParams::default())
    }

    /// Render the current parameters over `source`.
    pub fn render(
        &self,
        source: &[f32],
        channels: u16,
        sample_rate: u32,
    ) -> Result<Vec<f32>, EditError> {
        render(source, channels, sample_rate, &self.current)
    }

    /// The source file path. Exposed for the relink flow, never for writing.
    pub fn source_path(&self) -> &std::path::Path {
        &self.source.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(frames: u64, sample_rate: u32) -> SourceRef {
        SourceRef {
            path: PathBuf::from("samples/kick.wav"),
            content_hash: "abc123".to_owned(),
            sample_rate,
            channels: 1,
            frames,
        }
    }

    fn ramp(frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| index as f32 / frames as f32)
            .collect()
    }

    #[test]
    fn defaults_are_the_original_and_unmodified() {
        let params = EditParams::default();
        let source = source(48_000, 48_000);
        assert!(!params.is_modified());
        assert_eq!(params.validate(&source), Ok(()));
        assert_eq!(params.trimmed_duration(&source), 1.0);
        assert_eq!(params.rendered_duration(&source), 1.0);
    }

    #[test]
    fn trimming_returns_only_the_selected_frames() {
        let source = source(48_000, 48_000);
        let mut params = EditParams::default();
        params.trim_start = 0.25;
        params.trim_end = Some(0.5);
        let buffer = ramp(48_000);
        let rendered = render(&buffer, 1, 48_000, &params).expect("render");
        assert_eq!(rendered.len(), 12_000);
        assert!((rendered[0] - 0.25).abs() < 0.001);
    }

    #[test]
    fn rendering_never_mutates_the_source_buffer() {
        let buffer = ramp(1_000);
        let original = buffer.clone();
        let mut params = EditParams::default();
        params.reverse = true;
        params.gain_db = 6.0;
        params.normalize = true;
        let _ = render(&buffer, 1, 48_000, &params).expect("render");
        assert_eq!(buffer, original);
    }

    #[test]
    fn reversal_flips_the_buffer() {
        let buffer = vec![0.0, 0.25, 0.5, 0.75];
        let mut params = EditParams::default();
        params.reverse = true;
        let rendered = render(&buffer, 1, 4, &params).expect("render");
        assert_eq!(rendered, vec![0.75, 0.5, 0.25, 0.0]);
    }

    #[test]
    fn fade_in_starts_at_silence_and_rises_monotonically() {
        let buffer = vec![1.0; 1_000];
        let mut params = EditParams::default();
        params.trim_end = Some(1.0);
        params.fade_in = 0.1;
        let rendered = render(&buffer, 1, 1_000, &params).expect("render");
        assert_eq!(rendered[0], 0.0);
        let tail = &rendered[100..];
        assert!(
            tail.iter()
                .all(|sample| (*sample - 1.0).abs() < f32::EPSILON)
        );
        for pair in rendered[..100].windows(2) {
            assert!(pair[1] >= pair[0]);
        }
    }

    #[test]
    fn fade_out_ends_at_silence() {
        let buffer = vec![1.0; 1_000];
        let mut params = EditParams::default();
        params.trim_end = Some(1.0);
        params.fade_out = 0.1;
        let rendered = render(&buffer, 1, 1_000, &params).expect("render");
        assert_eq!(*rendered.last().expect("last"), 0.0);
        assert!((rendered[0] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn normalize_scales_the_peak_to_the_target() {
        let buffer = vec![0.1; 100];
        let mut params = EditParams::default();
        params.normalize = true;
        params.trim_end = Some(100.0);
        let rendered = render(&buffer, 1, 100, &params).expect("render");
        let peak = rendered
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert!((peak - NORMALIZE_TARGET).abs() < 1e-5);
    }

    #[test]
    fn gain_multiplies_in_db() {
        let buffer = vec![0.5; 10];
        let mut params = EditParams::default();
        params.gain_db = 6.020_6;
        let rendered = render(&buffer, 1, 10, &params).expect("render");
        assert!((rendered[0] - 1.0).abs() < 0.001);
    }

    #[test]
    fn pitch_up_shortens_and_pitch_down_lengthens() {
        let buffer = ramp(1_000);
        let mut up = EditParams::default();
        up.trim_end = Some(1_000.0);
        up.pitch_semitones = 12.0;
        let rendered = render(&buffer, 1, 1_000, &up).expect("render");
        assert_eq!(rendered.len(), 500);

        let mut down = EditParams::default();
        down.trim_end = Some(1_000.0);
        down.pitch_semitones = -12.0;
        let rendered = render(&buffer, 1, 1_000, &down).expect("render");
        assert_eq!(rendered.len(), 2_000);
    }

    #[test]
    fn trim_range_must_be_inside_the_source() {
        let source = source(10_000, 10_000);
        let mut params = EditParams::default();
        params.trim_start = 2.0;
        params.trim_end = Some(1.0);
        assert!(matches!(
            params.validate(&source),
            Err(EditError::TrimRange { .. })
        ));
    }

    #[test]
    fn fades_cannot_exceed_the_trimmed_region() {
        let source = source(1_000, 1_000);
        let mut params = EditParams::default();
        params.trim_end = Some(0.1);
        params.fade_in = 0.06;
        params.fade_out = 0.06;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::FadeTooLong { .. })
        ));
    }

    #[test]
    fn loop_points_must_be_ordered_and_inside_the_trim() {
        let source = source(48_000, 48_000);
        let mut params = EditParams::default();
        params.loop_enabled = true;
        params.loop_start = 0.6;
        params.loop_end = 0.4;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::LoopRange { .. })
        ));

        let mut params = EditParams::default();
        params.trim_start = 0.2;
        params.trim_end = Some(0.5);
        params.loop_enabled = true;
        params.loop_start = 0.1;
        params.loop_end = 0.4;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::LoopOutsideTrim { .. })
        ));
    }

    #[test]
    fn loop_crossfade_cannot_exceed_half_the_loop() {
        let source = source(48_000, 48_000);
        let mut params = EditParams::default();
        params.loop_enabled = true;
        params.loop_start = 0.1;
        params.loop_end = 0.3;
        params.loop_crossfade = 0.2;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::LoopCrossfadeTooLong { .. })
        ));

        params.loop_crossfade = 0.09;
        assert_eq!(params.validate(&source), Ok(()));
    }

    #[test]
    fn non_finite_values_are_rejected_before_anything_else() {
        let source = source(1_000, 1_000);
        let mut params = EditParams::default();
        params.gain_db = f32::NAN;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::NotFinite { .. })
        ));
    }

    #[test]
    fn gain_and_pitch_ranges_are_enforced() {
        let source = source(1_000, 1_000);
        let mut params = EditParams::default();
        params.gain_db = 40.0;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::GainOutOfRange { .. })
        ));
        params.gain_db = 0.0;
        params.pitch_semitones = 30.0;
        assert!(matches!(
            params.validate(&source),
            Err(EditError::PitchOutOfRange { .. })
        ));
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let source = source(100_000, 1_000);
        let mut session = EditSession::new(source);
        let mut trimmed = EditParams::default();
        trimmed.trim_start = 1.0;
        trimmed.trim_end = Some(2.0);
        session.commit("Trim", trimmed.clone()).expect("commit");
        assert!(session.can_undo());
        assert_eq!(session.undo_label(), Some("Trim"));

        let mut gained = trimmed.clone();
        gained.gain_db = 3.0;
        session.commit("Gain", gained.clone()).expect("commit");

        assert_eq!(session.undo(), Some("Gain".to_owned()));
        assert_eq!(session.current(), &trimmed);
        assert_eq!(session.redo(), Some("Gain".to_owned()));
        assert_eq!(session.current(), &gained);
        assert_eq!(session.undo(), Some("Gain".to_owned()));
        assert_eq!(session.undo(), Some("Trim".to_owned()));
        assert_eq!(session.current(), &EditParams::default());
        assert!(!session.can_undo());
        assert!(!session.is_modified());
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let source = source(100_000, 1_000);
        let mut session = EditSession::new(source);
        let mut one = EditParams::default();
        one.gain_db = 1.0;
        session.commit("Gain 1", one.clone()).expect("commit");
        session.undo();
        assert!(session.can_redo());

        let mut two = EditParams::default();
        two.gain_db = 2.0;
        session.commit("Gain 2", two).expect("commit");
        assert!(!session.can_redo());
    }

    #[test]
    fn a_rejected_edit_leaves_the_session_untouched() {
        let source = source(1_000, 1_000);
        let mut session = EditSession::new(source);
        let mut invalid = EditParams::default();
        invalid.gain_db = 100.0;
        assert!(session.commit("Bad gain", invalid).is_err());
        assert_eq!(session.current(), &EditParams::default());
        assert!(!session.can_undo());
    }

    #[test]
    fn committing_an_identical_parameter_set_is_a_no_op() {
        let source = source(1_000, 1_000);
        let mut session = EditSession::new(source);
        session
            .commit("Same", EditParams::default())
            .expect("commit");
        assert!(!session.can_undo());
    }

    #[test]
    fn reset_is_undoable_and_restores_the_original() {
        let source = source(100_000, 1_000);
        let mut session = EditSession::new(source);
        let mut params = EditParams::default();
        params.gain_db = -6.0;
        session.commit("Gain", params).expect("commit");
        assert!(session.is_modified());

        session.reset().expect("reset");
        assert!(!session.is_modified());
        assert_eq!(session.undo_label(), Some("Reset to original"));
        session.undo();
        assert!(session.is_modified());
    }

    #[test]
    fn history_is_bounded() {
        let source = source(1_000_000, 1_000);
        let mut session = EditSession::new(source);
        for step in 1..=(MAX_HISTORY + 10) {
            let mut params = EditParams::default();
            params.gain_db = -(step as f32) * 0.01;
            session
                .commit(format!("Gain {step}"), params)
                .expect("commit");
        }
        let mut undo_count = 0;
        while session.undo().is_some() {
            undo_count += 1;
        }
        assert_eq!(undo_count, MAX_HISTORY);
    }

    #[test]
    fn rendered_duration_accounts_for_pitch() {
        let source = source(48_000, 48_000);
        let mut params = EditParams::default();
        params.pitch_semitones = 12.0;
        assert!((params.rendered_duration(&source) - 0.5).abs() < 0.001);
    }

    #[test]
    fn loop_window_is_relative_to_the_trim() {
        let mut params = EditParams::default();
        params.trim_start = 0.5;
        params.trim_end = Some(1.5);
        params.loop_enabled = true;
        params.loop_start = 0.75;
        params.loop_end = 1.25;
        assert_eq!(params.loop_window(), Some((0.25, 0.75)));
    }

    #[test]
    fn stereo_edits_keep_channels_aligned() {
        let buffer = vec![0.0, 1.0, 0.5, 1.5, 1.0, 2.0, 1.5, 2.5];
        let mut params = EditParams::default();
        params.reverse = true;
        let rendered = render(&buffer, 2, 4, &params).expect("render");
        assert_eq!(rendered, vec![1.5, 2.5, 1.0, 2.0, 0.5, 1.5, 0.0, 1.0]);
    }
}
