//! Verifies the process-wide panic hook in an isolated test binary.
//!
//! The hook installs a global panic handler, so this lives outside the unit-test
//! binary: a panic raised by any other test running in parallel would otherwise
//! be captured into the same temporary crash directory.

use drumpad_app::logging::{CRASH_FILE_PREFIX, install_crash_hook};
use tempfile::tempdir;

#[test]
fn installed_panic_hook_writes_a_local_report() {
    let temp = tempdir().expect("temp dir");
    install_crash_hook(temp.path().to_path_buf(), "0.1.0-test");

    let panic = std::thread::spawn(|| panic!("hook test boom"));
    assert!(panic.join().is_err(), "the test thread must panic");

    let reports: Vec<_> = std::fs::read_dir(temp.path())
        .expect("read crash dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(CRASH_FILE_PREFIX)
        })
        .collect();
    assert_eq!(reports.len(), 1, "one crash report should be written");

    let contents = std::fs::read_to_string(reports[0].path()).expect("read report");
    assert!(contents.contains("hook test boom"));
    assert!(contents.contains("version: 0.1.0-test"));
    assert!(contents.contains("thread:"));
    assert!(contents.contains("backtrace:"));
    assert!(contents.contains("never uploaded"));
}
