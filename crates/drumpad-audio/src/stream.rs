//! Stream configuration, format negotiation, and the render-stream lifecycle.
//!
//! The Windows path owns a dedicated stream thread. Opening a stream tries the
//! requested sharing mode first and falls back predictably:
//!
//! 1. Exclusive as requested (integer 16/24-bit); on any format/mode failure,
//! 2. Shared low-latency (`IAudioClient3`) if the requested period is inside
//!    the device's supported period range,
//! 3. Shared default engine period.
//!
//! The chosen format, buffer size and callback period are reported in
//! [`StreamFormat`] so the UI can show what actually happened.

use crate::AudioError;
use crate::backend::AudioCallback;
use crate::diagnostics::StreamDiagnostics;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// WASAPI sharing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShareMode {
    /// Mix with other applications through the Windows audio engine.
    #[default]
    Shared,
    /// Take the endpoint exclusively. Lowest latency, blocks other apps.
    Exclusive,
}

impl std::fmt::Display for ShareMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareMode::Shared => write!(f, "shared"),
            ShareMode::Exclusive => write!(f, "exclusive"),
        }
    }
}

/// Sample format used on the wire to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SampleFormat {
    /// 32-bit IEEE float. Used by shared mode.
    F32,
    /// 16-bit signed integer.
    I16,
    /// 24-bit signed integer packed in 3 bytes.
    I24,
}

impl SampleFormat {
    /// Bytes per sample.
    pub fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::F32 => 4,
            SampleFormat::I16 => 2,
            SampleFormat::I24 => 3,
        }
    }

    /// Bits per sample as reported to the user.
    pub fn bits(self) -> u16 {
        match self {
            SampleFormat::F32 => 32,
            SampleFormat::I16 => 16,
            SampleFormat::I24 => 24,
        }
    }
}

/// Requested stream configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Endpoint id, or `None` for the current default render device.
    pub device_id: Option<String>,
    /// Requested sample rate in Hz.
    pub sample_rate: u32,
    /// Requested channel count.
    pub channels: u16,
    /// Requested buffer/period in frames.
    pub buffer_frames: u32,
    /// Requested sharing mode.
    pub share_mode: ShareMode,
    /// Try the IAudioClient3 low-latency shared period first.
    pub low_latency_shared: bool,
    /// Fall back to shared mode when exclusive mode is unavailable.
    pub fallback_to_shared: bool,
    /// Preferred exclusive-mode bit depth (16 or 24).
    pub exclusive_bits: u16,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: 128,
            share_mode: ShareMode::Shared,
            low_latency_shared: true,
            fallback_to_shared: true,
            exclusive_bits: 16,
        }
    }
}

impl StreamConfig {
    /// Human-readable description for error messages.
    pub fn format_request(&self) -> String {
        format!(
            "{} Hz, {} ch, {} frames, {} mode",
            self.sample_rate, self.channels, self.buffer_frames, self.share_mode
        )
    }
}

/// The format that was actually negotiated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamFormat {
    /// Endpoint id.
    pub device_id: String,
    /// Friendly device name.
    pub device_name: String,
    /// Negotiated sample rate.
    pub sample_rate: u32,
    /// Negotiated channel count.
    pub channels: u16,
    /// Wire sample format.
    pub sample_format: SampleFormat,
    /// Negotiated sharing mode.
    pub share_mode: ShareMode,
    /// Device buffer size in frames.
    pub buffer_frames: u32,
    /// One callback period in frames.
    pub callback_period_frames: u32,
    /// Driver-reported stream latency in milliseconds, when non-zero.
    pub stream_latency_ms: Option<f32>,
    /// True when the requested mode/format could not be used and a fallback ran.
    pub fell_back: bool,
    /// Why the fallback happened.
    pub fallback_reason: Option<String>,
}

impl StreamFormat {
    /// One callback period in milliseconds.
    pub fn callback_period_ms(&self) -> f32 {
        self.callback_period_frames as f32 / self.sample_rate.max(1) as f32 * 1000.0
    }

    /// Device buffer duration in milliseconds.
    pub fn buffer_ms(&self) -> f32 {
        self.buffer_frames as f32 / self.sample_rate.max(1) as f32 * 1000.0
    }

