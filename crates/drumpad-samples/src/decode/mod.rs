//! Audio decoding into interleaved `f32`.
//!
//! Symphonia probes the container, selects the default audio track, and
//! decodes every packet. Metadata (format, rate, channels, duration) is
//! captured before or alongside the decode so the UI can show it while the
//! import is still pending.

use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use drumpad_model::kit::ContentHash;
use serde::{Deserialize, Serialize};
use symphonia::core::audio::SampleBuffer as SymphoniaSampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use thiserror::Error;

/// Container/format family recognized by the sample pipeline.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    /// RIFF/WAVE.
    Wav,
    /// AIFF/AIFF-C.
    Aiff,
    /// FLAC.
    Flac,
    /// MPEG audio layer III.
    Mp3,
    /// Ogg Vorbis.
    Ogg,
    /// Anything the probe accepted but the extension does not identify.
    Unknown,
}

impl AudioFormat {
    /// Detect from a file extension, case-insensitively.
    pub fn from_path(path: &Path) -> Self {
        path.extension()
            .and_then(|extension| extension.to_str())
            .map_or(Self::Unknown, Self::from_extension)
    }

    /// Detect from an extension without the leading dot.
    pub fn from_extension(extension: &str) -> Self {
        match extension.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Self::Wav,
            "aiff" | "aif" | "aifc" => Self::Aiff,
            "flac" => Self::Flac,
            "mp3" => Self::Mp3,
            "ogg" | "oga" => Self::Ogg,
            _ => Self::Unknown,
        }
    }

    /// True for the formats the importer advertises.
    pub fn is_supported_extension(extension: &str) -> bool {
        Self::from_extension(extension) != Self::Unknown
    }

    /// File extensions accepted by drag-and-drop and the file picker.
    pub const SUPPORTED_EXTENSIONS: &'static [&'static str] = &[
        "wav", "wave", "aiff", "aif", "aifc", "flac", "mp3", "ogg", "oga",
    ];
}

impl fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Wav => "WAV",
            Self::Aiff => "AIFF",
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
            Self::Ogg => "Ogg Vorbis",
            Self::Unknown => "unknown",
        })
    }
}

/// Decoded interleaved PCM plus its source format.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AudioData {
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Interleaved samples, `frames * channels` values.
    pub samples: Vec<f32>,
}

/// Invalid audio buffer construction.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum AudioDataError {
    /// Sample rate must be positive.
    #[error("sample rate must be greater than zero")]
    ZeroRate,
    /// At least one channel is required.
    #[error("channel count must be at least one")]
    NoChannels,
    /// Sample count must be a multiple of the channel count.
    #[error("{samples} samples is not a whole number of frames for {channels} channels")]
    UnevenFrames {
        /// Number of interleaved samples.
        samples: usize,
        /// Channel count.
        channels: u16,
    },
    /// A sample was NaN or infinite.
    #[error("sample {index} is not finite")]
    NonFinite {
        /// Index of the offending sample.
        index: usize,
    },
}

impl AudioData {
    /// Construct after validating shape and finiteness.
    pub fn new(sample_rate: u32, channels: u16, samples: Vec<f32>) -> Result<Self, AudioDataError> {
        if sample_rate == 0 {
            return Err(AudioDataError::ZeroRate);
        }
        if channels == 0 {
            return Err(AudioDataError::NoChannels);
        }
        if samples.len() % usize::from(channels) != 0 {
            return Err(AudioDataError::UnevenFrames {
                samples: samples.len(),
                channels,
            });
        }
        if let Some(index) = samples.iter().position(|sample| !sample.is_finite()) {
            return Err(AudioDataError::NonFinite { index });
        }
        Ok(Self {
            sample_rate,
            channels,
            samples,
        })
    }

    /// Construct without validation. Internal callers only.
    pub(crate) fn from_decoded(sample_rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        Self {
            sample_rate,
            channels,
            samples,
        }
    }

