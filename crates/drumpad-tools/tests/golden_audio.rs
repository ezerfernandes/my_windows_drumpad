//! Golden audio tests (E11-T03).
//!
//! Each case renders a committed fixture through the real engine with a
//! committed sequence and compares the output against
//! `tests/fixtures/golden/<case>.json` (metrics + SHA-256) and, for short
//! renders, `<case>.f32` (raw interleaved little-endian f32).
//!
//! Regenerate after an intentional DSP change:
//!
//! ```powershell
//! $env:DRUMPAD_UPDATE_GOLDENS = "1"
//! cargo test -p drumpad-tools --test golden_audio
//! Remove-Item Env:DRUMPAD_UPDATE_GOLDENS
//! git diff -- tests/fixtures/golden
//! ```
//!
//! The diff is the review artifact: the commit message must include the
//! before/after metric table from the test output.

use std::path::{Path, PathBuf};

use drumpad_tools::fixture::FixtureBank;
use drumpad_tools::offline::{OfflineRender, OfflineSession, RenderConfig};
use drumpad_tools::sequence::Sequence;
use serde::{Deserialize, Serialize};

const GOLDEN_SCHEMA_VERSION: u32 = 1;
const PEAK_DB_TOLERANCE: f32 = 0.5;
const RMS_DB_TOLERANCE: f32 = 0.5;
const ONSET_TOLERANCE_SAMPLES: u64 = 1;

#[derive(Debug, Clone, Copy)]
struct GoldenCase {
    name: &'static str,
    fixture: &'static str,
    sequence: &'static str,
    tail_frames: u64,
    reference_buffer: bool,
}

