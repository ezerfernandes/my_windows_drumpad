//! Channel layout handling and offline sample-rate conversion.
//!
//! Resampling is a file-import concern, never an audio-callback concern. The
//! synchronous FFT resampler is fast and high quality for fixed-ratio
//! conversion, and `process_all` trims the startup delay so the result lines
//! up with the source.

use drumpad_model::kit::ContentHash;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use thiserror::Error;

use crate::decode::AudioData;

/// Optional mono downmix strategy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Downmix {
    /// Average every channel.
    #[default]
    Average,
    /// Keep the left channel only.
    Left,
    /// Keep the right channel only (left when mono).
    Right,
}

impl Downmix {
    /// Human-readable label for settings UI.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Average => "Average",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

/// Resampling and channel conversion failures.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum ResampleError {
    /// Source or target rate is zero.
    #[error("sample rate must be greater than zero (source {source_rate}, target {target_rate})")]
    InvalidRate {
        /// Source rate.
        source_rate: u32,
        /// Target rate.
        target_rate: u32,
    },
    /// The source buffer has no frames.
    #[error("cannot resample an empty sample")]
    Empty,
    /// Rubato could not be constructed.
    #[error("could not create resampler: {0}")]
    Construction(String),
    /// Rubato could not process the clip.
    #[error("resampling failed: {0}")]
    Process(String),
    /// The adapter rejected the buffer shape.
    #[error("invalid audio buffer: {0}")]
    Buffer(String),
}

/// Resample interleaved audio to a target rate.
pub fn resample(audio: &AudioData, target_rate: u32) -> Result<AudioData, ResampleError> {
    resample_with_chunk(audio, target_rate, 1024)
}

/// Resample with an explicit internal chunk size (mostly for tests/tuning).
pub fn resample_with_chunk(
    audio: &AudioData,
    target_rate: u32,
    chunk_size: usize,
) -> Result<AudioData, ResampleError> {
    if audio.sample_rate == 0 || target_rate == 0 {
        return Err(ResampleError::InvalidRate {
            source_rate: audio.sample_rate,
            target_rate,
        });
    }
    if audio.is_empty() {
        return Err(ResampleError::Empty);
    }
    if audio.sample_rate == target_rate {
        return Ok(audio.clone());
    }
    if audio.channels == 0 {
        return Err(ResampleError::Buffer("channel count is zero".into()));
    }

    let channels = usize::from(audio.channels);
    let frames = audio.frames();
    let mut resampler = Fft::<f32>::new(
        audio.sample_rate as usize,
        target_rate as usize,
        chunk_size.max(16),
        channels,
        FixedSync::Both,
    )
    .map_err(|error| ResampleError::Construction(error.to_string()))?;
    let input = InterleavedSlice::new(&audio.samples, channels, frames)
        .map_err(|error| ResampleError::Buffer(error.to_string()))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| ResampleError::Process(error.to_string()))?;
    let samples = output.take_data();
    if samples.is_empty() {
        return Err(ResampleError::Process(
            "resampler produced no frames".to_owned(),
        ));
    }
    AudioData::new(target_rate, audio.channels, samples)
        .map_err(|error| ResampleError::Buffer(error.to_string()))
}

/// Downmix to mono using the selected strategy.
pub fn downmix_to_mono(audio: &AudioData, mode: Downmix) -> Result<AudioData, ResampleError> {
    if audio.is_empty() {
        return Err(ResampleError::Empty);
    }
    if audio.channels == 0 {
        return Err(ResampleError::Buffer("channel count is zero".into()));
    }
    let channels = usize::from(audio.channels);
    if channels == 1 {
        return Ok(audio.clone());
    }
    let frames = audio.frames();
    let mut mono = Vec::with_capacity(frames);
    match mode {
        Downmix::Average => {
            let scale = 1.0 / channels as f32;
            for frame in 0..frames {
                let start = frame * channels;
                let sum: f32 = audio.samples[start..start + channels].iter().sum();
                mono.push(sum * scale);
            }
        }
        Downmix::Left => {
            for frame in 0..frames {
                mono.push(audio.samples[frame * channels]);
            }
        }
        Downmix::Right => {
            let index = if channels >= 2 { 1 } else { 0 };
            for frame in 0..frames {
                mono.push(audio.samples[frame * channels + index]);
            }
        }
    }
    AudioData::new(audio.sample_rate, 1, mono)
        .map_err(|error| ResampleError::Buffer(error.to_string()))
}

/// Downmix (optionally) and resample to the device rate in one step.
pub fn prepare_for_device(
    audio: &AudioData,
    device_rate: u32,
    mono: bool,
) -> Result<AudioData, ResampleError> {
    let staged = if mono {
        downmix_to_mono(audio, Downmix::Average)?
    } else {
        audio.clone()
    };
    resample(&staged, device_rate)
}

