//! CLI behavior tests for the latency evidence tools.
//!
//! These drive the real `latency_proxy` and `latency_report` binaries as
//! subprocesses, so they exercise the contract a person or CI job actually
//! uses: argument parsing, exit codes, the one-number proxy output, the JSON
//! record, the Markdown report, and the fail-closed handling of malformed or
//! incomplete evidence.
//!
//! Everything is deterministic: no audio device, no network, no sleeps. The
//! proxy's timing loop is a local engine queue push, so `--hits 300` (the
//! protocol minimum) finishes in milliseconds.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const PROXY: &str = env!("CARGO_BIN_EXE_latency_proxy");
const REPORT: &str = env!("CARGO_BIN_EXE_latency_report");

/// Path to the committed reference baseline that ships with the crate.
fn committed_baseline() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("baselines")
        .join("latency-baseline.json")
}

/// Path to the committed autoresearch capability table.
fn committed_capabilities() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("qa")
        .join("latency-autoresearch")
        .join("device-capabilities.json")
}

fn run(bin: &str, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not run {bin}: {error}"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn exit_code(output: &Output) -> i32 {
    output
        .status
        .code()
        .unwrap_or_else(|| panic!("process ended without an exit code: {output:?}"))
}

/// One measured configuration of the reference endpoint.
fn mode_entry(accepted: &[u32], period: u32, buffer: u32) -> Value {
    json!({
        "accepted_request_frames": accepted,
        "callback_period_frames": period,
        "buffer_frames": buffer,
        "note": "test fixture",
    })
}

/// A capability table in the committed schema.
fn capability_table(schema_version: u32, rate: u32, shared: Value, exclusive: Value) -> Value {
    json!({
        "schema_version": schema_version,
        "device": "Test Endpoint",
        "sample_rate": rate,
        "source": "tests/latency_cli.rs",
        "notes": "fixture",
        "modes": { "shared": shared, "exclusive": exclusive },
    })
}

fn write_json(dir: &Path, name: &str, value: &Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(value).expect("serialize fixture"),
    )
    .expect("write fixture");
    path
}

/// The table recorded from the reference endpoint in docs/audio/backend-spike.md.
fn reference_table() -> Value {
    capability_table(
        1,
        48_000,
        mode_entry(&[64, 128, 256], 512, 1126),
        mode_entry(&[64, 128, 256], 256, 256),
    )
}

/// The mode the app asks for when it starts without a settings file.
fn app_default_mode() -> String {
    drumpad_audio::AudioSettings::default()
        .share_mode
        .to_string()
}

/// A measured, passing baseline row.
fn measured_row(machine: &str, pass: bool) -> Value {
    json!({
        "status": "measured",
        "hardware": {
            "machine": machine,
            "cpu": "Test CPU",
            "ram_gib": 16.0,
            "os": "Windows 11",
            "os_build": "10.0.0",
            "audio_device": "Test Endpoint",
            "audio_driver": "Test Driver",
            "audio_mode": "exclusive",
            "keyboard": "scripted",
            "midi_interface": "none",
            "monitoring_path": "none",
            "operator": "test",
            "date_utc": "2026-11-09",
        },
        "sample_rate": 48_000,
        "block_frames": 256,
        "verdicts": [{
            "metric": "key_to_queue",
            "percentile": "p99",
            "budget_ms": 2.0,
            "hits": 200,
            "stats": {
                "count": 200,
                "min_ms": 0.001,
                "p50_ms": 0.002,
                "p95_ms": 0.003,
                "p99_ms": 0.004,
                "max_ms": 0.005,
                "mean_ms": 0.002,
                "std_dev_ms": 0.0005,
            },
            "pass": pass,
            "reason": if pass { "pass" } else { "p99 over budget" },
        }],
        "raw_capture": "tests/raw.csv",
        "notes": "fixture row",
    })
}

