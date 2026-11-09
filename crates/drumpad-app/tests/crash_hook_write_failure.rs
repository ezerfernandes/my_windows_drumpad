//! Verifies the panic hook survives an unwritable crash directory.
//!
//! This runs in its own test binary because it installs the process-global
//! panic hook and deliberately panics on a thread.

use std::fs;

use drumpad_app::logging::install_crash_hook;
use tempfile::tempdir;

#[test]
fn panic_hook_survives_an_unwritable_crash_directory() {
    let temp = tempdir().expect("temp dir");
    let crash_path = temp.path().join("crashes-is-a-file");
    fs::write(&crash_path, b"not a directory").expect("write blocker");
    install_crash_hook(crash_path, "0.1.0-test");

    let panic = std::thread::spawn(|| panic!("no report can be written"));
    assert!(
        panic.join().is_err(),
        "the panicking thread must still unwind"
    );

    // The hook reports the write failure on stderr and chains to the previous
    // hook; reaching this assertion proves it did not panic or abort itself.
}