/// Canonical content hash for a decoded buffer.
pub fn content_hash(audio: &AudioData) -> ContentHash {
    audio.content_hash()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, frequency: f64, seconds: f64, channels: u16) -> AudioData {
        let frames = (seconds * f64::from(rate)) as usize;
        let mut samples = Vec::with_capacity(frames * usize::from(channels));
        for frame in 0..frames {
            let value = (2.0 * std::f64::consts::PI * frequency * frame as f64 / f64::from(rate))
                .sin() as f32
                * 0.5;
            for channel in 0..channels {
                samples.push(if channel == 1 { -value } else { value });
            }
        }
        AudioData::new(rate, channels, samples).unwrap()
    }

    fn amplitude_at(samples: &[f32], rate: u32, frequency: f64) -> f64 {
        let mut real = 0.0f64;
        let mut imaginary = 0.0f64;
        for (index, sample) in samples.iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * frequency * index as f64 / f64::from(rate);
            real += f64::from(*sample) * phase.cos();
            imaginary -= f64::from(*sample) * phase.sin();
        }
        2.0 * (real * real + imaginary * imaginary).sqrt() / samples.len() as f64
    }

    #[test]
    fn resample_matches_the_reference_spectrum() {
        let source = sine(48_000, 1_000.0, 0.5, 1);
        let resampled = resample(&source, 44_100).unwrap();
        let reference = sine(44_100, 1_000.0, 0.5, 1);
        let expected_frames = reference.frames();
        assert!(
            (resampled.frames() as isize - expected_frames as isize).abs() <= 2,
            "frames {} vs {}",
            resampled.frames(),
            expected_frames
        );

        // Compare a whole number of 1 kHz cycles to avoid spectral leakage.
        let window_frames = 4_410; // 100 cycles at 1 kHz / 44.1 kHz
        let start = 2_000;
        let end = start + window_frames;
        let reference_tone = amplitude_at(&reference.samples[start..end], 44_100, 1_000.0);
        let resampled_tone = amplitude_at(&resampled.samples[start..end], 44_100, 1_000.0);
        assert!(
            (resampled_tone - 0.5).abs() < 0.01,
            "tone amplitude changed: {resampled_tone}"
        );
        assert!(
            (resampled_tone - reference_tone).abs() < 0.01,
            "resampled amplitude {resampled_tone} differs from reference {reference_tone}"
        );
        let off_tone = amplitude_at(&resampled.samples[start..end], 44_100, 1_200.0);
        assert!(off_tone < 0.01, "unexpected energy at 1.2 kHz: {off_tone}");
    }

    #[test]
    fn stereo_placement_is_preserved_when_resampling() {
        let source = sine(44_100, 440.0, 0.25, 2);
        let mut distinct = source.clone();
        for frame in 0..distinct.frames() {
            distinct.samples[frame * 2 + 1] *= 0.25;
        }
        let resampled = resample(&distinct, 48_000).unwrap();
        assert_eq!(resampled.channels, 2);
        let left: Vec<f32> = resampled.channel(0).collect();
        let right: Vec<f32> = resampled.channel(1).collect();
        let left_energy: f32 = left.iter().map(|sample| sample * sample).sum();
        let right_energy: f32 = right.iter().map(|sample| sample * sample).sum();
        assert!(
            left_energy > right_energy * 4.0,
            "channel placement was not retained: {left_energy} vs {right_energy}"
        );
    }

    #[test]
    fn mono_downmix_averages_stereo() {
        let audio = AudioData::new(48_000, 2, vec![0.5, -0.5, 1.0, 0.0, -0.25, -0.75]).unwrap();
        let mono = downmix_to_mono(&audio, Downmix::Average).unwrap();
        assert_eq!(mono.channels, 1);
        assert_eq!(mono.samples, vec![0.0, 0.5, -0.5]);
        let left = downmix_to_mono(&audio, Downmix::Left).unwrap();
        assert_eq!(left.samples, vec![0.5, 1.0, -0.25]);
        let right = downmix_to_mono(&audio, Downmix::Right).unwrap();
        assert_eq!(right.samples, vec![-0.5, 0.0, -0.75]);
    }

    #[test]
    fn same_rate_is_a_no_op() {
        let audio = sine(48_000, 440.0, 0.01, 2);
        assert_eq!(resample(&audio, 48_000).unwrap(), audio);
    }

    #[test]
    fn empty_and_zero_rate_are_rejected() {
        let empty = AudioData::new(48_000, 2, Vec::new()).unwrap();
        assert!(matches!(
            resample(&empty, 44_100),
            Err(ResampleError::Empty)
        ));
        let audio = AudioData::new(48_000, 1, vec![0.0]).unwrap();
        assert!(matches!(
            resample(&audio, 0),
            Err(ResampleError::InvalidRate { .. })
        ));
    }

    #[test]
    fn prepare_for_device_downmixes_and_resamples() {
        let audio = sine(44_100, 440.0, 0.1, 2);
        let prepared = prepare_for_device(&audio, 48_000, true).unwrap();
        assert_eq!(prepared.channels, 1);
        assert_eq!(prepared.sample_rate, 48_000);
        assert!((prepared.duration_seconds() - 0.1).abs() < 0.01);
    }
}
