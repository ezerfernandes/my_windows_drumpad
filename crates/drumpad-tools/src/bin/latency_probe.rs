//! Offline latency probe for the software half of E11-T01.
//!
//! Measures two software paths on the machine that runs it:
//!
//! - `key_to_queue`: wall time to push a trigger onto the engine queue.
//! - `midi_to_callback`: wall time from queue push until the engine has an
//!   active voice after processing one block (the offline stand-in for a
//!   callback).
//!
//! This is **not** a substitute for the analog loopback. It excludes the OS
//! input stack, the USB keyboard scan interval, WASAPI, the DAC, and the
//! Nitro Max Aux In. It exists so the JSON baseline has at least one measured
//! row with real hardware details while the physical rows stay `pending`.
//!
//! Usage:
//!
//! ```text
//! latency_probe [--hits 1200] [--out <json>] [--markdown <md>] [--raw <csv>]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use drumpad_input::trigger::{InputEvent, PadId, TriggerEvent, TriggerSource, Velocity};
use drumpad_tools::fake::FakeAudioBackend;
use drumpad_tools::latency::{
    AudioMode, BaselineReport, BaselineRow, BaselineStatus, HardwareProfile, Measurement, Metric,
    evaluate,
};
use drumpad_tools::offline::RenderConfig;

const DEFAULT_HITS: usize = 1_200;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

struct Args {
    hits: usize,
    out: PathBuf,
    markdown: Option<PathBuf>,
    raw: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let root = workspace_root();
    let mut hits = DEFAULT_HITS;
    let mut out = root
        .join("crates")
        .join("drumpad-tools")
        .join("baselines")
        .join("latency-baseline.json");
    let mut markdown = None;
    let mut raw = root
        .join("crates")
        .join("drumpad-tools")
        .join("baselines")
        .join("raw")
        .join("offline-probe.csv");
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--hits") => {
                hits = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .ok_or_else(|| "--hits requires an integer".to_owned())?;
            }
            Some("--out") => {
                out = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--out requires a path".to_owned())?,
                );
            }
            Some("--markdown") => {
                markdown = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--markdown requires a path".to_owned())?,
                ));
            }
            Some("--raw") => {
                raw = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--raw requires a path".to_owned())?,
                );
            }
            Some("--help" | "-h") => {
                println!(
                    "usage: latency_probe [--hits N] [--out json] [--markdown md] [--raw csv]"
                );
                std::process::exit(0);
            }
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid UTF-8".to_owned()),
        }
    }
    if hits < 300 {
        return Err("--hits must be at least 300".to_owned());
    }
    Ok(Args {
        hits,
        out,
        markdown,
        raw,
    })
}

