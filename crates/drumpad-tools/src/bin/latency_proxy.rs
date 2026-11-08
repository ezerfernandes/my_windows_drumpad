//! One-number keyboard-to-audio latency proxy for the autoresearch loop.
//!
//! The real metric (key press until the first audible sample) needs an analog
//! loopback and exclusive device access, which cannot run while the human's
//! app owns the output. This wrapper therefore reports one fixed, reproducible
//! proxy:
//!
//! ```text
//! proxy_e2e_ms = key_to_queue_ms + callback_period_ms + buffer_ms
//! ```
//!
//! - `key_to_queue_ms` is measured live with the same offline engine path as
//!   `latency_probe`: the real input trigger, engine queue and mixer, with a
//!   caller-driven fake audio backend (1200 hits, 100 warm-up, nearest-rank
//!   percentiles from [`drumpad_tools::latency`]).
//! - `callback_period_ms` and `buffer_ms` are the **negotiated** values for the
//!   app's default stream request, looked up in a capability table recorded
//!   from real hardware in `docs/audio/backend-spike.md`.
//!
//! The capability table fails closed. A request that was never measured on the
//! reference endpoint is an error, not a guess: that keeps the proxy identical
//! across iterations and stops a candidate from "winning" by asking for a
//! buffer the driver would silently clamp.
//!
//! The default request is read from [`drumpad_audio::AudioSettings::default`]
//! with the same rate and block pin that `MvpAudioRuntime::start` applies, so
//! changing the app default changes the proxy and nothing else does.
//!
//! Usage:
//!
//! ```text
//! latency_proxy [--hits N] [--capabilities <json>] [--json <out>] [--detail]
//! ```
//!
//! stdout is exactly one line, `proxy_e2e_p99_ms=<number>`. Components and
//! provenance go to stderr unless `--detail` is passed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use drumpad_audio::runtime::{MVP_BLOCK_FRAMES, MVP_SAMPLE_RATE};
use drumpad_audio::{AudioSettings, ShareMode};
use drumpad_input::trigger::{InputEvent, PadId, TriggerEvent, TriggerSource, Velocity};
use drumpad_tools::fake::FakeAudioBackend;
use drumpad_tools::latency::LatencyStats;
use drumpad_tools::latency::Measurement;
use drumpad_tools::latency::Metric;
use drumpad_tools::offline::RenderConfig;
use serde::{Deserialize, Serialize};

const DEFAULT_HITS: usize = 1_200;
const WARMUP_HITS: usize = 100;
const MIN_HITS: usize = 300;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// One measured configuration of the reference endpoint.
#[derive(Debug, Clone, Deserialize)]
struct ModeCapability {
    /// Requested frame counts that were actually measured on this endpoint.
    accepted_request_frames: Vec<u32>,
    /// Negotiated callback period in frames.
    callback_period_frames: u32,
    /// Negotiated device buffer in frames.
    buffer_frames: u32,
    /// Why these values are what they are.
    note: String,
}

/// The capability table: only configurations that were measured may rank.
#[derive(Debug, Clone, Deserialize)]
struct Capabilities {
    schema_version: u32,
    device: String,
    sample_rate: u32,
    source: String,
    modes: Modes,
}

/// The two sharing modes the reference endpoint was measured in.
#[derive(Debug, Clone, Deserialize)]
struct Modes {
    shared: ModeCapability,
    exclusive: ModeCapability,
}

impl Capabilities {
    fn for_mode(&self, mode: ShareMode) -> &ModeCapability {
        match mode {
            ShareMode::Shared => &self.modes.shared,
            ShareMode::Exclusive => &self.modes.exclusive,
        }
    }
}

