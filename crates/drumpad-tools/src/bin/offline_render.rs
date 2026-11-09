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

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use drumpad_engine::voice::SampleBank;
use drumpad_tools::fixture::{self, FixtureBank, PadFixture};
use drumpad_tools::offline::{OfflineSession, RenderConfig, RenderError};
use drumpad_tools::sequence::Sequence;
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

/// Parse arguments. `Ok(None)` means `--help` was requested.
fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let mut positional = Vec::new();
    let mut sequence = None;
    let mut config = None;
    let mut report = None;
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
            Some("--help" | "-h") => return Ok(None),
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
    Ok(Some(Args {
        fixture,
        out,
        sequence: sequence.unwrap_or_else(default_sequence),
        config,
        report,
    }))
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

fn run(args: impl Iterator<Item = OsString>, out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let args = match parse_args(args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            let _ = writeln!(out, "{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            let _ = writeln!(err, "error: {message}");
            return ExitCode::from(1);
        }
    };

    let config = match &args.config {
        None => RenderConfig::default(),
        Some(path) => {
            let json = match std::fs::read_to_string(path) {
                Ok(json) => json,
                Err(error) => {
                    let _ = writeln!(err, "error: cannot read {}: {error}", path.display());
                    return ExitCode::from(1);
                }
            };
            match serde_json::from_str::<RenderConfig>(&json) {
                Ok(config) => config,
                Err(error) => {
                    let _ = writeln!(err, "error: invalid config JSON: {error}");
                    return ExitCode::from(1);
                }
            }
        }
    };

    if let Err(error) = config.validate() {
        let _ = writeln!(err, "error: {error}");
        return ExitCode::from(1);
    }

    let sequence = match Sequence::load(&args.sequence) {
        Ok(sequence) => sequence,
        Err(error) => {
            let _ = writeln!(err, "error: {error}");
            return ExitCode::from(2);
        }
    };

    let bank = match build_bank(&args.fixture) {
        Ok(bank) => bank,
        Err(message) => {
            let _ = writeln!(err, "error: {message}");
            return ExitCode::from(2);
        }
    };

    let mut session = match OfflineSession::with_bank(config, bank) {
        Ok(session) => session,
        Err(error) => {
            report_render_error(err, &error);
            return ExitCode::from(3);
        }
    };
    let rendered = match session.render(&sequence) {
        Ok(rendered) => rendered,
        Err(error) => {
            report_render_error(err, &error);
            return ExitCode::from(3);
        }
    };
    if let Err(error) = rendered.write_wav(&args.out) {
        report_render_error(err, &error);
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
            let _ = writeln!(err, "error: cannot serialize report: {error}");
            return ExitCode::from(3);
        }
    };
    match args.report {
        Some(path) => {
            if let Err(error) = std::fs::write(&path, format!("{json}\n")) {
                let _ = writeln!(err, "error: cannot write {}: {error}", path.display());
                return ExitCode::from(1);
            }
            let _ = writeln!(
                out,
                "rendered {} frames to {} (report {})",
                report.frames,
                report.out,
                path.display()
            );
        }
        None => {
            let _ = writeln!(out, "{json}");
        }
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    run(std::env::args_os().skip(1), &mut out, &mut err)
}