    /// Number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / usize::from(self.channels)
        }
    }

    /// Duration in seconds.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / f64::from(self.sample_rate)
        }
    }

    /// True when the buffer has no frames.
    pub fn is_empty(&self) -> bool {
        self.frames() == 0
    }

    /// Canonical decoded-content hash.
    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of_pcm(self.sample_rate, self.channels, &self.samples)
    }

    /// Read one channel's samples (zero-based).
    pub fn channel(&self, channel: usize) -> impl Iterator<Item = f32> + '_ {
        let channels = usize::from(self.channels.max(1));
        let channel = channel.min(channels - 1);
        self.samples.iter().skip(channel).step_by(channels).copied()
    }

    /// Deep equality test used by tests and cache checks.
    pub fn same_audio(&self, other: &Self) -> bool {
        self.sample_rate == other.sample_rate
            && self.channels == other.channels
            && self.samples == other.samples
    }

    /// Move the buffer into a shared immutable [`SampleBuffer`] without
    /// copying the PCM data.
    pub fn into_sample_buffer(self) -> Result<SampleBuffer, SampleBufferError> {
        let channels = u8::try_from(self.channels)
            .map_err(|_| SampleBufferError::UnsupportedChannels(u8::MAX))?;
        SampleBuffer::new(self.samples, channels, self.sample_rate)
    }
}

impl DecodedAudio {
    /// Move a decoded sample into the shared immutable form used by the engine.
    pub fn into_sample(self) -> Result<DecodedSample, SampleBufferError> {
        let sample = self.audio.into_sample_buffer()?;
        Ok(DecodedSample {
            sample,
            metadata: self.metadata,
        })
    }
}

/// Source metadata available before a full sample buffer is assigned.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AudioMetadata {
    /// Detected container format.
    pub format: AudioFormat,
    /// Source sample rate in Hz.
    pub sample_rate: u32,
    /// Source channel count.
    pub channels: u16,
    /// Source frame count when the container reports it.
    pub frames: Option<u64>,
    /// Duration in seconds when the frame count is known.
    pub duration_seconds: Option<f64>,
}

impl AudioMetadata {
    /// True when the source is mono.
    pub fn is_mono(&self) -> bool {
        self.channels == 1
    }
}

/// A decoded sample plus its metadata.
#[derive(Clone, PartialEq, Debug)]
pub struct DecodedAudio {
    /// Interleaved PCM.
    pub audio: AudioData,
    /// Source metadata.
    pub metadata: AudioMetadata,
}

/// Maximum number of channels the playback engine accepts.
pub const MAX_SAMPLE_CHANNELS: u8 = 2;

/// Immutable, shared-ownership sample buffer for the audio thread.
///
/// The buffer keeps its native sample rate; the engine adjusts the playback
/// increment when a sample rate differs from the device rate. Cloning a
/// `SampleBuffer` is cheap (`Arc` refcount only), so publishing a new sample
/// bank never copies audio.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleBuffer {
    data: std::sync::Arc<[f32]>,
    channels: u8,
    sample_rate: u32,
}

/// Invalid [`SampleBuffer`] construction.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum SampleBufferError {
    /// A buffer must contain at least one frame.
    #[error("sample buffer is empty")]
    Empty,
    /// Channel count must be 1 or 2.
    #[error("unsupported channel count {0}; expected 1 or 2")]
    UnsupportedChannels(u8),
    /// Sample rate must be positive.
    #[error("sample rate must be greater than zero")]
    InvalidSampleRate,
    /// Samples must be a whole number of frames.
    #[error("sample data is not aligned to the channel count")]
    MisalignedData,
}

impl SampleBuffer {
    /// Build from interleaved `f32` data.
    pub fn new(data: Vec<f32>, channels: u8, sample_rate: u32) -> Result<Self, SampleBufferError> {
        if data.is_empty() {
            return Err(SampleBufferError::Empty);
        }
        if !(1..=MAX_SAMPLE_CHANNELS).contains(&channels) {
            return Err(SampleBufferError::UnsupportedChannels(channels));
        }
        if sample_rate == 0 {
            return Err(SampleBufferError::InvalidSampleRate);
        }
        if data.len() % usize::from(channels) != 0 {
            return Err(SampleBufferError::MisalignedData);
        }
        Ok(Self {
            data: std::sync::Arc::from(data),
            channels,
            sample_rate,
        })
    }

    /// Build a mono buffer.
    pub fn from_mono(data: Vec<f32>, sample_rate: u32) -> Result<Self, SampleBufferError> {
        Self::new(data, 1, sample_rate)
    }

    /// Build a stereo buffer from interleaved `[L, R, ...]` data.
    pub fn from_stereo(data: Vec<f32>, sample_rate: u32) -> Result<Self, SampleBufferError> {
        Self::new(data, 2, sample_rate)
    }