const CASES: [GoldenCase; 6] = [
    GoldenCase {
        name: "one_shot",
        fixture: "one_shot",
        sequence: "one_shot.seq",
        tail_frames: 9_600,
        reference_buffer: true,
    },
    GoldenCase {
        name: "gate",
        fixture: "gate",
        sequence: "gate.seq",
        tail_frames: 9_600,
        reference_buffer: false,
    },
    GoldenCase {
        name: "loop",
        fixture: "loop",
        sequence: "loop.seq",
        tail_frames: 9_600,
        reference_buffer: true,
    },
    GoldenCase {
        name: "choke",
        fixture: "standard",
        sequence: "choke.seq",
        tail_frames: 9_600,
        reference_buffer: true,
    },
    GoldenCase {
        name: "limiter",
        fixture: "standard",
        sequence: "limiter.seq",
        tail_frames: 9_600,
        reference_buffer: true,
    },
    GoldenCase {
        name: "pitch",
        fixture: "standard",
        sequence: "pitch.seq",
        tail_frames: 9_600,
        reference_buffer: true,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Golden {
    schema_version: u32,
    case: String,
    fixture: String,
    sequence: String,
    config: RenderConfig,
    frames: u64,
    peak: f32,
    rms: f32,
    peak_db: f32,
    rms_db: f32,
    first_non_zero_frame: Option<u64>,
    last_non_zero_frame: Option<u64>,
    output_sha256: String,
    reference_f32: Option<String>,
    peak_db_tolerance: f32,
    rms_db_tolerance: f32,
    onset_tolerance_samples: u64,
}

impl Golden {
    fn from_render(case: &GoldenCase, rendered: &OfflineRender) -> Self {
        Self {
            schema_version: GOLDEN_SCHEMA_VERSION,
            case: case.name.to_owned(),
            fixture: case.fixture.to_owned(),
            sequence: case.sequence.to_owned(),
            config: rendered.config,
            frames: rendered.frames,
            peak: rendered.stats.peak,
            rms: rendered.stats.rms,
            peak_db: rendered.stats.peak_db,
            rms_db: rendered.stats.rms_db,
            first_non_zero_frame: rendered.stats.first_non_zero_frame,
            last_non_zero_frame: rendered.stats.last_non_zero_frame,
            output_sha256: rendered.stats.output_sha256.clone(),
            reference_f32: case.reference_buffer.then(|| format!("{}.f32", case.name)),
            peak_db_tolerance: PEAK_DB_TOLERANCE,
            rms_db_tolerance: RMS_DB_TOLERANCE,
            onset_tolerance_samples: ONSET_TOLERANCE_SAMPLES,
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

fn fixtures_root() -> PathBuf {
    workspace_root().join("tests").join("fixtures")
}

fn golden_path(case: &GoldenCase) -> PathBuf {
    fixtures_root()
        .join("golden")
        .join(format!("{}.json", case.name))
}

fn reference_path(case: &GoldenCase) -> PathBuf {
    fixtures_root()
        .join("golden")
        .join(format!("{}.f32", case.name))
}

fn update_mode() -> bool {
    std::env::var_os("DRUMPAD_UPDATE_GOLDENS").is_some()
}

fn config_for(case: &GoldenCase) -> RenderConfig {
    RenderConfig {
        tail_frames: case.tail_frames,
        ..RenderConfig::default()
    }
}

fn build_bank(fixture_name: &str) -> FixtureBank {
    if fixture_name == "standard" {
        FixtureBank::standard()
    } else {
        FixtureBank::single(fixture_name)
            .unwrap_or_else(|| panic!("case fixture {fixture_name:?} is not a known fixture"))
    }
}

fn render_case(case: &GoldenCase) -> OfflineRender {
    let sequence_path = fixtures_root().join("sequences").join(case.sequence);
    let sequence = Sequence::load(&sequence_path)
        .unwrap_or_else(|error| panic!("{}: cannot load {:?}: {error}", case.name, sequence_path));
    let config = config_for(case);
    let bank = build_bank(case.fixture);
    let mut session = OfflineSession::with_bank(config, bank)
        .unwrap_or_else(|error| panic!("{}: cannot create session: {error}", case.name));
    session
        .render(&sequence)
        .unwrap_or_else(|error| panic!("{}: render failed: {error}", case.name))
}

fn write_reference(path: &Path, output: &[f32]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create golden directory");
    }
    let mut bytes = Vec::with_capacity(output.len() * 4);
    for sample in output {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write reference buffer");
}

fn read_reference(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("missing golden reference {}: {error}", path.display()));
    assert_eq!(
        bytes.len() % 4,
        0,
        "golden reference {} is not a whole number of f32 values",
        path.display()
    );
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn update_golden(case: &GoldenCase, rendered: &OfflineRender) {
    let golden = Golden::from_render(case, rendered);
    let path = golden_path(case);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create golden directory");
    }
    let json = serde_json::to_string_pretty(&golden).expect("serialize golden");
    std::fs::write(&path, format!("{json}\n")).expect("write golden json");
    if case.reference_buffer {
        write_reference(&reference_path(case), &rendered.output);
    } else {
        let _ = std::fs::remove_file(reference_path(case));
    }
    println!(
        "updated {}: frames={} peak_db={:.3} rms_db={:.3} sha={}",
        path.display(),
        golden.frames,
        golden.peak_db,
        golden.rms_db,
        golden.output_sha256
    );
}

fn load_golden(case: &GoldenCase) -> Golden {
    let path = golden_path(case);
    let json = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: cannot read golden {}: {error}; run with DRUMPAD_UPDATE_GOLDENS=1",
            case.name,
            path.display()
        )
    });
    let golden: Golden = serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("{}: invalid golden JSON: {error}", case.name));
    assert_eq!(
        golden.schema_version, GOLDEN_SCHEMA_VERSION,
        "{}: golden schema changed; regenerate intentionally",
        case.name
    );
    assert_eq!(golden.case, case.name);
    golden
}

fn assert_close(case: &GoldenCase, metric: &str, expected: f32, actual: f32, tolerance: f32) {
    let delta = (expected - actual).abs();
    assert!(
        delta <= tolerance,
        "{}: {metric} changed: expected {expected:.4}, actual {actual:.4}, |delta| {delta:.4} > tolerance {tolerance:.4}",
        case.name
    );
}

