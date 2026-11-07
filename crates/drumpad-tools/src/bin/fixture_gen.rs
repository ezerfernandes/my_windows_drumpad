//! Generate or verify the committed deterministic WAV fixtures.
//!
//! ```text
//! fixture_gen               # write tests/fixtures/samples + manifest.json
//! fixture_gen --check       # verify the tree matches a fresh generation
//! fixture_gen --root <dir>  # write somewhere else
//! ```
//!
//! Exit codes: 0 success, 1 argument or IO error, 2 verification mismatch.

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

struct Args {
    root: PathBuf,
    check: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args_os().skip(1);
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
            Some("--help" | "-h") => {
                println!("usage: fixture_gen [--root <dir>] [--check]");
                std::process::exit(0);
            }
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid UTF-8".to_owned()),
        }
    }
    Ok(Args {
        root: root.unwrap_or_else(default_root),
        check,
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

    if args.check {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => {
                eprintln!("error: cannot create a temporary directory: {error}");
                return ExitCode::from(1);
            }
        };
        let fresh = match generate_all(temp.path()) {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!("error: cannot generate fixtures: {error}");
                return ExitCode::from(1);
            }
        };
        let mut mismatches = 0usize;
        for entry in &fresh.fixtures {
            let committed = args.root.join(&entry.file);
            match sha256_file(&committed) {
                Ok(hash) if hash == entry.sha256 => {}
                Ok(hash) => {
                    eprintln!(
                        "mismatch: {} is {hash}, generator produced {}",
                        committed.display(),
                        entry.sha256
                    );
                    mismatches += 1;
                }
                Err(error) => {
                    eprintln!("missing or unreadable fixture: {error}");
                    mismatches += 1;
                }
            }
        }
        if mismatches > 0 {
            eprintln!(
                "{mismatches} fixture(s) differ from a fresh generation; run `cargo run -p drumpad-tools --bin fixture_gen`"
            );
            return ExitCode::from(2);
        }
        println!(
            "fixtures verified: {} files match tests/fixtures/manifest.json",
            fresh.fixtures.len()
        );
        return ExitCode::SUCCESS;
    }

    match generate_all(&args.root) {
        Ok(manifest) => {
            println!(
                "wrote {} fixtures and manifest.json to {}",
                manifest.fixtures.len(),
                args.root.display()
            );
            for entry in &manifest.fixtures {
                println!(
                    "  {:<10} {:>6} frames  {}",
                    entry.name, entry.frames, entry.sha256
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: cannot generate fixtures: {error}");
            ExitCode::from(1)
        }
    }
}