fn baseline_report(rows: Vec<Value>) -> Value {
    json!({
        "schema_version": 1,
        "generated_by": "tests/latency_cli.rs",
        "protocol": "docs/qa/latency-protocol.md",
        "rows": rows,
        "notes": "fixture report",
    })
}

// ---------------------------------------------------------------------------
// latency_proxy
// ---------------------------------------------------------------------------

#[test]
fn proxy_help_prints_usage_and_exits_zero() {
    let output = run(PROXY, &["--help"]);
    assert_eq!(exit_code(&output), 0);
    assert!(
        stdout(&output).contains("usage: latency_proxy"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn proxy_rejects_an_unknown_argument() {
    let output = run(PROXY, &["--bogus"]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("unknown argument: --bogus"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_rejects_hits_below_the_protocol_minimum() {
    let output = run(PROXY, &["--hits", "299"]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("--hits must be at least 300"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_rejects_a_hits_flag_without_a_value() {
    let output = run(PROXY, &["--hits"]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("--hits requires an integer"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_reports_a_missing_capability_table() {
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("absent.json");
    let output = run(
        PROXY,
        &["--capabilities", missing.to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 1);
    let message = stderr(&output);
    assert!(message.contains("cannot read"), "{message}");
    assert!(message.contains("absent.json"), "{message}");
}

#[test]
fn proxy_rejects_a_capability_table_that_is_not_json() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("broken.json");
    std::fs::write(&path, "{ not a capability table").expect("write");
    let output = run(
        PROXY,
        &["--capabilities", path.to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("is not a capability table"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_rejects_an_unsupported_capability_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let table = capability_table(
        2,
        48_000,
        mode_entry(&[128], 512, 1126),
        mode_entry(&[128], 256, 256),
    );
    let path = write_json(dir.path(), "schema2.json", &table);
    let output = run(
        PROXY,
        &["--capabilities", path.to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("schema 2 is not supported"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_fails_closed_when_the_app_request_was_never_measured() {
    let dir = tempfile::tempdir().expect("temp dir");
    // No accepted request frames in either mode: whatever the app default is,
    // the table must refuse to rank it instead of extrapolating.
    let table = capability_table(
        1,
        48_000,
        mode_entry(&[], 512, 1126),
        mode_entry(&[], 256, 256),
    );
    let path = write_json(dir.path(), "unmeasured.json", &table);
    let output = run(
        PROXY,
        &["--capabilities", path.to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 1);
    let message = stderr(&output);
    assert!(message.contains("was never measured on"), "{message}");
    assert!(message.contains("Test Endpoint"), "{message}");
    // The safety property is that no number is printed when the evidence
    // cannot rank the request: a stale metric must never look like a result.
    assert!(
        stdout(&output).is_empty(),
        "fail-closed must print no metric, got {:?}",
        stdout(&output)
    );
}

#[test]
fn proxy_rejects_a_capability_table_at_another_sample_rate() {
    let dir = tempfile::tempdir().expect("temp dir");
    let table = capability_table(
        1,
        44_100,
        mode_entry(&[128], 512, 1126),
        mode_entry(&[128], 256, 256),
    );
    let path = write_json(dir.path(), "rate.json", &table);
    let output = run(
        PROXY,
        &["--capabilities", path.to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("only covers 44100 Hz"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_prints_one_number_and_writes_a_machine_readable_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let table = write_json(dir.path(), "caps.json", &reference_table());
    let record_path = dir.path().join("record.json");

    let output = run(
        PROXY,
        &[
            "--hits",
            "300",
            "--capabilities",
            table.to_str().expect("utf-8 path"),
            "--json",
            record_path.to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));

    // The documented contract is exactly one line on stdout.
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "expected one line, got {text:?}");
    let printed: f64 = lines[0]
        .strip_prefix("proxy_e2e_p99_ms=")
        .unwrap_or_else(|| panic!("unexpected stdout {text:?}"))
        .parse()
        .expect("printed metric is a number");
    assert!(printed > 0.0, "proxy must report a positive latency");

    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(&record_path).expect("record file"))
            .expect("record is JSON");
    assert_eq!(record["schema_version"], 1);
    assert_eq!(
        record["sample_rate"],
        u64::from(drumpad_audio::runtime::MVP_SAMPLE_RATE)
    );
    assert_eq!(
        record["requested_frames"],
        drumpad_audio::runtime::MVP_BLOCK_FRAMES as u64,
        "the proxy must request the block size the MVP runtime pins"
    );
    assert_eq!(record["hits"], 200, "100 warm-up hits must be discarded");
    assert_eq!(
        record["mode"].as_str().expect("mode"),
        app_default_mode(),
        "the record must report the mode the app actually requests"
    );

    // The negotiated values must come from the table entry for that mode.
    let (period, buffer) = if record["mode"] == "exclusive" {
        (256.0, 256.0)
    } else {
        (512.0, 1126.0)
    };
    assert_eq!(record["callback_period_frames"], period as u64);
    assert_eq!(record["buffer_frames"], buffer as u64);

    let key_p99 = record["key_to_queue_p99_ms"].as_f64().expect("key p99");
    let expected = key_p99 + period * 1000.0 / 48_000.0 + buffer * 1000.0 / 48_000.0;
    let recorded = record["proxy_e2e_p99_ms"].as_f64().expect("proxy p99");
    assert!(
        (recorded - expected).abs() < 1e-6,
        "recorded {recorded} != key {key_p99} + period + buffer {expected}"
    );
    assert!(
        (printed - recorded).abs() < 1e-4,
        "stdout {printed} must match the record {recorded}"
    );
    assert_eq!(record["device"].as_str().expect("device"), "Test Endpoint");
}

#[test]
fn proxy_detail_explains_every_component_of_the_metric() {
    let dir = tempfile::tempdir().expect("temp dir");
    let table = write_json(dir.path(), "caps.json", &reference_table());

    let output = run(
        PROXY,
        &[
            "--hits",
            "300",
            "--capabilities",
            table.to_str().expect("utf-8 path"),
            "--detail",
        ],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.starts_with(&format!("mode={}", app_default_mode())),
        "{text}"
    );
    for expected in [
        "negotiated: callback_period_frames=",
        "key_to_queue: p50=",
        "midi_to_callback: p99=",
        "audio period+buffer:",
        "proxy_e2e: p50=",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in {text}");
    }
    assert!(
        text.lines()
            .last()
            .is_some_and(|line| line.starts_with("proxy_e2e_p99_ms=")),
        "the last line must stay machine-readable: {text}"
    );
}

#[test]
fn proxy_reports_the_mode_and_negotiated_frames_the_table_records() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Distinct values per mode so a wrong lookup cannot pass by accident.
    let table = capability_table(
        1,
        48_000,
        mode_entry(&[128], 480, 960),
        mode_entry(&[128], 240, 240),
    );
    let path = write_json(dir.path(), "distinct.json", &table);
    let record_path = dir.path().join("record.json");

    let output = run(
        PROXY,
        &[
            "--hits",
            "300",
            "--capabilities",
            path.to_str().expect("utf-8 path"),
            "--json",
            record_path.to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));

    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(&record_path).expect("record file"))
            .expect("record is JSON");
    let mode = record["mode"].as_str().expect("mode");
    let (period, buffer) = match mode {
        "exclusive" => (240, 240),
        "shared" => (480, 960),
        other => panic!("unexpected mode {other}"),
    };
    assert_eq!(record["callback_period_frames"], period);
    assert_eq!(record["buffer_frames"], buffer);
    // 480 frames @ 48 kHz = 10 ms; 240 frames = 5 ms.
    let period_ms = period as f64 * 1000.0 / 48_000.0;
    let buffer_ms = buffer as f64 * 1000.0 / 48_000.0;
    let key_p99 = record["key_to_queue_p99_ms"].as_f64().expect("key p99");
    let proxy = record["proxy_e2e_p99_ms"].as_f64().expect("proxy p99");
    assert!((proxy - (key_p99 + period_ms + buffer_ms)).abs() < 1e-6);
}

#[test]
fn proxy_accepts_the_committed_autoresearch_evidence_table() {
    // The evidence table committed with the autoresearch run must still accept
    // whatever the app requests today. This is the contract between the
    // evidence and the product default, not a hardcoded mode.
    let output = run(
        PROXY,
        &[
            "--hits",
            "300",
            "--capabilities",
            committed_capabilities().to_str().expect("utf-8 path"),
            "--detail",
        ],
    );
    assert_eq!(
        exit_code(&output),
        0,
        "committed evidence table rejected the app default: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).starts_with(&format!("mode={}", app_default_mode())),
        "{}",
        stdout(&output)
    );
}

#[test]
fn proxy_reports_a_json_directory_it_cannot_create() {
    let dir = tempfile::tempdir().expect("temp dir");
    let table = write_json(dir.path(), "caps.json", &reference_table());
    // A regular file where the record's parent directory should be.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "not a directory").expect("write blocker");
    let record = blocker.join("record.json");

    let output = run(
        PROXY,
        &[
            "--hits",
            "300",
            "--capabilities",
            table.to_str().expect("utf-8 path"),
            "--json",
            record.to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("cannot create"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn proxy_reports_a_json_path_it_cannot_write() {
    let dir = tempfile::tempdir().expect("temp dir");
    let table = write_json(dir.path(), "caps.json", &reference_table());
    // A directory is not a writable record target.
    let output = run(
        PROXY,
        &[
            "--hits",
            "300",
            "--capabilities",
            table.to_str().expect("utf-8 path"),
            "--json",
            dir.path().to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("cannot write"),
        "{}",
        stderr(&output)
    );
}

// ---------------------------------------------------------------------------
// latency_report
// ---------------------------------------------------------------------------

#[test]
fn report_renders_the_committed_baseline_as_markdown() {
    let output = run(
        REPORT,
        &[committed_baseline().to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("# Latency baseline results"), "{text}");
    assert!(
        text.contains("| Status | Hardware | OS build | Driver | Device | Mode | Rate / buffer |"),
        "{text}"
    );
    // The committed document has one measured passing row and three pending
    // reference rows; pending rows are not evidence.
    assert!(
        text.contains("rows: 4 measured+passing: 1 pending: 3 failing: 0"),
        "{text}"
    );
    assert!(
        text.contains("| pending |"),
        "pending rows must be labelled"
    );
}

#[test]
fn report_writes_the_markdown_to_a_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let markdown_path = dir.path().join("latency-baseline.md");
    let output = run(
        REPORT,
        &[
            committed_baseline().to_str().expect("utf-8 path"),
            "--markdown",
            markdown_path.to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).starts_with("wrote "), "{}", stdout(&output));
    let written = std::fs::read_to_string(&markdown_path).expect("markdown file");
    assert!(written.contains("# Latency baseline results"), "{written}");
    assert!(written.contains("| Status | Hardware"), "{written}");
    // The document keeps the report notes that explain the pending rows.
    assert!(written.contains("## Notes"), "{written}");
    assert!(written.contains("Generated by"), "{written}");
}

#[test]
fn report_rejects_a_missing_input_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("absent.json");
    let output = run(REPORT, &[missing.to_str().expect("utf-8 path")]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("cannot read"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_rejects_invalid_baseline_json() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("broken.json");
    std::fs::write(&path, "{\"rows\": \"not a list\"}").expect("write");
    let output = run(REPORT, &[path.to_str().expect("utf-8 path")]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("invalid baseline JSON"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_requires_an_input_argument() {
    let output = run(REPORT, &[]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("usage: latency_report"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_rejects_an_unknown_argument() {
    let output = run(
        REPORT,
        &[
            committed_baseline().to_str().expect("utf-8 path"),
            "--bogus",
        ],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("unknown argument: --bogus"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_rejects_a_markdown_path_it_cannot_write() {
    let dir = tempfile::tempdir().expect("temp dir");
    // A directory is not a writable file target.
    let output = run(
        REPORT,
        &[
            committed_baseline().to_str().expect("utf-8 path"),
            "--markdown",
            dir.path().to_str().expect("utf-8 path"),
        ],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("cannot write"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_rejects_a_markdown_flag_without_a_value() {
    let output = run(
        REPORT,
        &[
            committed_baseline().to_str().expect("utf-8 path"),
            "--markdown",
        ],
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("--markdown requires a path"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_require_pass_fails_while_reference_rows_are_pending() {
    let output = run(
        REPORT,
        &[
            committed_baseline().to_str().expect("utf-8 path"),
            "--require-pass",
        ],
    );
    assert_eq!(exit_code(&output), 2);
    let message = stderr(&output);
    assert!(
        message.contains("--require-pass was set but not every row"),
        "{message}"
    );
    assert!(message.contains("pending 3"), "{message}");
    assert!(message.contains("failing 0"), "{message}");
}

#[test]
fn report_require_pass_succeeds_when_every_row_is_measured_and_passing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_json(
        dir.path(),
        "all-pass.json",
        &baseline_report(vec![measured_row("Test Bench", true)]),
    );
    let output = run(
        REPORT,
        &[path.to_str().expect("utf-8 path"), "--require-pass"],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("rows: 1 measured+passing: 1 pending: 0 failing: 0"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn report_counts_a_measured_row_that_fails_the_budget() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_json(
        dir.path(),
        "failing.json",
        &baseline_report(vec![measured_row("Test Bench", false)]),
    );

    let output = run(REPORT, &[path.to_str().expect("utf-8 path")]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("rows: 1 measured+passing: 0 pending: 0 failing: 1"),
        "{text}"
    );
    assert!(
        text.contains("| FAIL |"),
        "the verdict must be visible: {text}"
    );

    let output = run(
        REPORT,
        &[path.to_str().expect("utf-8 path"), "--require-pass"],
    );
    assert_eq!(exit_code(&output), 2);
    assert!(stderr(&output).contains("failing 1"), "{}", stderr(&output));
}

#[test]
fn report_escapes_pipes_in_evidence_text_so_the_table_stays_valid() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_json(
        dir.path(),
        "pipes.json",
        &baseline_report(vec![measured_row("Desk|Bench", true)]),
    );
    let output = run(REPORT, &[path.to_str().expect("utf-8 path")]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Desk\\|Bench"),
        "a pipe in the machine name must be escaped: {text}"
    );
}

#[test]
fn report_renders_the_negotiated_mode_for_measured_and_pending_rows() {
    // The committed baseline records every row in shared mode: one measured
    // offline row and three pending reference rows.
    let output = run(
        REPORT,
        &[committed_baseline().to_str().expect("utf-8 path")],
    );
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    let data_rows: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("| measured") || line.starts_with("| pending"))
        .collect();
    assert!(!data_rows.is_empty(), "no data rows in {text}");
    assert!(
        data_rows.iter().all(|row| row.contains("| shared |")),
        "every committed row was measured in shared mode: {text}"
    );

    // One exclusive and one shared row, so a constant in the Mode column
    // cannot satisfy the contract.
    let dir = tempfile::tempdir().expect("temp dir");
    let mut exclusive = measured_row("Exclusive Bench", true);
    exclusive["hardware"]["audio_mode"] = json!("exclusive");
    let mut shared = measured_row("Shared Bench", true);
    shared["hardware"]["audio_mode"] = json!("shared");
    let path = write_json(
        dir.path(),
        "modes.json",
        &baseline_report(vec![exclusive, shared]),
    );

    let output = run(REPORT, &[path.to_str().expect("utf-8 path")]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("| exclusive |"),
        "the exclusive row must render its mode: {text}"
    );
    assert!(
        text.contains("| shared |"),
        "the shared row must render its mode: {text}"
    );
}