fn first_differing_frame(expected: &[f32], actual: &[f32]) -> Option<u64> {
    let len = expected.len().min(actual.len());
    for index in 0..len {
        if expected[index].to_bits() != actual[index].to_bits() {
            return Some((index / 2) as u64);
        }
    }
    (expected.len() != actual.len()).then_some((len / 2) as u64)
}

fn check_case(case: GoldenCase) {
    let rendered = render_case(&case);
    if update_mode() {
        update_golden(&case, &rendered);
        return;
    }
    let golden = load_golden(&case);
    assert_eq!(
        rendered.frames, golden.frames,
        "{}: frame count changed; expected {}, actual {}",
        case.name, golden.frames, rendered.frames
    );
    assert_eq!(
        rendered.config, golden.config,
        "{}: render config changed",
        case.name
    );
    assert_close(
        &case,
        "peak_db",
        golden.peak_db,
        rendered.stats.peak_db,
        golden.peak_db_tolerance,
    );
    assert_close(
        &case,
        "rms_db",
        golden.rms_db,
        rendered.stats.rms_db,
        golden.rms_db_tolerance,
    );
    let onset_delta = match (
        golden.first_non_zero_frame,
        rendered.stats.first_non_zero_frame,
    ) {
        (Some(expected), Some(actual)) => expected.abs_diff(actual),
        (None, None) => 0,
        (expected, actual) => panic!(
            "{}: onset changed: expected {expected:?}, actual {actual:?}",
            case.name
        ),
    };
    assert!(
        onset_delta <= golden.onset_tolerance_samples,
        "{}: onset moved {onset_delta} samples (tolerance {})",
        case.name,
        golden.onset_tolerance_samples
    );
    if case.reference_buffer {
        let expected = read_reference(&reference_path(&case));
        if let Some(frame) = first_differing_frame(&expected, &rendered.output) {
            let index = frame as usize * 2;
            panic!(
                "{}: sample data changed at frame {frame}: expected {:?}, actual {:?} (expected {} samples, actual {}); sha expected {}, actual {}. Run with DRUMPAD_UPDATE_GOLDENS=1 only for a reviewed DSP change.",
                case.name,
                expected.get(index).copied(),
                rendered.output.get(index).copied(),
                expected.len(),
                rendered.output.len(),
                golden.output_sha256,
                rendered.stats.output_sha256
            );
        }
    }
    assert_eq!(
        rendered.stats.output_sha256, golden.output_sha256,
        "{}: output hash changed; expected {}, actual {}",
        case.name, golden.output_sha256, rendered.stats.output_sha256
    );
}

#[test]
fn golden_one_shot() {
    let case = CASES[0];
    check_case(case);
    if !update_mode() {
        let rendered = render_case(&case);
        assert_eq!(
            rendered.stats.first_non_zero_frame,
            Some(1),
            "one_shot: the start fade must zero sample 0 and pass sample 1"
        );
        assert!(rendered.stats.peak > 0.1, "one_shot: no audible output");
        assert!(
            rendered.no_stuck_voices(),
            "one_shot: a voice outlived the rendered tail"
        );
    }
}

#[test]
fn golden_gate_releases_within_the_fade_window() {
    let case = CASES[1];
    check_case(case);
    if !update_mode() {
        let rendered = render_case(&case);
        // Sequence release at frame 24000 is quantized to block 24064; the
        // engine's gate release is 5 ms (240 frames at 48 kHz).
        let after_release = 24_064 + 240 + 128;
        assert_eq!(
            rendered.voices_at(after_release),
            0,
            "gate: voice still active {} frames after the release",
            after_release
        );
        let tail_start = rendered.output.len().saturating_sub(2_000);
        let tail_rms = (rendered.output[tail_start..]
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>()
            / (rendered.output.len() - tail_start) as f64)
            .sqrt();
        assert!(
            tail_rms < 1e-3,
            "gate: tail did not decay to silence (rms {tail_rms:e})"
        );
    }
}

