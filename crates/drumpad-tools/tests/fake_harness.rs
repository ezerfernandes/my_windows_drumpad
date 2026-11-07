//! Fake MIDI/audio integration harness tests (E11-T04).
//!
//! These tests run with no audio device, no MIDI port, and no GPU. They drive
//! the real MIDI parser/router, keyboard map, and playback engine through the
//! caller-driven fake backend.

use std::path::{Path, PathBuf};

use drumpad_input::trigger::{InputEvent, PadId, TriggerEvent, TriggerSource, Velocity};
use drumpad_tools::fake::{FakeAudioBackend, FakeHarness, KeyboardScript, MidiScript};
use drumpad_tools::offline::RenderConfig;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

fn full_report() -> drumpad_tools::fake::HarnessReport {
    let midi = MidiScript::load(fixtures_root().join("scripts").join("full.midi"))
        .expect("load MIDI script");
    let keys = KeyboardScript::load(fixtures_root().join("scripts").join("full.keys"))
        .expect("load keyboard script");
    let harness = FakeHarness::new(RenderConfig::default(), midi, keys).expect("harness");
    harness.run()
}

#[test]
fn full_scripted_sequence_runs_without_hardware() {
    let report = full_report();
    assert!(report.is_sane(), "{}", report.failure_dump());
    assert!(
        report.trigger_log.len() >= 7,
        "expected MIDI and keyboard triggers, got {}",
        report.trigger_log.len()
    );
    // Keyboard pad 0, the MIDI kick, and the channel-10 kick.
    assert_eq!(report.trigger_count(0), 3, "{}", report.failure_dump());
    assert_eq!(report.trigger_count(1), 2, "{}", report.failure_dump());
    assert_eq!(report.final_active_voices(), 0, "{}", report.failure_dump());
}

#[test]
fn sequence_covers_note_off_velocity_zero_running_status_and_malformed_bytes() {
    let report = full_report();
    assert!(
        !report.midi_ignored.is_empty(),
        "malformed byte was not reported"
    );
    // The kick is released twice (velocity-zero running status and an explicit
    // note-off) and the first release is the one that counts.
    assert!(
        report.release_count(0) >= 2,
        "velocity-zero/running-status release missing: {}",
        report.failure_dump()
    );
    // The keyboard script releases pads 0, 1, and 2.
    assert!(report.release_count(2) >= 1);
}

#[test]
fn voice_count_rises_and_falls_without_stuck_voices() {
    let report = full_report();
    let peak_voices = report
        .blocks
        .iter()
        .map(|block| block.active)
        .max()
        .unwrap_or(0);
    assert!(
        peak_voices >= 3,
        "voice count never rose: {}",
        report.failure_dump()
    );
    assert_eq!(report.final_active_voices(), 0, "stuck voices");
}

#[test]
fn per_block_output_is_finite_and_has_signal() {
    let report = full_report();
    let noisy_blocks = report
        .blocks
        .iter()
        .filter(|block| block.rms > 1e-6)
        .count();
    assert!(noisy_blocks > 0, "{}", report.failure_dump());
    for block in &report.blocks {
        assert!(
            block.peak.is_finite() && block.rms.is_finite(),
            "non-finite output in block {}: {:?}",
            block.block,
            block
        );
    }
}

#[test]
fn failure_dump_names_the_script_log_and_voice_table() {
    let report = full_report();
    let dump = report.failure_dump();
    assert!(dump.contains("MIDI script"));
    assert!(dump.contains("keyboard script"));
    assert!(dump.contains("trigger log"));
    assert!(dump.contains("voice table"));
    assert!(dump.contains("per-pad"));
}

#[test]
fn all_notes_off_drains_gate_voices() {
    let mut backend = FakeAudioBackend::new(RenderConfig::default()).expect("backend");
    let pad = PadId::new(2).expect("pad"); // gate fixture
    let velocity = Velocity::new(100).expect("velocity");
    backend
        .send_input(InputEvent::Trigger(TriggerEvent::new(
            pad,
            velocity,
            TriggerSource::Keyboard,
            0,
        )))
        .expect("send");
    for _ in 0..4 {
        backend.process_block();
    }
    assert!(
        backend.active_pad_counts()[2] >= 1,
        "gate pad did not start"
    );
    backend.handle_mut().all_notes_off().expect("all off");
    for _ in 0..4 {
        backend.process_block();
    }
    assert_eq!(
        backend.active_pad_counts(),
        [0; 16],
        "all_notes_off left a stuck voice"
    );
}

#[test]
fn queue_full_is_observable_and_does_not_panic() {
    let mut backend = FakeAudioBackend::new(RenderConfig::default()).expect("backend");
    let pad = PadId::new(0).expect("pad");
    let velocity = Velocity::new(100).expect("velocity");
    for index in 0..6_000 {
        let event = TriggerEvent::new(pad, velocity, TriggerSource::Keyboard, index);
        // The queue is allowed to reject; the counter must observe it.
        let _ = backend.send_input(InputEvent::Trigger(event));
    }
    assert!(
        backend.dropped_triggers() > 0,
        "queue accepted every event, so the full-queue path was not exercised"
    );
    for _ in 0..4 {
        let block = backend.process_block();
        assert!(block.iter().all(|sample| sample.is_finite()));
    }
}

#[test]
fn callback_overrun_steals_voices_instead_of_growing() {
    let config = RenderConfig {
        voice_limit: 16,
        ..RenderConfig::default()
    };
    let mut backend = FakeAudioBackend::new(config).expect("backend");
    let pad = PadId::new(5).expect("pad");
    let velocity = Velocity::new(127).expect("velocity");
    for index in 0..128 {
        backend
            .send_input(InputEvent::Trigger(TriggerEvent::new(
                pad,
                velocity,
                TriggerSource::Keyboard,
                index,
            )))
            .expect("send");
    }
    let mut peak = 0u16;
    for _ in 0..8 {
        backend.process_block();
        peak = peak.max(backend.active_pad_counts().iter().copied().sum());
    }
    assert!(peak <= 16, "voice limit exceeded: {peak}");
    backend.handle_mut().panic().expect("panic");
    for _ in 0..8 {
        backend.process_block();
    }
    assert_eq!(backend.active_pad_counts(), [0; 16]);
}

#[test]
fn device_loss_simulation_leaves_no_stuck_notes() {
    let mut backend = FakeAudioBackend::new(RenderConfig::default()).expect("backend");
    let velocity = Velocity::new(100).expect("velocity");
    for pad in [0, 2, 4] {
        backend
            .send_input(InputEvent::Trigger(TriggerEvent::new(
                PadId::new(pad).expect("pad"),
                velocity,
                TriggerSource::Midi,
                0,
            )))
            .expect("send");
    }
    backend.process_block();
    // Device loss: no further input, panic the engine the way the app does on
    // disconnect.
    backend.handle_mut().panic().expect("panic");
    for _ in 0..8 {
        backend.process_block();
    }
    assert_eq!(
        backend.active_pad_counts(),
        [0; 16],
        "device loss left a stuck voice"
    );
}