    /// Convert a validated [`AudioData`] view.
    pub fn from_audio(audio: &AudioData) -> Result<Self, SampleBufferError> {
        let channels = u8::try_from(audio.channels)
            .map_err(|_| SampleBufferError::UnsupportedChannels(u8::MAX))?;
        Self::new(audio.samples.clone(), channels, audio.sample_rate)
    }

    /// Convert back into an owned [`AudioData`].
    pub fn to_audio(&self) -> Result<AudioData, AudioDataError> {
        AudioData::new(
            self.sample_rate,
            u16::from(self.channels),
            self.data.to_vec(),
        )
    }

    /// Channel count (1 or 2).
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Native sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames per channel.
    pub fn frame_count(&self) -> usize {
        self.data.len() / usize::from(self.channels)
    }

    /// Duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.frame_count() as f64 / f64::from(self.sample_rate)
    }

    /// Borrow the interleaved data.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Canonical decoded-content hash.
    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of_pcm(self.sample_rate, u16::from(self.channels), &self.data)
    }
}

impl TryFrom<&AudioData> for SampleBuffer {
    type Error = SampleBufferError;

    fn try_from(audio: &AudioData) -> Result<Self, Self::Error> {
        Self::from_audio(audio)
    }
}

/// Source metadata plus an immutable sample buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSample {
    /// Immutable audio buffer.
    pub sample: SampleBuffer,
    /// Source metadata.
    pub metadata: AudioMetadata,
}

impl TryFrom<DecodedAudio> for DecodedSample {
    type Error = SampleBufferError;

    fn try_from(decoded: DecodedAudio) -> Result<Self, Self::Error> {
        decoded.into_sample()
    }
}

/// Decode failures with enough context to identify the offending file.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// The file could not be opened or read.
    #[error("failed to open {path}: {source}")]
    Io {
        /// File path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// No audio track or a codec the build does not support.
    #[error("unsupported audio in {path}: {detail}")]
    Unsupported {
        /// File path.
        path: PathBuf,
        /// Probe or codec explanation.
        detail: String,
    },
    /// The container or bitstream is malformed.
    #[error("corrupt audio in {path}: {detail}")]
    Corrupt {
        /// File path.
        path: PathBuf,
        /// Decoder explanation.
        detail: String,
    },
    /// The file decoded to zero frames.
    #[error("audio file {path} contains no frames")]
    Empty {
        /// File path.
        path: PathBuf,
    },
    /// Decoded buffers changed format mid-stream.
    #[error("audio format changed while decoding {path}: {detail}")]
    FormatChanged {
        /// File path.
        path: PathBuf,
        /// Explanation.
        detail: String,
    },
}

fn probe(path: &Path) -> Result<symphonia::core::probe::ProbeResult, DecodeError> {
    let file = File::open(path).map_err(|source| DecodeError::Io {
        path: path.to_owned(),
        source,
    })?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let format_options = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    symphonia::default::get_probe()
        .format(&hint, stream, &format_options, &MetadataOptions::default())
        .map_err(|error| DecodeError::Unsupported {
            path: path.to_owned(),
            detail: error.to_string(),
        })
}

/// Read source metadata without decoding the full file.
pub fn probe_file(path: &Path) -> Result<AudioMetadata, DecodeError> {
    let probed = probe(path)?;
    let format = probed
        .format
        .default_track()
        .ok_or_else(|| DecodeError::Unsupported {
            path: path.to_owned(),
            detail: "container has no audio track".into(),
        })?;
    metadata_for(path, &format.codec_params)
}

fn metadata_for(
    path: &Path,
    params: &symphonia::core::codecs::CodecParameters,
) -> Result<AudioMetadata, DecodeError> {
    let sample_rate = params.sample_rate.ok_or_else(|| DecodeError::Unsupported {
        path: path.to_owned(),
        detail: "track does not declare a sample rate".into(),
    })?;
    let channels = params
        .channels
        .map(|channels| channels.count() as u16)
        .filter(|channels| *channels > 0)
        .ok_or_else(|| DecodeError::Unsupported {
            path: path.to_owned(),
            detail: "track does not declare a channel layout".into(),
        })?;
    if params.codec == CODEC_TYPE_NULL {
        return Err(DecodeError::Unsupported {
            path: path.to_owned(),
            detail: "track uses an unknown codec".into(),
        });
    }
    let frames = params.n_frames;
    let duration_seconds = frames.map(|frames| frames as f64 / f64::from(sample_rate));
    Ok(AudioMetadata {
        format: AudioFormat::from_path(path),
        sample_rate,
        channels,
        frames,
        duration_seconds,
    })
}