/// Machine-readable record of one proxy run.
#[derive(Debug, Serialize)]
struct ProxyRecord {
    schema_version: u32,
    mode: String,
    requested_frames: u32,
    callback_period_frames: u32,
    buffer_frames: u32,
    sample_rate: u32,
    hits: usize,
    key_to_queue_p50_ms: f64,
    key_to_queue_p99_ms: f64,
    midi_to_callback_p99_ms: f64,
    proxy_e2e_p50_ms: f64,
    proxy_e2e_p99_ms: f64,
    device: String,
    capability_source: String,
    negotiation_note: String,
}

struct Args {
    hits: usize,
    capabilities: PathBuf,
    json: Option<PathBuf>,
    detail: bool,
}

fn parse_args() -> Result<Args, String> {
    let root = workspace_root();
    let mut hits = DEFAULT_HITS;
    let mut capabilities = root
        .join("docs")
        .join("qa")
        .join("latency-autoresearch")
        .join("device-capabilities.json");
    let mut json = None;
    let mut detail = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--hits") => {
                hits = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .ok_or_else(|| "--hits requires an integer".to_owned())?;
            }
            Some("--capabilities") => {
                capabilities = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--capabilities requires a path".to_owned())?,
                );
            }
            Some("--json") => {
                json = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--json requires a path".to_owned())?,
                ));
            }
            Some("--detail") => detail = true,
            Some("--help" | "-h") => {
                println!(
                    "usage: latency_proxy [--hits N] [--capabilities json] [--json out] [--detail]"
                );
                std::process::exit(0);
            }
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid UTF-8".to_owned()),
        }
    }
    if hits < MIN_HITS {
        return Err(format!("--hits must be at least {MIN_HITS}"));
    }
    Ok(Args {
        hits,
        capabilities,
        json,
        detail,
    })
}

fn load_capabilities(path: &Path) -> Result<Capabilities, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let caps: Capabilities = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a capability table: {error}", path.display()))?;
    if caps.schema_version != 1 {
        return Err(format!(
            "capability table schema {} is not supported",
            caps.schema_version
        ));
    }
    Ok(caps)
}

/// The stream request the MVP runtime makes when it starts without a settings
/// file: the persisted defaults, pinned to the engine rate and block size.
fn default_request() -> AudioSettings {
    AudioSettings {
        sample_rate: MVP_SAMPLE_RATE,
        buffer_frames: MVP_BLOCK_FRAMES as u32,
        ..AudioSettings::default()
    }
}

fn measure(hits: usize) -> Result<(LatencyStats, LatencyStats), String> {
    let config = RenderConfig::default();
    let mut backend =
        FakeAudioBackend::new(config).map_err(|error| format!("cannot create backend: {error}"))?;
    let pad = PadId::new(0).expect("pad 0");
    let velocity = Velocity::new(100).expect("velocity");
    let mut key_samples = Vec::with_capacity(hits);
    let mut midi_samples = Vec::with_capacity(hits);
    for index in 0..hits {
        let event = InputEvent::Trigger(TriggerEvent::new(
            pad,
            velocity,
            TriggerSource::UiPreview,
            (index as u64) * 1_000,
        ));
        let key_start = Instant::now();
        backend
            .send_input(event)
            .map_err(|error| format!("queue push failed: {error}"))?;
        key_samples.push(key_start.elapsed().as_secs_f64() * 1_000.0);

        let midi_start = Instant::now();
        backend.process_block();
        midi_samples.push(midi_start.elapsed().as_secs_f64() * 1_000.0);
        if backend.active_pad_counts()[0] == 0 {
            return Err(format!(
                "no active voice after a trigger block at hit {index}"
            ));
        }
    }
    let key = Measurement::new(Metric::KeyToQueue, WARMUP_HITS, key_samples)
        .map_err(|error| format!("key measurement: {error}"))?
        .stats()
        .map_err(|error| format!("key statistics: {error}"))?;
    let midi = Measurement::new(Metric::MidiToCallback, WARMUP_HITS, midi_samples)
        .map_err(|error| format!("midi measurement: {error}"))?
        .stats()
        .map_err(|error| format!("midi statistics: {error}"))?;
    Ok((key, midi))
}

