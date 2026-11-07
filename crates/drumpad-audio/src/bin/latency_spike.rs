//! WASAPI latency spike for E02-T01.
//!
//! Measures shared (default and low-latency request) and exclusive modes at
//! 48 kHz with 64, 128 and 256-frame requests on the selected render device.
//! Results are printed as a table and written as JSON for `docs/audio/`.
//!
//! ```text
//! cargo run -p drumpad-audio --bin latency-spike -- --seconds 8 \
//!   --out docs/audio/measurements/wasapi-spike-<machine>.json
//! ```
//!
//! Environment: `WASAPI_SPIKE_DEVICE=<substring>` selects a render device
//! instead of the default; `WASAPI_SPIKE_CONFIGS=shared:128,exclusive:256`
//! restricts the matrix.

use serde::Serialize;
use std::time::{Duration, Instant};
use wasapi::*;

#[derive(Serialize, Clone, Debug)]
struct Config {
    mode: String,
    requested_frames: u32,
    sample_rate: u32,
    channels: u16,
    sample_bits: u16,
    sample_type: String,
}

#[derive(Serialize, Clone, Debug, Default)]
struct RunResult {
    config: Option<Config>,
    ok: bool,
    error: Option<String>,
    buffer_frames: Option<u32>,
    callback_period_frames: Option<f64>,
    stream_latency_ms: Option<f64>,
    engine_period_default: Option<u32>,
    engine_period_min: Option<u32>,
    engine_period_max: Option<u32>,
    callbacks: u64,
    late_callbacks: u64,
    timeouts: u64,
    interval_ms_mean: f64,
    interval_ms_p50: f64,
    interval_ms_p95: f64,
    interval_ms_p99: f64,
    interval_ms_max: f64,
    cpu_load_percent: f64,
    audio_wall_seconds: f64,
    estimated_block_ms: Option<f64>,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn hns_to_ms(hns: i64) -> f64 {
    hns as f64 / 10_000.0
}

fn frames_to_hns(frames: u32, rate: u32) -> i64 {
    (frames as i64 * 10_000_000) / rate.max(1) as i64
}

fn make_format(config: &Config) -> WaveFormat {
    WaveFormat::new(
        config.sample_bits as usize,
        config.sample_bits as usize,
        match config.sample_type.as_str() {
            "float" => SampleType::Float,
            _ => SampleType::Int,
        },
        config.sample_rate as usize,
        config.channels as usize,
        None,
    )
}

fn run(device: &Device, config: &Config, seconds: f64) -> RunResult {
    // Match the production stream thread: without MMCSS, exclusive-mode
    // callbacks are preempted by normal-priority work on this machine.
    let _mmcss = drumpad_audio::boost_audio_thread();
    let mut result = RunResult {
        config: Some(config.clone()),
        ..Default::default()
    };

    let mut client = match device.get_iaudioclient() {
        Ok(client) => client,
        Err(error) => {
            result.error = Some(format!("get_iaudioclient: {error}"));
            return result;
        }
    };
    let requested_format = make_format(config);
    let (default_period, min_period) = match client.get_device_period() {
        Ok(period) => period,
        Err(error) => {
            result.error = Some(format!("get_device_period: {error}"));
            return result;
        }
    };
    result.callback_period_frames =
        Some(hns_to_ms(default_period) * config.sample_rate as f64 / 1000.0);

    let format;
    match config.mode.as_str() {
        "shared-lowlatency" => {
            let mix = match client.get_mixformat() {
                Ok(mix) => mix,
                Err(error) => {
                    result.error = Some(format!("get_mixformat: {error}"));
                    return result;
                }
            };
            match client.get_shared_mode_engine_period(&mix) {
                Ok(periods) => {
                    result.engine_period_default = Some(periods.default);
                    result.engine_period_min = Some(periods.min);
                    result.engine_period_max = Some(periods.max);
                    let fundamental = periods.fundamental.max(1);
                    let mut want = config.requested_frames.max(periods.min).min(periods.max);
                    want = ((want + fundamental / 2) / fundamental) * fundamental;
                    want = want.clamp(periods.min, periods.max);
                    if let Err(error) =
                        client.initialize_shared_audio_stream(&mix, TimingMode::Events, want)
                    {
                        result.error =
                            Some(format!("initialize_shared_audio_stream({want}): {error}"));
                        return result;
                    }
                    result.callback_period_frames = Some(want as f64);
                    format = mix;
                }
                Err(error) => {
                    result.error = Some(format!("get_shared_mode_engine_period: {error}"));
                    return result;
                }
            }
        }
        "shared" => {
            let mix = match client.get_mixformat() {
                Ok(mix) => mix,
                Err(error) => {
                    result.error = Some(format!("get_mixformat: {error}"));
                    return result;
                }
            };
            let requested_us = frames_to_hns(config.requested_frames, config.sample_rate);
            let mode = StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: requested_us.max(default_period),
            };
            if let Err(error) = client.initialize_client(&mix, Direction::Render, mode, None) {
                result.error = Some(format!("initialize_client(shared): {error}"));
                return result;
            }
            format = mix;
        }
        _ => {
            let supported = match client.is_supported_exclusive_with_quirks(&requested_format) {
                Ok(format) => format,
                Err(error) => {
                    result.error = Some(format!("is_supported_exclusive_with_quirks: {error}"));
                    return result;
                }
            };
            let requested_us = frames_to_hns(config.requested_frames, config.sample_rate);
            let mut period =
                match client.calculate_aligned_period_near(requested_us, Some(128), &supported) {
                    Ok(period) => period.max(min_period),
                    Err(error) => {
                        result.error = Some(format!("calculate_aligned_period_near: {error}"));
                        return result;
                    }
                };
            let mut init = client.initialize_client(
                &supported,
                Direction::Render,
                StreamMode::EventsExclusive { period_hns: period },
                None,
            );
            if init.is_err() {
                // AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED recovery.
                if let Ok(aligned_frames) = client.get_buffer_size() {
                    period = calculate_period_100ns(
                        aligned_frames as i64,
                        supported.get_samplespersec() as i64,
                    );
                    match device.get_iaudioclient() {
                        Ok(fresh) => {
                            client = fresh;
                            init = client.initialize_client(
                                &supported,
                                Direction::Render,
                                StreamMode::EventsExclusive { period_hns: period },
                                None,
                            );
                        }
                        Err(error) => {
                            result.error = Some(format!("recreate client: {error}"));
                            return result;
                        }
                    }
                }
            }
            if let Err(error) = init {
                result.error = Some(format!(
                    "initialize_client(exclusive, {period}hns): {error}"
                ));
                return result;
            }
            result.callback_period_frames =
                Some(config.sample_rate as f64 * period as f64 / 10_000_000.0);
            format = supported;
        }
    }

