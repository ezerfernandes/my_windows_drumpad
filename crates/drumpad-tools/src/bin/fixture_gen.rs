//! Generate or verify the committed deterministic WAV fixtures.
//!
//! ```text
//! fixture_gen               # write tests/fixtures/samples + manifest.json
//! fixture_gen --check       # verify the tree matches a fresh generation
//! fixture_gen --root <dir>  # write somewhere else
//! ```
//!
//! Exit codes: 0 success, 1 argument or IO error, 2 verification mismatch.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use drumpad_tools::fixture::{generate_all, sha256_file};

fn default_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    root: PathBuf,
    check: bool,
}

const USAGE: &str = "usage: fixture_gen [--root <dir>] [--check]";

/// Parse command-line arguments. `Ok(None)` means `--help` was requested.
fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Option<Args>, String> {
    let mut root = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--root") => {
                let value = args
                    .next()
                    .ok_or_else(|| "--root requires a path".to_owned())?;
                root = Some(PathBuf::from(value));
            }
            Some("--check") => check = true,
            Some("--help" | "-h") => return Ok(None),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid UTF-8".to_owned()),
        }
    }
    Ok(Some(Args {
        root: root.unwrap_or_else(default_root),
        check,
    }))
}

fn run(args: impl Iterator<Item = OsString>, out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let args = match parse_args(args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            let _ = writeln!(out, "{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            let _ = writeln!(err, "error: {message}");
            return ExitCode::from(1);
        }
    };

    if args.check {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => {
                let _ = writeln!(err, "error: cannot create a temporary directory: {error}");
                return ExitCode::from(1);
            }
        };
        let fresh = match generate_all(temp.path()) {
            Ok(manifest) => manifest,
            Err(error) => {
                let _ = writeln!(err, "error: cannot generate fixtures: {error}");
                return ExitCode::from(1);
            }
        };
        let mut mismatches = 0usize;
        for entry in &fresh.fixtures {
            let committed = args.root.join(&entry.file);
            match sha256_file(&committed) {
                Ok(hash) if hash == entry.sha256 => {}
                Ok(hash) => {
                    let _ = writeln!(
                        err,
                        "mismatch: {} is {hash}, generator produced {}",
                        committed.display(),
                        entry.sha256
                    );
                    mismatches += 1;
                }
                Err(error) => {
                    let _ = writeln!(err, "missing or unreadable fixture: {error}");
                    mismatches += 1;
                }
            }
        }
        if mismatches > 0 {
            let _ = writeln!(
                err,
                "{mismatches} fixture(s) differ from a fresh generation; run `cargo run -p drumpad-tools --bin fixture_gen`"
            );
            return ExitCode::from(2);
        }
        let _ = writeln!(
            out,
            "fixtures verified: {} files match tests/fixtures/manifest.json",
            fresh.fixtures.len()
        );
        return ExitCode::SUCCESS;
    }

    match generate_all(&args.root) {
        Ok(manifest) => {
            let _ = writeln!(
                out,
                "wrote {} fixtures and manifest.json to {}",
                manifest.fixtures.len(),
                args.root.display()
            );
            for entry in &manifest.fixtures {
                let _ = writeln!(
                    out,
                    "  {:<10} {:>6} frames  {}",
                    entry.name, entry.frames, entry.sha256
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = writeln!(err, "error: cannot generate fixtures: {error}");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    run(std::env::args_os().skip(1), &mut out, &mut err)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn help_returns_success_without_touching_files() {
        let (code, out, err) = run_with(&["--help"]);
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(out.contains("usage: fixture_gen"), "{out}");
        assert!(err.is_empty());
    }

    #[test]
    fn unknown_and_incomplete_arguments_fail() {
        let (code, _, err) = run_with(&["--bogus"]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("unknown argument"), "{err}");

        let (code, _, err) = run_with(&["--root"]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("--root requires a path"), "{err}");
    }

    #[test]
    fn default_root_is_the_committed_fixture_directory() {
        let parsed = parse_args(args(&[])).expect("parse").expect("args");
        assert!(!parsed.check);
        assert!(
            parsed.root.ends_with(Path::new("tests").join("fixtures")),
            "{}",
            parsed.root.display()
        );
    }

    #[test]
    fn generate_then_verify_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_string_lossy().into_owned();

        let (code, out, err) = run_with(&["--root", &root]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert!(out.contains("wrote 6 fixtures"), "{out}");
        assert!(temp.path().join("manifest.json").exists());

        let (code, out, err) = run_with(&["--root", &root, "--check"]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert!(out.contains("fixtures verified: 6"), "{out}");
    }

    #[test]
    fn check_reports_a_tampered_fixture() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_string_lossy().into_owned();
        let (code, _, _) = run_with(&["--root", &root]);
        assert_eq!(code, ExitCode::SUCCESS);

        let tampered = temp.path().join("samples").join("one_shot.wav");
        std::fs::write(&tampered, b"not a wav").expect("tamper");
        let (code, _, err) = run_with(&["--root", &root, "--check"]);
        assert_eq!(code, ExitCode::from(2));
        assert!(err.contains("mismatch"), "{err}");
        assert!(err.contains("differ from a fresh generation"), "{err}");
    }

    #[test]
    fn check_reports_a_missing_fixture() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_string_lossy().into_owned();
        let (code, _, _) = run_with(&["--root", &root]);
        assert_eq!(code, ExitCode::SUCCESS);
        std::fs::remove_file(temp.path().join("samples").join("gate.wav")).expect("remove");
        let (code, _, err) = run_with(&["--root", &root, "--check"]);
        assert_eq!(code, ExitCode::from(2));
        assert!(err.contains("missing or unreadable"), "{err}");
    }

    #[test]
    fn generation_failure_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        // A file where the fixture root should be makes create_dir_all fail.
        let blocked = temp.path().join("blocked");
        std::fs::write(&blocked, b"x").expect("write");
        let root = blocked.to_string_lossy().into_owned();
        let (code, _, err) = run_with(&["--root", &root]);
        assert_eq!(code, ExitCode::from(1));
        assert!(err.contains("cannot generate fixtures"), "{err}");
    }
}
