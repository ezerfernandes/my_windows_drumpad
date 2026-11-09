//! Soak runner (E11-T06 tooling).
//!
//! Placeholder until the soak module lands. The stub still has to fail loudly
//! and with a stable message so CI and scripts can distinguish "not
//! implemented" from a successful run.

use std::io::Write;
use std::process::ExitCode;

fn run(out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let _ = writeln!(out, "soak runner is not implemented yet");
    let _ = writeln!(err, "soak runner is not implemented yet");
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    run(&mut out, &mut err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_fails_with_a_stable_message() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&mut out, &mut err);
        assert_eq!(code, ExitCode::from(1));
        let out = String::from_utf8(out).expect("utf-8");
        let err = String::from_utf8(err).expect("utf-8");
        assert!(out.contains("not implemented"), "{out}");
        assert!(err.contains("not implemented"), "{err}");
    }
}