fn run(args: &Args) -> Result<ProxyRecord, String> {
    let caps = load_capabilities(&args.capabilities)?;
    let request = default_request();
    if request.sample_rate != caps.sample_rate {
        return Err(format!(
            "app requests {} Hz but the capability table only covers {} Hz",
            request.sample_rate, caps.sample_rate
        ));
    }
    let capability = caps.for_mode(request.share_mode);
    if !capability
        .accepted_request_frames
        .contains(&request.buffer_frames)
    {
        return Err(format!(
            "app requests {} frames in {} mode, which was never measured on {}; measured requests: {:?}",
            request.buffer_frames,
            request.share_mode,
            caps.device,
            capability.accepted_request_frames
        ));
    }

    let (key, midi) = measure(args.hits)?;
    let rate = caps.sample_rate as f64;
    let period_ms = capability.callback_period_frames as f64 * 1_000.0 / rate;
    let buffer_ms = capability.buffer_frames as f64 * 1_000.0 / rate;
    let proxy_p50 = key.p50_ms + period_ms + buffer_ms;
    let proxy_p99 = key.p99_ms + period_ms + buffer_ms;

    Ok(ProxyRecord {
        schema_version: 1,
        mode: request.share_mode.to_string(),
        requested_frames: request.buffer_frames,
        callback_period_frames: capability.callback_period_frames,
        buffer_frames: capability.buffer_frames,
        sample_rate: caps.sample_rate,
        hits: args.hits - WARMUP_HITS,
        key_to_queue_p50_ms: key.p50_ms,
        key_to_queue_p99_ms: key.p99_ms,
        midi_to_callback_p99_ms: midi.p99_ms,
        proxy_e2e_p50_ms: proxy_p50,
        proxy_e2e_p99_ms: proxy_p99,
        device: caps.device.clone(),
        capability_source: caps.source.clone(),
        negotiation_note: capability.note.clone(),
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let record = match run(&args) {
        Ok(record) => record,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };

    if args.detail {
        println!(
            "mode={} requested_frames={}",
            record.mode, record.requested_frames
        );
        println!(
            "negotiated: callback_period_frames={} buffer_frames={} rate={} device={}",
            record.callback_period_frames, record.buffer_frames, record.sample_rate, record.device
        );
        println!(
            "key_to_queue: p50={:.4} ms p99={:.4} ms",
            record.key_to_queue_p50_ms, record.key_to_queue_p99_ms
        );
        println!(
            "midi_to_callback: p99={:.4} ms",
            record.midi_to_callback_p99_ms
        );
        println!(
            "audio period+buffer: {:.4} ms + {:.4} ms",
            record.callback_period_frames as f64 * 1_000.0 / record.sample_rate as f64,
            record.buffer_frames as f64 * 1_000.0 / record.sample_rate as f64
        );
        println!(
            "proxy_e2e: p50={:.4} ms p99={:.4} ms",
            record.proxy_e2e_p50_ms, record.proxy_e2e_p99_ms
        );
        println!("negotiation: {}", record.negotiation_note);
    } else {
        eprintln!(
            "mode={} requested_frames={} negotiated_period={} negotiated_buffer={} key_p99={:.4}ms",
            record.mode,
            record.requested_frames,
            record.callback_period_frames,
            record.buffer_frames,
            record.key_to_queue_p99_ms
        );
    }
    println!("proxy_e2e_p99_ms={:.4}", record.proxy_e2e_p99_ms);

    if let Some(path) = &args.json {
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            eprintln!("error: cannot create {}: {error}", parent.display());
            return ExitCode::from(1);
        }
        let json = match serde_json::to_string_pretty(&record) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("error: cannot serialize proxy record: {error}");
                return ExitCode::from(1);
            }
        };
        if let Err(error) = std::fs::write(path, format!("{json}\n")) {
            eprintln!("error: cannot write {}: {error}", path.display());
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}