    result.stream_latency_ms = client.get_stream_latency().ok().map(hns_to_ms);
    let buffer_frames = match client.get_buffer_size() {
        Ok(frames) => frames,
        Err(error) => {
            result.error = Some(format!("get_buffer_size: {error}"));
            return result;
        }
    };
    result.buffer_frames = Some(buffer_frames);
    result.estimated_block_ms = Some(buffer_frames as f64 / config.sample_rate as f64 * 1000.0);

    let event = match client.set_get_eventhandle() {
        Ok(event) => event,
        Err(error) => {
            result.error = Some(format!("set_get_eventhandle: {error}"));
            return result;
        }
    };
    let render = match client.get_audiorenderclient() {
        Ok(render) => render,
        Err(error) => {
            result.error = Some(format!("get_audiorenderclient: {error}"));
            return result;
        }
    };

    let block_align = format.get_blockalign() as usize;
    // Fixed preallocated scratch buffer of silence, reused every callback.
    let scratch = vec![0u8; buffer_frames as usize * block_align];
    if let Ok(space) = client.get_available_space_in_frames() {
        let bytes = space as usize * block_align;
        if let Err(error) = render.write_to_device(&scratch[..bytes.min(scratch.len())], None) {
            result.error = Some(format!("prime write: {error}"));
            return result;
        }
    }

    if let Err(error) = client.start_stream() {
        result.error = Some(format!("start_stream: {error}"));
        return result;
    }

    let expected_ms = result
        .callback_period_frames
        .unwrap_or(buffer_frames as f64)
        / config.sample_rate as f64
        * 1000.0;
    let start = Instant::now();
    let mut last = start;
    let mut intervals = Vec::new();
    let mut callback_time = Duration::ZERO;
    let mut callbacks = 0u64;
    let mut late = 0u64;
    let mut timeouts = 0u64;

    while start.elapsed() < Duration::from_secs_f64(seconds) {
        if event.wait_for_event(1000).is_err() {
            timeouts += 1;
            if timeouts > 5 {
                break;
            }
            continue;
        }
        let now = Instant::now();
        let interval = now.duration_since(last).as_secs_f64() * 1000.0;
        last = now;
        callbacks += 1;
        if callbacks > 2 {
            intervals.push(interval);
            if interval > expected_ms * 1.5 {
                late += 1;
            }
        }

        let work_start = Instant::now();
        let space = match client.get_available_space_in_frames() {
            Ok(space) => space as usize,
            Err(_) => 0,
        };
        if space > 0 {
            let bytes = space * block_align;
            if render
                .write_to_device(&scratch[..bytes.min(scratch.len())], None)
                .is_err()
            {
                break;
            }
        }
        callback_time += work_start.elapsed();
    }

    let wall = start.elapsed().as_secs_f64();
    let _ = client.stop_stream();

    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    result.ok = true;
    result.callbacks = callbacks;
    result.late_callbacks = late;
    result.timeouts = timeouts;
    if !intervals.is_empty() {
        result.interval_ms_mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
        result.interval_ms_p50 = percentile(&intervals, 0.50);
        result.interval_ms_p95 = percentile(&intervals, 0.95);
        result.interval_ms_p99 = percentile(&intervals, 0.99);
        result.interval_ms_max = *intervals.last().unwrap();
    }
    result.cpu_load_percent = if wall > 0.0 {
        callback_time.as_secs_f64() / wall * 100.0
    } else {
        0.0
    };
    result.audio_wall_seconds = wall;
    result
}