fn hardware() -> HardwareProfile {
    HardwareProfile {
        machine: "ASUSTeK ROG Strix G533QS".to_owned(),
        cpu: "AMD Ryzen 9 5900HX with Radeon Graphics".to_owned(),
        ram_gib: 64.0,
        os: "Windows 11 Pro".to_owned(),
        os_build: "10.0.26300".to_owned(),
        audio_device: "Realtek High Definition Audio (onboard)".to_owned(),
        audio_driver: "Realtek onboard driver; offline engine path only".to_owned(),
        audio_mode: AudioMode::Shared,
        keyboard: "none: offline queue probe".to_owned(),
        midi_interface: "none: offline queue probe".to_owned(),
        monitoring_path: "none: offline queue probe".to_owned(),
        operator: "MistyGrove (offline probe)".to_owned(),
        date_utc: "2026-11-07".to_owned(),
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };

    let config = RenderConfig::default();
    let mut backend = match FakeAudioBackend::new(config) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("error: cannot create backend: {error}");
            return ExitCode::from(1);
        }
    };
    let pad = PadId::new(0).expect("pad 0");
    let velocity = Velocity::new(100).expect("velocity");
    let mut key_samples = Vec::with_capacity(args.hits);
    let mut midi_samples = Vec::with_capacity(args.hits);
    for index in 0..args.hits {
        let event = InputEvent::Trigger(TriggerEvent::new(
            pad,
            velocity,
            TriggerSource::UiPreview,
            (index as u64) * 1_000,
        ));
        let key_start = Instant::now();
        if let Err(error) = backend.send_input(event) {
            eprintln!("error: queue push failed: {error}");
            return ExitCode::from(1);
        }
        key_samples.push(key_start.elapsed().as_secs_f64() * 1_000.0);

        let midi_start = Instant::now();
        backend.process_block();
        midi_samples.push(midi_start.elapsed().as_secs_f64() * 1_000.0);
        if backend.active_pad_counts()[0] == 0 {
            eprintln!("error: no active voice after a trigger block at hit {index}");
            return ExitCode::from(1);
        }
    }

    let key_measurement =
        Measurement::new(Metric::KeyToQueue, 100, key_samples.clone()).expect("key measurement");
    let midi_measurement = Measurement::new(Metric::MidiToCallback, 100, midi_samples.clone())
        .expect("midi measurement");
    let key_verdict = match evaluate(
        &key_measurement,
        config.sample_rate,
        config.block_frames as u32,
    ) {
        Ok(verdict) => verdict,
        Err(error) => {
            eprintln!("error: key verdict: {error}");
            return ExitCode::from(1);
        }
    };
    let midi_verdict = match evaluate(
        &midi_measurement,
        config.sample_rate,
        config.block_frames as u32,
    ) {
        Ok(verdict) => verdict,
        Err(error) => {
            eprintln!("error: midi verdict: {error}");
            return ExitCode::from(1);
        }
    };

    // Raw capture: one line per hit, no summary-only artifact.
    if let Some(parent) = args.raw.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("error: cannot create {}: {error}", parent.display());
        return ExitCode::from(1);
    }
    let mut raw = String::from("index,key_to_queue_ms,midi_to_callback_ms\n");
    for index in 0..args.hits {
        raw.push_str(&format!(
            "{index},{:.9},{:.9}\n",
            key_samples[index], midi_samples[index]
        ));
    }
    if let Err(error) = std::fs::write(&args.raw, raw) {
        eprintln!("error: cannot write {}: {error}", args.raw.display());
        return ExitCode::from(1);
    }

    let measured_row = BaselineRow {
        status: BaselineStatus::Measured,
        hardware: hardware(),
        sample_rate: config.sample_rate,
        block_frames: config.block_frames as u32,
        verdicts: vec![key_verdict, midi_verdict],
        raw_capture: args
            .raw
            .strip_prefix(workspace_root())
            .unwrap_or(&args.raw)
            .display()
            .to_string(),
        notes: "Offline engine-path probe only: excludes OS input, USB scan, WASAPI, DAC, and the Nitro Max Aux In. Physical rows are pending.".to_owned(),
    };

    let mut report = BaselineReport::new();
    report.rows.push(measured_row);
    let mut pending_48 = BaselineRow::reference_pending_row();
    pending_48.notes =
        "Required reference row: Finding 35-key keyboard + Nitro Max USB-MIDI + onboard 3.5 mm -> Aux In. Not measured."
            .to_owned();
    let mut pending_441 = BaselineRow::reference_pending_row();
    pending_441.sample_rate = 44_100;
    pending_441.notes = "Required format row at 44.1 kHz. Not measured.".to_owned();
    let mut pending_96 = BaselineRow::reference_pending_row();
    pending_96.sample_rate = 96_000;
    pending_96.notes = "Required format row at 96 kHz. Not measured.".to_owned();
    report.rows.push(pending_48);
    report.rows.push(pending_441);
    report.rows.push(pending_96);
    report.notes = "Generated by `cargo run -p drumpad-tools --bin latency_probe`. \
        The first row is a real offline measurement on the named machine; the reference rows \
        require the physical loopback in docs/qa/latency-protocol.md and are not evidence until measured."
        .to_owned();

    if let Some(parent) = args.out.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("error: cannot create {}: {error}", parent.display());
        return ExitCode::from(1);
    }
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("error: cannot serialize baseline: {error}");
            return ExitCode::from(1);
        }
    };
    if let Err(error) = std::fs::write(&args.out, format!("{json}\n")) {
        eprintln!("error: cannot write {}: {error}", args.out.display());
        return ExitCode::from(1);
    }

    println!(
        "offline probe: {} hits, key_to_queue p99 {:.4} ms ({}), midi_to_callback p99 {:.4} ms ({})",
        args.hits,
        key_measurement
            .stats()
            .map(|stats| stats.p99_ms)
            .unwrap_or(0.0),
        if report.rows[0].verdicts[0].pass {
            "pass"
        } else {
            "FAIL"
        },
        midi_measurement
            .stats()
            .map(|stats| stats.p99_ms)
            .unwrap_or(0.0),
        if report.rows[0].verdicts[1].pass {
            "pass"
        } else {
            "FAIL"
        },
    );
    println!("wrote {} and {}", args.out.display(), args.raw.display());

    if let Some(path) = args.markdown {
        if let Err(error) = std::fs::write(&path, report.to_markdown()) {
            eprintln!("error: cannot write {}: {error}", path.display());
            return ExitCode::from(1);
        }
        println!("wrote {}", path.display());
    }

    if report.rows[0].verdicts.iter().all(|verdict| verdict.pass) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}
