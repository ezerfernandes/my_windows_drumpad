//! Deterministic offline renderer.
//!
//! ```text
//! offline_render <fixture> <out.wav> [--sequence <file>] [--config <json>] [--report <json>]
//! ```
//!
//! `<fixture>` is `standard`, one of the named fixtures (`one_shot`, `gate`,
//! `loop`, `choke`, `pitch`, `hot`), or a path to a WAV file. The sequence
//! file defaults to `tests/fixtures/sequences/basic.seq`. Rendering never
//! touches an audio device.
//!
//! Exit codes: 0 success, 1 argument/IO error, 2 bad fixture or sequence,
//! 3 render error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use drumpad_engine::voice::SampleBank;
use drumpad_tools::fixture::{self, FixtureBank, PadFixture};
use drumpad_tools::offline::{OfflineSession, RenderConfig, RenderError};
use drumpad_tools::sequence::{Sequence, SequenceError};
use serde::Serialize;

struct Args {
    fixture: String,
    out: PathBuf,
    sequence: PathBuf,
    config: Option<PathBuf>,
    report: Option<PathBuf>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

fn default_sequence() -> PathBuf {
    workspace_root()
        .join("tests")
        .join("fixtures")
        .join("sequences")
        .join("basic.seq")
}

fn usage() -> &'static str {
    "usage: offline_render <fixture|wav> <out.wav> [--sequence <file>] [--config <json>] [--report <json>]"
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut sequence = None;
    let mut config = None;
    let mut report = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--sequence") => {
                sequence = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--sequence requires a path".to_owned())?,
                ));
            }
            Some("--config") => {
                config = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--config requires a path".to_owned())?,
                ));
            }
            Some("--report") => {
                report = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--report requires a path".to_owned())?,
                ));
            }
            Some("--help" | "-h") => {
                println!("{}", usage());
                std::process::exit(0);
            }
            Some(other) if other.starts_with("--") => {
                return Err(format!("unknown option {other}; {}", usage()));
            }
            Some(_) | None => positional.push(PathBuf::from(arg)),
        }
    }
    if positional.len() != 2 {
        return Err(usage().to_owned());
    }
    let out = positional.pop().expect("checked");
    let fixture = positional
        .pop()
        .expect("checked")
        .to_string_lossy()
        .into_owned();
    Ok(Args {
        fixture,
        out,
        sequence: sequence.unwrap_or_else(default_sequence),
        config,
        report,
    })
}

#[derive(Serialize)]
struct CliReport {
    fixture: String,
    sequence: String,
    out: String,
    config: RenderConfig,
    frames: u64,
    stats: drumpad_tools::offline::RenderStats,
    trigger_count: Vec<u32>,
}

fn build_bank(spec: &str) -> Result<FixtureBank, String> {
    if spec == "standard" {
        return Ok(FixtureBank::standard());
    }
    if let Some(bank) = FixtureBank::single(spec) {
        return Ok(bank);
    }
    let path = Path::new(spec);
    if path.extension().and_then(|ext| ext.to_str()) == Some("wav") {
        let sample = fixture::load_wav(path).map_err(|error| error.to_string())?;
        let mut bank = SampleBank::new();
        let slot = bank.push(Arc::new(sample));
        return Ok(FixtureBank {
            bank,
            pads: [PadFixture::one_shot(slot); 16],
        });
    }
    Err(format!(
        "unknown fixture {spec:?}; expected standard, one of {}, or a .wav path",
        fixture::FIXTURE_NAMES.join(", ")
    ))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };

    let config = match &args.config {
        None => RenderConfig::default(),
        Some(path) => {
            let json = match std::fs::read_to_string(path) {
                Ok(json) => json,
                Err(error) => {
                    eprintln!("error: cannot read {}: {error}", path.display());
                    return ExitCode::from(1);
                }
            };
            match serde_json::from_str::<RenderConfig>(&json) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("error: invalid config JSON: {error}");
                    return ExitCode::from(1);
                }
            }
        }
    };

    if let Err(error) = config.validate() {
        eprintln!("error: {error}");
        return ExitCode::from(1);
    }

    let sequence = match Sequence::load(&args.sequence) {
        Ok(sequence) => sequence,
        Err(error) => {
            report_sequence_error(&error);
            return ExitCode::from(2);
        }
    };

    let bank = match build_bank(&args.fixture) {
        Ok(bank) => bank,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    let mut session = match OfflineSession::with_bank(config, bank) {
        Ok(session) => session,
        Err(error) => {
            report_render_error(&error);
            return ExitCode::from(3);
        }
    };
    let rendered = match session.render(&sequence) {
        Ok(rendered) => rendered,
        Err(error) => {
            report_render_error(&error);
            return ExitCode::from(3);
        }
    };
    if let Err(error) = rendered.write_wav(&args.out) {
        report_render_error(&error);
        return ExitCode::from(3);
    }

    let report = CliReport {
        fixture: args.fixture.clone(),
        sequence: args.sequence.display().to_string(),
        out: args.out.display().to_string(),
        config: rendered.config,
        frames: rendered.frames,
        stats: rendered.stats.clone(),
        trigger_count: rendered.trigger_count.to_vec(),
    };
    let json = match serde_json::to_string_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("error: cannot serialize report: {error}");
            return ExitCode::from(3);
        }
    };
    match args.report {
        Some(path) => {
            if let Err(error) = std::fs::write(&path, format!("{json}\n")) {
                eprintln!("error: cannot write {}: {error}", path.display());
                return ExitCode::from(1);
            }
            println!(
                "rendered {} frames to {} (report {})",
                report.frames,
                report.out,
                path.display()
            );
        }
        None => println!("{json}"),
    }
    ExitCode::SUCCESS
}

fn report_sequence_error(error: &SequenceError) {
    eprintln!("error: {error}");
}

fn report_render_error(error: &RenderError) {
    eprintln!("error: {error}");
}
