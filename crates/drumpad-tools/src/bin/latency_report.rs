//! Render the latency baseline JSON as Markdown, or validate it for CI.
//!
//! Usage:
//!
//! ```text
//! latency_report <baseline.json> [--markdown <out.md>] [--require-pass]
//! ```
//!
//! Exit codes:
//! - `0`: report rendered (and passes when `--require-pass` is given)
//! - `1`: invalid arguments or unreadable/invalid JSON
//! - `2`: `--require-pass` was given and at least one row is pending or failing

use std::path::PathBuf;
use std::process::ExitCode;

use drumpad_tools::latency::BaselineReport;

struct Args {
    input: PathBuf,
    markdown: Option<PathBuf>,
    require_pass: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args_os().skip(1);
    let input = args.next().map(PathBuf::from).ok_or_else(|| {
        "usage: latency_report <baseline.json> [--markdown <out.md>] [--require-pass]".to_owned()
    })?;
    let mut markdown = None;
    let mut require_pass = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--markdown") => {
                let value = args
                    .next()
                    .ok_or_else(|| "--markdown requires a path".to_owned())?;
                markdown = Some(PathBuf::from(value));
            }
            Some("--require-pass") => require_pass = true,
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid UTF-8".to_owned()),
        }
    }
    Ok(Args {
        input,
        markdown,
        require_pass,
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

    let json = match std::fs::read_to_string(&args.input) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", args.input.display());
            return ExitCode::from(1);
        }
    };
    let report: BaselineReport = match serde_json::from_str(&json) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: invalid baseline JSON: {error}");
            return ExitCode::from(1);
        }
    };

    let markdown = report.to_markdown();
    match &args.markdown {
        Some(path) => {
            if let Err(error) = std::fs::write(path, &markdown) {
                eprintln!("error: cannot write {}: {error}", path.display());
                return ExitCode::from(1);
            }
            println!("wrote {}", path.display());
        }
        None => print!("{markdown}"),
    }

    let pending = report
        .rows
        .iter()
        .filter(|row| row.status == drumpad_tools::latency::BaselineStatus::Pending)
        .count();
    let failed = report
        .rows
        .iter()
        .filter(|row| {
            row.status == drumpad_tools::latency::BaselineStatus::Measured && !row.passes()
        })
        .count();
    println!(
        "rows: {} measured+passing: {} pending: {} failing: {}",
        report.rows.len(),
        report.rows.iter().filter(|row| row.passes()).count(),
        pending,
        failed
    );

    if args.require_pass && !report.all_pass() {
        eprintln!(
            "error: --require-pass was set but not every row is measured and passing (pending {pending}, failing {failed})"
        );
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