/// Decode a file into interleaved `f32`.
pub fn decode_file(path: &Path) -> Result<DecodedAudio, DecodeError> {
    let probed = probe(path)?;
    let format = probed
        .format
        .default_track()
        .ok_or_else(|| DecodeError::Unsupported {
            path: path.to_owned(),
            detail: "container has no audio track".into(),
        })?;
    let mut metadata = metadata_for(path, &format.codec_params)?;
    let track_id = format.id;
    let codec_params = format.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|error| DecodeError::Unsupported {
            path: path.to_owned(),
            detail: error.to_string(),
        })?;
    let mut reader = probed.format;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = metadata.sample_rate;
    let mut channels = metadata.channels;

    loop {
        let packet = match reader.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(DecodeError::FormatChanged {
                    path: path.to_owned(),
                    detail: "stream requires a decoder reset".into(),
                });
            }
            Err(error) => {
                return Err(DecodeError::Corrupt {
                    path: path.to_owned(),
                    detail: error.to_string(),
                });
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(detail)) => {
                return Err(DecodeError::Corrupt {
                    path: path.to_owned(),
                    detail: detail.to_string(),
                });
            }
            Err(SymphoniaError::IoError(error)) => {
                return Err(DecodeError::Io {
                    path: path.to_owned(),
                    source: error,
                });
            }
            Err(error) => {
                return Err(DecodeError::Corrupt {
                    path: path.to_owned(),
                    detail: error.to_string(),
                });
            }
        };
        let spec = *decoded.spec();
        let decoded_channels = spec.channels.count() as u16;
        if !samples.is_empty() && (spec.rate != sample_rate || decoded_channels != channels) {
            return Err(DecodeError::FormatChanged {
                path: path.to_owned(),
                detail: format!(
                    "started at {sample_rate} Hz/{channels}ch, then {}/{}ch",
                    spec.rate, decoded_channels
                ),
            });
        }
        sample_rate = spec.rate;
        channels = decoded_channels;
        if channels == 0 {
            return Err(DecodeError::Corrupt {
                path: path.to_owned(),
                detail: "decoder produced a buffer with zero channels".into(),
            });
        }

        let mut buffer = SymphoniaSampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        samples.extend_from_slice(buffer.samples());
    }

    if samples.is_empty() {
        return Err(DecodeError::Empty {
            path: path.to_owned(),
        });
    }
    if samples.len() % usize::from(channels) != 0 {
        return Err(DecodeError::Corrupt {
            path: path.to_owned(),
            detail: "decoded sample count is not frame-aligned".into(),
        });
    }

    metadata.sample_rate = sample_rate;
    metadata.channels = channels;
    metadata.frames = Some((samples.len() / usize::from(channels)) as u64);
    metadata.duration_seconds =
        Some((samples.len() / usize::from(channels)) as f64 / f64::from(sample_rate));

    Ok(DecodedAudio {
        audio: AudioData::from_decoded(sample_rate, channels, samples),
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn decodes_every_supported_format() {
        for (name, format) in [
            ("tone.wav", AudioFormat::Wav),
            ("tone.aiff", AudioFormat::Aiff),
            ("tone.flac", AudioFormat::Flac),
            ("tone.mp3", AudioFormat::Mp3),
            ("tone.ogg", AudioFormat::Ogg),
        ] {
            let decoded = decode_file(&fixture(name)).unwrap_or_else(|error| {
                panic!("failed to decode {name}: {error}");
            });
            assert_eq!(decoded.audio.sample_rate, 44_100, "{name}");
            assert_eq!(decoded.audio.channels, 2, "{name}");
            assert!(decoded.audio.frames() > 1_000, "{name}");
            assert_eq!(decoded.metadata.format, format, "{name}");
            assert!(
                (decoded.audio.duration_seconds() - 0.25).abs() < 0.06,
                "{name}: got {} s",
                decoded.audio.duration_seconds()
            );
            assert!(
                decoded
                    .audio
                    .samples
                    .iter()
                    .all(|sample| sample.is_finite())
            );
        }
    }

    #[test]
    fn metadata_is_available_before_decoding() {
        let metadata = probe_file(&fixture("tone.wav")).unwrap();
        assert_eq!(metadata.sample_rate, 44_100);
        assert_eq!(metadata.channels, 2);
        assert!(metadata.frames.unwrap() > 1_000);
        assert!(metadata.duration_seconds.unwrap() > 0.2);
        assert_eq!(metadata.format, AudioFormat::Wav);
    }

    #[test]
    fn corrupt_files_return_typed_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.wav");
        std::fs::write(&path, b"this is not audio at all").unwrap();
        let error = decode_file(&path).unwrap_err();
        assert!(
            matches!(
                error,
                DecodeError::Unsupported { .. } | DecodeError::Corrupt { .. }
            ),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn unsupported_extension_is_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.xyz");
        std::fs::write(&path, b"random").unwrap();
        let error = decode_file(&path).unwrap_err();
        assert!(matches!(error, DecodeError::Unsupported { .. }));
        assert!(!AudioFormat::is_supported_extension("xyz"));
    }

    #[test]
    fn truncated_wav_does_not_panic() {
        let bytes = std::fs::read(fixture("tone.wav")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.wav");
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let _ = decode_file(&path);
    }

    #[test]
    fn content_hash_uses_decoded_pcm() {
        let wav = decode_file(&fixture("tone.wav")).unwrap();
        let flac = decode_file(&fixture("tone.flac")).unwrap();
        // Both derive from the same source, but FLAC is lossless, so their
        // decoded content hashes must match exactly.
        assert_eq!(wav.audio.content_hash(), flac.audio.content_hash());
    }

    #[test]
    fn audio_data_moves_into_a_sample_buffer() {
        let audio = AudioData::new(48_000, 2, vec![0.0, 1.0, 0.5, -0.5]).unwrap();
        let expected_hash = audio.content_hash();
        let buffer = audio.clone().into_sample_buffer().unwrap();
        assert_eq!(buffer.data(), &[0.0, 1.0, 0.5, -0.5]);
        assert_eq!(buffer.content_hash(), expected_hash);

        let decoded = DecodedAudio {
            audio,
            metadata: AudioMetadata {
                format: AudioFormat::Wav,
                sample_rate: 48_000,
                channels: 2,
                frames: Some(2),
                duration_seconds: Some(2.0 / 48_000.0),
            },
        };
        let sample = decoded.into_sample().unwrap();
        assert_eq!(sample.sample, buffer);
    }

    #[test]
    fn sample_buffer_shares_an_immutable_arc() {
        let audio = AudioData::new(48_000, 2, vec![0.1, -0.1, 0.2, -0.2]).unwrap();
        let buffer = SampleBuffer::try_from(&audio).unwrap();
        let clone = buffer.clone();
        assert_eq!(buffer.data().as_ptr(), clone.data().as_ptr());
        assert_eq!(buffer.channels(), 2);
        assert_eq!(buffer.sample_rate(), 48_000);
        assert_eq!(buffer.frame_count(), 2);
        assert!(buffer.duration_secs() > 0.0);
        assert_eq!(buffer.content_hash(), audio.content_hash());
        assert_eq!(buffer.to_audio().unwrap(), audio);

        let decoded = DecodedAudio {
            audio: audio.clone(),
            metadata: AudioMetadata {
                format: AudioFormat::Wav,
                sample_rate: 48_000,
                channels: 2,
                frames: Some(2),
                duration_seconds: Some(2.0 / 48_000.0),
            },
        };
        let sample = DecodedSample::try_from(decoded).unwrap();
        assert_eq!(sample.sample, buffer);
        assert_eq!(sample.metadata.channels, 2);
    }

    #[test]
    fn sample_buffer_rejects_invalid_shapes() {
        assert_eq!(
            SampleBuffer::from_mono(Vec::new(), 48_000),
            Err(SampleBufferError::Empty)
        );
        assert_eq!(
            SampleBuffer::new(vec![0.0; 6], 3, 48_000),
            Err(SampleBufferError::UnsupportedChannels(3))
        );
        assert_eq!(
            SampleBuffer::new(vec![0.0; 3], 2, 48_000),
            Err(SampleBufferError::MisalignedData)
        );
        assert_eq!(
            SampleBuffer::from_mono(vec![0.0], 0),
            Err(SampleBufferError::InvalidSampleRate)
        );
    }

    #[test]
    fn audio_formats_render_and_parse_extensions() {
        let cases = [
            (AudioFormat::Wav, "WAV"),
            (AudioFormat::Aiff, "AIFF"),
            (AudioFormat::Flac, "FLAC"),
            (AudioFormat::Mp3, "MP3"),
            (AudioFormat::Ogg, "Ogg Vorbis"),
            (AudioFormat::Unknown, "unknown"),
        ];
        for (format, label) in cases {
            assert_eq!(format.to_string(), label);
        }
        assert_eq!(
            AudioFormat::from_path(Path::new("loop.WAV")),
            AudioFormat::Wav
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("loop.aiff")),
            AudioFormat::Aiff
        );
        assert_eq!(
            AudioFormat::from_path(Path::new("loop")),
            AudioFormat::Unknown
        );
        for extension in [
            "wav", "wave", "aiff", "aif", "aifc", "flac", "mp3", "ogg", "oga",
        ] {
            assert!(
                AudioFormat::is_supported_extension(extension),
                "{extension}"
            );
        }
        assert!(!AudioFormat::is_supported_extension("txt"));
    }

    #[test]
    fn audio_data_validates_shapes_and_exposes_frames() {
        assert_eq!(
            AudioData::new(0, 1, vec![0.0]),
            Err(AudioDataError::ZeroRate)
        );
        assert_eq!(
            AudioData::new(48_000, 0, Vec::new()),
            Err(AudioDataError::NoChannels)
        );
        assert_eq!(
            AudioData::new(48_000, 2, vec![0.0; 3]),
            Err(AudioDataError::UnevenFrames {
                samples: 3,
                channels: 2
            })
        );
        assert_eq!(
            AudioData::new(48_000, 1, vec![f32::NAN]),
            Err(AudioDataError::NonFinite { index: 0 })
        );

        let audio = AudioData::new(48_000, 2, vec![1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(audio.frames(), 2);
        assert!(!audio.is_empty());
        assert!((audio.duration_seconds() - 2.0 / 48_000.0).abs() < f64::EPSILON);
        assert_eq!(audio.channel(0).collect::<Vec<_>>(), vec![1.0, 3.0]);
        assert_eq!(audio.channel(99).collect::<Vec<_>>(), vec![2.0, 4.0]);
        assert!(audio.same_audio(&audio.clone()));
        let mut different = audio.clone();
        different.samples[0] = 9.0;
        assert!(!audio.same_audio(&different));

        let empty = AudioData {
            sample_rate: 0,
            channels: 0,
            samples: Vec::new(),
        };
        assert_eq!(empty.frames(), 0);
        assert_eq!(empty.duration_seconds(), 0.0);
        assert!(empty.is_empty());
    }

    #[test]
    fn metadata_and_sample_buffer_conversions_round_trip() {
        let metadata = AudioMetadata {
            format: AudioFormat::Wav,
            sample_rate: 48_000,
            channels: 2,
            frames: Some(2),
            duration_seconds: Some(2.0 / 48_000.0),
        };
        assert!(!metadata.is_mono());
        assert!(
            AudioMetadata {
                channels: 1,
                ..metadata.clone()
            }
            .is_mono()
        );

        let audio = AudioData::new(48_000, 2, vec![0.1, -0.1, 0.2, -0.2]).unwrap();
        let buffer = SampleBuffer::from_stereo(audio.samples.clone(), 48_000).unwrap();
        assert_eq!(buffer.channels(), 2);
        assert_eq!(buffer.sample_rate(), 48_000);
        assert_eq!(buffer.frame_count(), 2);
        assert!((buffer.duration_secs() - 2.0 / 48_000.0).abs() < f64::EPSILON);
        assert_eq!(buffer.data(), audio.samples.as_slice());
        assert_eq!(buffer.to_audio().unwrap(), audio);

        let moved = audio.clone().into_sample_buffer().unwrap();
        assert_eq!(moved, buffer);
        let decoded = DecodedAudio { audio, metadata };
        let sample = decoded.into_sample().unwrap();
        assert_eq!(sample.sample, buffer);

        let wide = AudioData::new(48_000, 300, vec![0.0; 300]).unwrap();
        assert_eq!(
            SampleBuffer::from_audio(&wide),
            Err(SampleBufferError::UnsupportedChannels(u8::MAX))
        );
    }

    #[test]
    fn empty_wav_decodes_to_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.finalize().unwrap();
        let error = decode_file(&path).unwrap_err();
        assert!(matches!(error, DecodeError::Empty { .. }), "{error}");
    }
}