    /// Software latency estimate before the analog path. `GetStreamLatency`
    /// already includes the queued device buffer where the driver reports it,
    /// so take the maximum instead of adding the two.
    pub fn estimated_latency_ms(&self) -> f32 {
        self.buffer_ms().max(self.stream_latency_ms.unwrap_or(0.0))
    }
}

/// Control surface for a running stream.
pub trait StreamControl: Send {
    /// Stop the stream and join the audio thread.
    fn stop(&mut self);
    /// True while the audio thread is rendering.
    fn is_running(&self) -> bool;
    /// Shared diagnostics counters.
    fn diagnostics(&self) -> &Arc<StreamDiagnostics>;
}

/// A running render stream plus the format it actually negotiated.
pub struct RenderStream {
    format: StreamFormat,
    control: Box<dyn StreamControl>,
}

impl RenderStream {
    /// Wrap a control implementation.
    pub fn new(format: StreamFormat, control: Box<dyn StreamControl>) -> Self {
        Self { format, control }
    }

    /// Negotiated format.
    pub fn format(&self) -> &StreamFormat {
        &self.format
    }

    /// Shared diagnostics.
    pub fn diagnostics(&self) -> &Arc<StreamDiagnostics> {
        self.control.diagnostics()
    }

    /// True while the stream thread is rendering.
    pub fn is_running(&self) -> bool {
        self.control.is_running()
    }

    /// Stop and join the stream thread.
    pub fn stop(&mut self) {
        self.control.stop();
    }
}

impl Drop for RenderStream {
    fn drop(&mut self) {
        self.control.stop();
    }
}

impl std::fmt::Debug for RenderStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderStream")
            .field("format", &self.format)
            .field("running", &self.is_running())
            .finish()
    }
}