#[test]
fn golden_loop_wraps_without_a_click() {
    let case = CASES[2];
    check_case(case);
    if !update_mode() {
        let rendered = render_case(&case);
        // Panic at 8192 is quantized to 8192; check the loop body only.
        let mut max_step = 0.0f32;
        for frame in 129u64..8_192 {
            let left = rendered.output[frame as usize * 2];
            let previous = rendered.output[(frame as usize - 1) * 2];
            max_step = max_step.max((left - previous).abs());
        }
        assert!(
            max_step < 0.5,
            "loop: discontinuity of {max_step:.4} in the loop body"
        );
        assert!(
            rendered.pad_voices_at(8_000, 3) >= 1,
            "loop: the loop voice stopped before the panic"
        );
        assert!(rendered.no_stuck_voices(), "loop: voice survived the panic");
    }
}

#[test]
fn golden_choke_fades_the_old_voice() {
    let case = CASES[3];
    check_case(case);
    if !update_mode() {
        let rendered = render_case(&case);
        let trigger = 2_432; // 2400 rounded up to the next 128-frame block
        let after_fade = trigger + 6 * 48 + 128; // 6 ms choke + one block of slack
        assert_eq!(
            rendered.pad_voices_at(after_fade, 0),
            0,
            "choke: pad 0 voice survived the choke group fade"
        );
        assert!(
            rendered.pad_voices_at(after_fade, 1) >= 1,
            "choke: pad 1 voice did not survive the choke group fade"
        );
        let mut max_step = 0.0f32;
        for frame in (trigger as usize - 64)..(trigger as usize + 512) {
            let left = rendered.output[frame * 2];
            let previous = rendered.output[(frame - 1) * 2];
            max_step = max_step.max((left - previous).abs());
        }
        assert!(
            max_step < 0.5,
            "choke: discontinuity of {max_step:.4} during the choke fade"
        );
    }
}

#[test]
fn golden_limiter_never_clips() {
    let case = CASES[4];
    check_case(case);
    if !update_mode() {
        let rendered = render_case(&case);
        assert_eq!(
            rendered.stats.non_finite, 0,
            "limiter: rendered non-finite samples"
        );
        assert!(
            rendered.stats.peak < 1.0,
            "limiter: peak {:.6} reached or exceeded 0 dBFS (E11-T03 gate)",
            rendered.stats.peak
        );
    }
}

#[test]
fn golden_pitch_and_resample_ratio_is_preserved() {
    let case = CASES[5];
    check_case(case);
    if !update_mode() {
        let tuned = render_case(&case);
        // The same fixture without the +700 cent pad tuning.
        let untuned_case = GoldenCase {
            name: "pitch",
            fixture: "pitch",
            sequence: "pitch.seq",
            tail_frames: 9_600,
            reference_buffer: false,
        };
        let untuned = render_case(&untuned_case);
        let tuned_frames = tuned.stats.last_non_zero_frame.expect("tuned output") as f64
            - tuned.stats.first_non_zero_frame.unwrap_or(0) as f64;
        let untuned_frames = untuned.stats.last_non_zero_frame.expect("untuned output") as f64
            - untuned.stats.first_non_zero_frame.unwrap_or(0) as f64;
        let expected = untuned_frames * 2.0f64.powf(-700.0 / 1200.0);
        let tolerance = (untuned_frames / 48_000.0).max(1.0);
        assert!(
            (tuned_frames - expected).abs() <= tolerance,
            "pitch: tuned duration {tuned_frames} frames, expected {expected:.1} +/- {tolerance:.1} (untuned {untuned_frames})"
        );
    }
}

#[test]
fn golden_manifest_lists_every_case() {
    for case in CASES {
        let path = golden_path(&case);
        assert!(
            path.exists() || update_mode(),
            "{}: missing {}",
            case.name,
            path.display()
        );
    }
    let manifest = fixtures_root().join("manifest.json");
    assert!(
        manifest.exists(),
        "missing fixture manifest {}; run fixture_gen",
        manifest.display()
    );
}