fn list_devices() {
    let enumerator = DeviceEnumerator::new().unwrap();
    let collection = enumerator.get_device_collection(Direction::Render).unwrap();
    let count = collection.get_nbr_devices().unwrap_or(0);
    println!("Render devices: {count}");
    for index in 0..count {
        if let Ok(device) = collection.get_device_at_index(index) {
            let name = device
                .get_friendlyname()
                .unwrap_or_else(|_| "<unknown>".into());
            let id = device.get_id().unwrap_or_else(|_| "<no id>".into());
            let state = device
                .get_state()
                .map(|state| state.to_string())
                .unwrap_or_else(|_| "<unknown>".into());
            println!("  [{index}] {name}\n      id={id}\n      state={state}");
        }
    }
}

fn find_device(enumerator: &DeviceEnumerator, wanted: &str) -> Device {
    let collection = enumerator.get_device_collection(Direction::Render).unwrap();
    let count = collection.get_nbr_devices().unwrap_or(0);
    for index in 0..count {
        if let Ok(device) = collection.get_device_at_index(index) {
            let name = device.get_friendlyname().unwrap_or_default();
            if name.to_lowercase().contains(&wanted.to_lowercase()) {
                return device;
            }
        }
    }
    panic!("no render device matching {wanted:?}");
}

fn parse_arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn main() {
    initialize_mta().ok().unwrap();
    list_devices();

    let args: Vec<String> = std::env::args().collect();
    let seconds = parse_arg(&args, "--seconds")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(8.0);
    let output = parse_arg(&args, "--out").unwrap_or_else(|| {
        format!(
            "target/latency-spike/wasapi-spike-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        )
    });
    let wanted = std::env::var("WASAPI_SPIKE_DEVICE").unwrap_or_default();
    let enumerator = DeviceEnumerator::new().unwrap();
    let device = if wanted.is_empty() {
        enumerator.get_default_device(Direction::Render).unwrap()
    } else {
        find_device(&enumerator, &wanted)
    };
    let device_name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "<unknown>".into());
    let device_id = device.get_id().unwrap_or_default();
    println!("\nSelected render device: {device_name}\n  id={device_id}");

    let mut configs = Vec::new();
    for frames in [64u32, 128, 256] {
        configs.push(Config {
            mode: "shared-lowlatency".into(),
            requested_frames: frames,
            sample_rate: 48_000,
            channels: 2,
            sample_bits: 32,
            sample_type: "float".into(),
        });
    }
    configs.push(Config {
        mode: "shared".into(),
        requested_frames: 128,
        sample_rate: 48_000,
        channels: 2,
        sample_bits: 32,
        sample_type: "float".into(),
    });
    for frames in [64u32, 128, 256] {
        configs.push(Config {
            mode: "exclusive".into(),
            requested_frames: frames,
            sample_rate: 48_000,
            channels: 2,
            sample_bits: 16,
            sample_type: "int".into(),
        });
    }
    configs.push(Config {
        mode: "exclusive".into(),
        requested_frames: 128,
        sample_rate: 48_000,
        channels: 2,
        sample_bits: 24,
        sample_type: "int".into(),
    });

    let only = std::env::var("WASAPI_SPIKE_CONFIGS").unwrap_or_default();
    let only: Vec<&str> = only.split(',').filter(|value| !value.is_empty()).collect();

    let mut results = Vec::new();
    for config in &configs {
        if !only.is_empty() {
            let key = format!("{}:{}", config.mode, config.requested_frames);
            if !only.iter().any(|candidate| *candidate == key) {
                continue;
            }
        }
        println!("\n--- {config:?} ---");
        let result = run(&device, config, seconds);
        if let Some(error) = &result.error {
            println!("  FAILED: {error}");
        } else {
            println!(
                "  buffer={:?} frames period={:?} latency={:?} ms callbacks={} late={} timeouts={} p50={:.3} p99={:.3} max={:.3} ms cpu={:.2}%",
                result.buffer_frames,
                result
                    .callback_period_frames
                    .map(|frames| (frames * 1000.0).round() / 1000.0),
                result
                    .stream_latency_ms
                    .map(|ms| (ms * 1000.0).round() / 1000.0),
                result.callbacks,
                result.late_callbacks,
                result.timeouts,
                result.interval_ms_p50,
                result.interval_ms_p99,
                result.interval_ms_max,
                result.cpu_load_percent
            );
        }
        results.push(result);
    }

    let document = serde_json::json!({
        "tool": "drumpad-audio latency-spike",
        "phase": "E02-T01",
        "os": "Windows 11 x64",
        "machine": std::env::var("COMPUTERNAME").unwrap_or_default(),
        "device_name": device_name,
        "device_id": device_id,
        "timestamp_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "seconds_per_config": seconds,
        "sample_rate": 48_000,
        "results": results,
    });
    if let Some(parent) = std::path::Path::new(&output).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&output, serde_json::to_string_pretty(&document).unwrap()).unwrap();
    println!("\nWrote {output}");
}
