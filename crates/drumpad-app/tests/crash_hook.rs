//! Verifies the process-wide panic hook in an isolated test binary.
//!
//! The hook installs a global panic handler, so this lives outside the unit-test
//! binary: a panic raised by any other test running in parallel would otherwise
//! be captured into the same temporary crash directory.

use std::fs;

use drumpad_app::logging::{
    CRASH_FILE_PREFIX, CrashReport, install_crash_hook, set_device_context,
};
use tempfile::tempdir;

#[test]
fn installed_panic_hook_writes_reports_for_every_payload_shape() {
    let temp = tempdir().expect("temp dir");
    install_crash_hook(temp.path().to_path_buf(), "0.1.0-test");
    set_device_context("audio=Test Output 48000 Hz 128 frames; midi=Nitro Max");

    let literal = std::thread::spawn(|| panic!("literal payload boom"));
    assert!(
        literal.join().is_err(),
        "the literal-payload thread must panic"
    );

    let formatted = std::thread::spawn(|| {
        let pad = 3;
        panic!("formatted payload {pad}");
    });
    assert!(
        formatted.join().is_err(),
        "the formatted-payload thread must panic"
    );

    let non_string = std::thread::spawn(|| std::panic::panic_any(42_u32));
    assert!(
        non_string.join().is_err(),
        "the non-string-payload thread must panic"
    );

    let mut reports = Vec::new();
    for entry in fs::read_dir(temp.path()).expect("read crash dir") {
        let entry = entry.expect("crash entry");
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(CRASH_FILE_PREFIX)
        {
            reports.push(fs::read_to_string(entry.path()).expect("read crash report"));
        }
    }

    assert_eq!(reports.len(), 3, "one report per panic: {reports:#?}");
    assert!(
        reports.iter().any(|report| {
            report.contains("message: literal payload boom")
                && report.contains("device: audio=Test Output 48000 Hz 128 frames; midi=Nitro Max")
        }),
        "literal report missing or without device context: {reports:#?}"
    );
    assert!(
        reports
            .iter()
            .any(|report| report.contains("message: formatted payload 3")),
        "formatted payload not captured: {reports:#?}"
    );
    assert!(
        reports
            .iter()
            .any(|report| report.contains("message: <non-string panic payload>")),
        "non-string payload not captured: {reports:#?}"
    );
    assert!(reports.iter().all(|report| {
        report.contains("version: 0.1.0-test")
            && report.contains("thread:")
            && report.contains("backtrace:")
            && report.contains("never uploaded")
    }));
}

#[test]
fn crash_report_render_normalizes_a_missing_trailing_newline() {
    let report = CrashReport {
        version: "0.1.0".to_owned(),
        unix_time_secs: 1_700_000_002,
        os: "windows",
        arch: "x86_64",
        thread: "render-test".to_owned(),
        device_context: None,
        message: "rendered without a trailing backtrace newline".to_owned(),
        location: Some("crates/drumpad-app/tests/crash_hook.rs:1".to_owned()),
        backtrace: "frame-without-newline".to_owned(),
    };

    let rendered = report.render();
    assert!(
        rendered.contains("frame-without-newline\nThis report is stored locally"),
        "backtrace and closing note must be separated by a newline: {rendered}"
    );
    assert!(rendered.contains("device: <not opened>"));
}

#[test]
fn crash_report_write_reports_an_unusable_directory() {
    let temp = tempdir().expect("temp dir");
    let blocked = temp.path().join("crashes-is-a-file");
    fs::write(&blocked, b"not a directory").expect("write blocker");

    let report = CrashReport {
        version: "0.1.0".to_owned(),
        unix_time_secs: 1_700_000_003,
        os: "windows",
        arch: "x86_64",
        thread: "write-test".to_owned(),
        device_context: None,
        message: "write failure".to_owned(),
        location: None,
        backtrace: "<unavailable>\n".to_owned(),
    };

    let error = report
        .write_to(&blocked)
        .expect_err("a file cannot be a crash directory");
    assert!(
        matches!(error, drumpad_model::error::Error::Io { .. }),
        "unexpected {error:?}"
    );
}