/// Interleaved `f32` to wire-format conversion. Kept public for the spike and
/// for tests; it is allocation-free and runs on the audio thread.
pub fn convert_f32(out: &mut [u8], input: &[f32], format: SampleFormat) {
    match format {
        SampleFormat::F32 => {
            for (sample, chunk) in input.iter().zip(out.chunks_exact_mut(4)) {
                chunk.copy_from_slice(&sample.to_le_bytes());
            }
        }
        SampleFormat::I16 => {
            for (sample, chunk) in input.iter().zip(out.chunks_exact_mut(2)) {
                let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
                chunk.copy_from_slice(&value.to_le_bytes());
            }
        }
        SampleFormat::I24 => {
            for (sample, chunk) in input.iter().zip(out.chunks_exact_mut(3)) {
                let value = (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                let bytes = value.to_le_bytes();
                chunk[0] = bytes[0];
                chunk[1] = bytes[1];
                chunk[2] = bytes[2];
            }
        }
    }
}

/// Open a render stream on the platform backend.
///
/// The callback is moved to the audio thread and must not allocate, lock, log,
/// or touch the filesystem.
pub fn open_render_stream(
    config: StreamConfig,
    callback: Box<dyn AudioCallback>,
) -> Result<RenderStream, AudioError> {
    #[cfg(windows)]
    {
        windows::open(config, callback)
    }
    #[cfg(not(windows))]
    {
        let _ = (config, callback);
        Err(AudioError::Backend(
            "the WASAPI render stream is only available on Windows".into(),
        ))
    }
}

/// Expected interval between callbacks.
pub(crate) fn callback_interval(period_frames: u32, sample_rate: u32) -> Duration {
    let nanos = period_frames as u128 * 1_000_000_000u128 / sample_rate.max(1) as u128;
    Duration::from_nanos(nanos.max(1) as u64)
}

/// Shared loop body used by the Windows stream and the fake backend's thread.
pub(crate) fn callback_overrun(interval: Duration, expected: Duration) -> bool {
    interval.as_nanos() as f64 > expected.as_nanos() as f64 * 1.5
}

#[cfg(windows)]
mod windows {
    use super::*;
    // Explicit import: `wasapi::*` also exports a `ShareMode`.
    use crate::stream::ShareMode;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use wasapi::*;

    pub(super) fn open(
        config: StreamConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<RenderStream, AudioError> {
        let diagnostics = Arc::new(StreamDiagnostics::new());
        diagnostics.set_state(crate::diagnostics::StreamState::Starting);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_diagnostics = Arc::clone(&diagnostics);
        let (sender, receiver) = mpsc::sync_channel(1);

        let handle: JoinHandle<()> = thread::spawn(move || {
            let _mmcss = crate::priority::boost_audio_thread();
            let result = open_inner(&config, callback, &thread_stop, &thread_diagnostics);
            match result {
                Ok(mut running) => {
                    if thread_stop.load(Ordering::Relaxed) {
                        // The waiter timed out; drop the initialized stream on
                        // this thread instead of sending a stale success.
                        thread_diagnostics.set_state(crate::diagnostics::StreamState::Stopped);
                        return;
                    }
                    let _ = sender.send(Ok(running.format.clone()));
                    run_loop(&mut running, &thread_stop, &thread_diagnostics);
                    thread_diagnostics.set_state(crate::diagnostics::StreamState::Stopped);
                }
                Err(error) => {
                    thread_diagnostics.set_error(error.to_string());
                    let _ = sender.send(Err(error));
                }
            }
        });

        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(format)) => {
                diagnostics.set_state(crate::diagnostics::StreamState::Running);
                diagnostics.set_stream_latency(format.stream_latency_ms);
                Ok(RenderStream::new(
                    format,
                    Box::new(WindowsStreamControl {
                        stop,
                        handle: Some(handle),
                        running: true,
                        diagnostics,
                    }),
                ))
            }
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                // Deliberately detach: the thread checks `stop` after opening
                // and owns all COM objects, so it will exit without leaking a
                // stream. Joining here could block forever on a bad driver.
                drop(handle);
                Err(AudioError::Backend(
                    "timed out opening the WASAPI stream".into(),
                ))
            }
        }
    }

    struct WindowsStreamControl {
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
        running: bool,
        diagnostics: Arc<StreamDiagnostics>,
    }

    impl StreamControl for WindowsStreamControl {
        fn stop(&mut self) {
            if !self.running {
                return;
            }
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            self.running = false;
        }

        fn is_running(&self) -> bool {
            self.running && self.diagnostics.state() == crate::diagnostics::StreamState::Running
        }

        fn diagnostics(&self) -> &Arc<StreamDiagnostics> {
            &self.diagnostics
        }
    }

    struct Running {
        client: AudioClient,
        render: AudioRenderClient,
        event: Handle,
        format: StreamFormat,
        callback: Box<dyn AudioCallback>,
        block_align: usize,
        buffer_frames: u32,
        sample_format: SampleFormat,
        f32_scratch: Vec<f32>,
        byte_scratch: Vec<u8>,
    }

    fn open_inner(
        config: &StreamConfig,
        callback: Box<dyn AudioCallback>,
        stop: &AtomicBool,
        diagnostics: &StreamDiagnostics,
    ) -> Result<Running, AudioError> {
        let _ = stop;
        crate::devices::ensure_com();
        let enumerator = DeviceEnumerator::new()?;
        let device = match &config.device_id {
            Some(id) => enumerator
                .get_device(id)
                .map_err(|_| AudioError::DeviceNotFound(id.clone()))?,
            None => enumerator
                .get_default_device(Direction::Render)
                .map_err(|_| AudioError::NoDevice)?,
        };
        let device_id = device.get_id().unwrap_or_default();
        let device_name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "<unnamed audio device>".into());

        let (client, mut format) = match config.share_mode {
            ShareMode::Exclusive => match open_exclusive(&device, config) {
                Ok(opened) => opened,
                Err(error) if config.fallback_to_shared => {
                    let (client, mut shared_format) = open_shared(&device, config)?;
                    shared_format.fell_back = true;
                    shared_format.fallback_reason = Some(error.to_string());
                    diagnostics.set_error(format!(
                        "exclusive mode unavailable ({}); using shared mode",
                        error
                    ));
                    diagnostics.clear_error();
                    (client, shared_format)
                }
                Err(error) => return Err(error),
            },
            ShareMode::Shared => open_shared(&device, config)?,
        };
        format.device_id = device_id;
        format.device_name = device_name;
        finish(client, format, callback)
    }

    fn open_shared(
        device: &Device,
        config: &StreamConfig,
    ) -> Result<(AudioClient, StreamFormat), AudioError> {
        let mut client = device.get_iaudioclient()?;
        let channel_count = config.channels.max(1) as usize;
        let requested = WaveFormat::new(
            32,
            32,
            SampleType::Float,
            config.sample_rate as usize,
            channel_count,
            None,
        );
        let mix = client.get_mixformat().ok();
        let usable_low_latency = mix
            .as_ref()
            .map(|mix| {
                mix.get_samplespersec() == config.sample_rate
                    && mix.get_nchannels() == config.channels
                    && mix.get_subformat().ok() == Some(SampleType::Float)
            })
            .unwrap_or(false);

        if config.low_latency_shared && usable_low_latency {
            if let Some(mix) = mix.as_ref() {
                if let Ok(periods) = client.get_shared_mode_engine_period(mix) {
                    let want = align_period(config.buffer_frames, &periods);
                    let in_range = want >= periods.min && want <= periods.max;
                    if in_range
                        && client
                            .initialize_shared_audio_stream(mix, TimingMode::Events, want)
                            .is_ok()
                    {
                        let actual = client
                            .get_current_shared_mode_engine_period()
                            .map(|(_, period)| period)
                            .unwrap_or(want);
                        let buffer = client.get_buffer_size()?;
                        let latency = client.get_stream_latency().ok().map(hns_to_ms);
                        let format = StreamFormat {
                            device_id: String::new(),
                            device_name: String::new(),
                            sample_rate: config.sample_rate,
                            channels: config.channels,
                            sample_format: SampleFormat::F32,
                            share_mode: ShareMode::Shared,
                            buffer_frames: buffer,
                            callback_period_frames: actual,
                            stream_latency_ms: latency.filter(|ms| *ms > 0.0),
                            fell_back: false,
                            fallback_reason: None,
                        };
                        return Ok((client, format));
                    }
                    // Recreate the client after a failed initialization.
                    client = device.get_iaudioclient()?;
                }
            }
        }

        let (default_period, _minimum) = client.get_device_period()?;
        let requested_hns = frames_to_hns(config.buffer_frames, config.sample_rate);
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: requested_hns.max(default_period),
        };
        let device_name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "<unnamed audio device>".into());
        client
            .initialize_client(&requested, Direction::Render, mode, None)
            .map_err(|error| AudioError::UnsupportedFormat {
                device: device_name,
                request: format!("{} ({error})", config.format_request()),
            })?;
        let buffer = client.get_buffer_size()?;
        let callback_period = client
            .get_shared_mode_engine_period(&requested)
            .map(|periods| periods.default)
            .unwrap_or_else(|_| {
                (hns_to_ms(default_period) as f64 * config.sample_rate as f64 / 1000.0).round()
                    as u32
            });
        let latency = client.get_stream_latency().ok().map(hns_to_ms);
        let format = StreamFormat {
            device_id: String::new(),
            device_name: String::new(),
            sample_rate: config.sample_rate,
            channels: config.channels,
            sample_format: SampleFormat::F32,
            share_mode: ShareMode::Shared,
            buffer_frames: buffer,
            callback_period_frames: callback_period.max(1),
            stream_latency_ms: latency.filter(|ms| *ms > 0.0),
            fell_back: false,
            fallback_reason: None,
        };
        Ok((client, format))
    }

    fn open_exclusive(
        device: &Device,
        config: &StreamConfig,
    ) -> Result<(AudioClient, StreamFormat), AudioError> {
        let rate = config.sample_rate.max(1);
        let channels = config.channels.max(1) as usize;
        let preferred_bits = if config.exclusive_bits == 24 { 24 } else { 16 };
        let mut last_error = None;
        for bits in [preferred_bits, if preferred_bits == 16 { 24 } else { 16 }] {
            let requested = WaveFormat::new(
                bits as usize,
                bits as usize,
                SampleType::Int,
                rate as usize,
                channels,
                None,
            );
            let mut client = device.get_iaudioclient()?;
            let supported = match client.is_supported_exclusive_with_quirks(&requested) {
                Ok(supported) => supported,
                Err(error) => {
                    last_error = Some(error.to_string());
                    continue;
                }
            };
            let (_default_period, min_period) = client.get_device_period()?;
            let want = frames_to_hns(config.buffer_frames, rate);
            let mut period = client
                .calculate_aligned_period_near(want, Some(128), &supported)
                .unwrap_or(want)
                .max(min_period);
            let mut init = client.initialize_client(
                &supported,
                Direction::Render,
                StreamMode::EventsExclusive { period_hns: period },
                None,
            );
            if init.is_err() {
                // AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED recovery: take the
                // aligned buffer size, convert it to a period, retry once
                // with a fresh client.
                if let Ok(aligned_frames) = client.get_buffer_size() {
                    period = calculate_period_100ns(aligned_frames as i64, rate as i64);
                    client = device.get_iaudioclient()?;
                    init = client.initialize_client(
                        &supported,
                        Direction::Render,
                        StreamMode::EventsExclusive { period_hns: period },
                        None,
                    );
                }
            }
            if let Err(error) = init {
                last_error = Some(error.to_string());
                continue;
            }
            let buffer = client.get_buffer_size()?;
            let latency = client.get_stream_latency().ok().map(hns_to_ms);
            let format = StreamFormat {
                device_id: String::new(),
                device_name: String::new(),
                sample_rate: rate,
                channels: config.channels,
                sample_format: if bits == 24 {
                    SampleFormat::I24
                } else {
                    SampleFormat::I16
                },
                share_mode: ShareMode::Exclusive,
                buffer_frames: buffer,
                callback_period_frames: buffer.max(1),
                stream_latency_ms: latency.filter(|ms| *ms > 0.0),
                fell_back: false,
                fallback_reason: None,
            };
            return Ok((client, format));
        }
        Err(AudioError::UnsupportedFormat {
            device: device
                .get_friendlyname()
                .unwrap_or_else(|_| "<unnamed audio device>".into()),
            request: format!(
                "{} ({})",
                config.format_request(),
                last_error.unwrap_or_else(|| "no integer format accepted".into())
            ),
        })
    }

    fn finish(
        client: AudioClient,
        format: StreamFormat,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Running, AudioError> {
        let event = client.set_get_eventhandle()?;
        let render = client.get_audiorenderclient()?;
        let buffer_frames = client.get_buffer_size()?;
        let block_align = format.channels as usize * format.sample_format.bytes_per_sample();
        let sample_format = format.sample_format;
        let f32_scratch = vec![0.0f32; buffer_frames as usize * format.channels as usize];
        let byte_scratch = vec![0u8; buffer_frames as usize * block_align];
        Ok(Running {
            client,
            render,
            event,
            format,
            callback,
            block_align,
            buffer_frames,
            sample_format,
            f32_scratch,
            byte_scratch,
        })
    }

    fn run_loop(running: &mut Running, stop: &AtomicBool, diagnostics: &StreamDiagnostics) {
        diagnostics.set_format(
            running.format.sample_rate,
            running.buffer_frames,
            running.format.callback_period_frames,
        );
        diagnostics.set_stream_latency(running.format.stream_latency_ms);

        // Prime the device with silence so start-up does not pop.
        if let Ok(space) = running.client.get_available_space_in_frames() {
            let bytes = space as usize * running.block_align;
            if bytes <= running.byte_scratch.len() {
                let _ = running
                    .render
                    .write_to_device(&running.byte_scratch[..bytes], None);
            }
        }
        if let Err(error) = running.client.start_stream() {
            let _ = error;
            diagnostics.mark_error();
            return;
        }
        diagnostics.set_state(crate::diagnostics::StreamState::Running);

        let expected = callback_interval(
            running.format.callback_period_frames,
            running.format.sample_rate,
        );
        let mut last = Instant::now();
        let mut callbacks = 0u64;

        while !stop.load(Ordering::Relaxed) {
            match running.event.wait_for_event(100) {
                Ok(()) => {
                    let now = Instant::now();
                    let interval = now.duration_since(last);
                    last = now;
                    callbacks += 1;
                    if callbacks > 1 && callback_overrun(interval, expected) {
                        diagnostics.on_late_callback(interval);
                    }

                    let frames = match running.client.get_available_space_in_frames() {
                        Ok(0) => {
                            diagnostics.on_xrun();
                            continue;
                        }
                        Ok(frames) => frames as usize,
                        Err(error) => {
                            let _ = error;
                            diagnostics.mark_device_lost();
                            break;
                        }
                    };
                    let frames = frames.min(running.buffer_frames as usize);
                    let samples = frames * running.format.channels as usize;
                    if samples > running.f32_scratch.len() {
                        diagnostics.on_xrun();
                        continue;
                    }
                    let work = Instant::now();
                    for sample in &mut running.f32_scratch[..samples] {
                        *sample = 0.0;
                    }
                    // The callback is invoked indirectly through the boxed
                    // closure supplied by the caller.
                    if !running.invoke_callback(samples, frames) {
                        diagnostics.mark_device_lost();
                        break;
                    }
                    let bytes = frames * running.block_align;
                    convert_f32(
                        &mut running.byte_scratch[..bytes],
                        &running.f32_scratch[..samples],
                        running.sample_format,
                    );
                    if let Err(error) = running
                        .render
                        .write_to_device(&running.byte_scratch[..bytes], None)
                    {
                        let _ = error;
                        diagnostics.mark_error();
                        break;
                    }
                    diagnostics.record_frames(frames);
                    let elapsed = work.elapsed();
                    diagnostics.on_callback(elapsed, interval);
                }
                Err(WasapiError::EventTimeout) => {
                    diagnostics.on_timeout();
                }
                Err(error) => {
                    let _ = error;
                    diagnostics.mark_device_lost();
                    break;
                }
            }
        }
        let _ = running.client.stop_stream();
    }

    // The callback is stored on `Running`; the loop calls it through this
    // helper so the borrow of the scratch buffer stays field-local.
    impl Running {
        fn invoke_callback(&mut self, samples: usize, frames: usize) -> bool {
            let Running {
                callback,
                f32_scratch,
                ..
            } = self;
            callback.process(&mut f32_scratch[..samples], frames);
            true
        }
    }

    fn align_period(requested: u32, periods: &SharedModeEnginePeriod) -> u32 {
        let fundamental = periods.fundamental.max(1);
        let mut want = requested.max(periods.min).min(periods.max);
        want = ((want + fundamental / 2) / fundamental) * fundamental;
        want.clamp(periods.min, periods.max)
    }

    fn frames_to_hns(frames: u32, rate: u32) -> i64 {
        (frames as i64 * 10_000_000) / rate.max(1) as i64
    }

    fn hns_to_ms(hns: i64) -> f32 {
        hns as f32 / 10_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_conversion_is_little_endian() {
        let mut out = [0u8; 8];
        convert_f32(&mut out, &[1.0, -0.5], SampleFormat::F32);
        assert_eq!(f32::from_le_bytes(out[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(out[4..8].try_into().unwrap()), -0.5);
    }

    #[test]
    fn i16_conversion_clamps_and_scales() {
        let mut out = [0u8; 6];
        convert_f32(&mut out, &[1.0, -1.0, 2.0], SampleFormat::I16);
        assert_eq!(i16::from_le_bytes(out[0..2].try_into().unwrap()), i16::MAX);
        assert_eq!(i16::from_le_bytes(out[2..4].try_into().unwrap()), -i16::MAX);
        assert_eq!(i16::from_le_bytes(out[4..6].try_into().unwrap()), i16::MAX);
    }

    #[test]
    fn i24_conversion_writes_three_bytes() {
        let mut out = [0u8; 6];
        convert_f32(&mut out, &[1.0, -1.0], SampleFormat::I24);
        let sign_extend = |bytes: [u8; 3]| {
            let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
            if value & 0x0080_0000 != 0 {
                value | !0x00FF_FFFF
            } else {
                value
            }
        };
        assert_eq!(sign_extend([out[0], out[1], out[2]]), 8_388_607);
        assert_eq!(sign_extend([out[3], out[4], out[5]]), -8_388_607);
    }

    #[test]
    fn callback_interval_matches_frames() {
        let interval = callback_interval(128, 48_000);
        assert!((interval.as_secs_f64() - 0.002_666_7).abs() < 1e-6);
        assert!(callback_overrun(
            Duration::from_millis(5),
            Duration::from_micros(2667)
        ));
        assert!(!callback_overrun(
            Duration::from_micros(2700),
            Duration::from_micros(2667)
        ));
    }
}