fn report_render_error(err: &mut dyn Write, error: &RenderError) {
    let _ = writeln!(err, "error: {error}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
        values.iter().map(OsString::from)
    }

    fn run_with(values: &[&str]) -> (ExitCode, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(args(values), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).expect("utf-8 stdout"),
            String::from_utf8(err).expect("utf-8 stderr"),
        )
    }

    fn write_config(path: &Path, config: RenderConfig) {
        std::fs::write(path, serde_json::to_string(&config).expect("json")).expect("write");
    }

    #[test]
    fn help_prints_usage_and_succeeds() {
        let (code, out, err) = run_with(&["--help"]);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(out.contains("usage: offline_render"), "{out}");
        assert!(err.is_empty());
    }

    #[test]
    fn wrong_positional_count_and_unknown_options_fail() {
        let (code, _, err) = run_with(&[]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("usage: offline_render"), "{err}");

        let (code, _, err) = run_with(&["only-one-arg"]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("usage"), "{err}");

        let (code, _, err) = run_with(&["standard", "out.wav", "--bogus"]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("unknown option --bogus"), "{err}");
    }

    #[test]
    fn missing_option_values_fail_with_the_option_name() {
        for option in ["--sequence", "--config", "--report"] {
            let (code, _, err) = run_with(&["standard", "out.wav", option]);
            assert_eq!(code, ExitCode::from(1));
            assert!(err.contains(option), "{option}: {err}");
            assert!(err.contains("requires a path"), "{option}: {err}");
        }
    }

    #[test]
    fn bad_config_paths_and_json_fail() {
        let temp = tempfile::tempdir().expect("tempdir");
        let out = temp.path().join("out.wav");
        let out = out.to_string_lossy().into_owned();

        let missing = temp.path().join("missing.json");
        let missing = missing.to_string_lossy().into_owned();
        let (code, _, err) = run_with(&["standard", &out, "--config", &missing]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("cannot read"), "{err}");

        let invalid = temp.path().join("invalid.json");
        std::fs::write(&invalid, b"{ not json").expect("write");
        let invalid = invalid.to_string_lossy().into_owned();
        let (code, _, err) = run_with(&["standard", &out, "--config", &invalid]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("invalid config JSON"), "{err}");

        let bad_values = temp.path().join("bad-values.json");
        write_config(
            &bad_values,
            RenderConfig {
                block_frames: 0,
                ..RenderConfig::default()
            },
        );
        let bad_values = bad_values.to_string_lossy().into_owned();
        let (code, _, err) = run_with(&["standard", &out, "--config", &bad_values]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("block_frames"), "{err}");
    }

    #[test]
    fn missing_sequence_and_unknown_fixture_are_exit_2() {
        let temp = tempfile::tempdir().expect("tempdir");
        let out = temp.path().join("out.wav");
        let out = out.to_string_lossy().into_owned();
        let missing = temp.path().join("missing.seq");
        let missing = missing.to_string_lossy().into_owned();

        let (code, _, err) = run_with(&["standard", &out, "--sequence", &missing]);
        assert_eq!(code, ExitCode::from(2));
        assert!(err.contains("cannot read"), "{err}");

        let (code, _, err) = run_with(&["definitely-not-a-fixture", &out]);
        assert_eq!(code, ExitCode::from(2));
        assert!(err.contains("unknown fixture"), "{err}");
    }

    #[test]
    fn missing_wav_fixture_is_exit_2() {
        let temp = tempfile::tempdir().expect("tempdir");
        let out = temp.path().join("out.wav");
        let out = out.to_string_lossy().into_owned();
        let sample = temp.path().join("missing.wav");
        let sample = sample.to_string_lossy().into_owned();
        let (code, _, err) = run_with(&[&sample, &out]);
        assert_eq!(code, ExitCode::from(2));
        assert!(err.contains("WAV error"), "{err}");
    }

    #[test]
    fn standard_render_writes_a_wav_and_a_json_report() {
        let temp = tempfile::tempdir().expect("tempdir");
        let out = temp.path().join("out.wav");
        let report = temp.path().join("report.json");
        let (code, stdout, err) = run_with(&["standard", &s(&out), "--report", &s(&report)]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert!(stdout.contains("rendered"), "{stdout}");
        assert!(out.exists());
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).expect("report")).expect("json");
        assert_eq!(json["fixture"], "standard");
        assert_eq!(json["config"]["sample_rate"], 48_000);
        assert!(json["frames"].as_u64().unwrap_or(0) > 0);
        assert!(json["stats"]["output_sha256"].as_str().is_some());
        assert_eq!(
            json["trigger_count"].as_array().map(Vec::len),
            Some(16),
            "report must carry one trigger count per pad"
        );
    }

    #[test]
    fn two_runs_produce_byte_identical_output() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first = temp.path().join("first.wav");
        let second = temp.path().join("second.wav");
        let (code, _, err) = run_with(&["one_shot", &s(&first)]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        let (code, _, err) = run_with(&["one_shot", &s(&second)]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert_eq!(
            std::fs::read(&first).expect("first"),
            std::fs::read(&second).expect("second"),
            "offline renders must be byte-identical"
        );
    }

    #[test]
    fn wav_fixture_argument_is_accepted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sample_path = temp.path().join("sample.wav");
        let sample = fixture::buffer("one_shot").expect("fixture");
        fixture::write_wav(&sample_path, &sample).expect("write sample");
        let out = temp.path().join("out.wav");
        let (code, _, err) = run_with(&[&s(&sample_path), &s(&out)]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert!(out.exists());
    }

    #[test]
    fn json_report_goes_to_stdout_when_no_report_path_is_given() {
        let temp = tempfile::tempdir().expect("tempdir");
        let out = temp.path().join("out.wav");
        let (code, stdout, err) = run_with(&["one_shot", &s(&out)]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout json");
        assert_eq!(json["fixture"], "one_shot");
    }

    #[test]
    fn unwritable_output_and_report_paths_are_typed_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sequence = temp.path().join("one.seq");
        std::fs::write(&sequence, b"0 note_on 0 100\n").expect("sequence");
        let sequence = sequence.to_string_lossy().into_owned();

        // A directory as the output path fails inside write_wav -> render error.
        let (code, _, err) = run_with(&["one_shot", &s(temp.path()), "--sequence", &sequence]);
        assert_eq!(code, ExitCode::from(3));
        assert!(err.contains("fixture error"), "{err}");

        // A directory as the report path fails after a successful render.
        let out = temp.path().join("out.wav");
        let (code, _, err) = run_with(&[
            "one_shot",
            &s(&out),
            "--sequence",
            &sequence,
            "--report",
            &s(temp.path()),
        ]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("cannot write"), "{err}");
    }
}
