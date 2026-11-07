//! Keyboard latency spike harness.
//!
//! Measures the part of the key-to-queue budget this crate owns: physical key
//! lookup, velocity generation, trigger construction, and the lock-free push
//! into the audio queue. The OS-to-handler and queue-to-callback legs are
//! measured on the reference hardware with the protocol in
//! `docs/keyboard-input.md`; this harness keeps the software leg honest on
//! every run.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p drumpad-input --release --example keyboard_latency_spike
//! ```

use drumpad_input::keyboard::{KeyboardInput, KeyboardProfile, PhysicalKey};
use drumpad_input::trigger::{
    EventClock, LatencyProbe, LatencyRecorder, MonotonicClock, event_queue,
};

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100_000);

    let (producer, consumer) = event_queue(4096);
    let mut keyboard =
        KeyboardInput::default_with_profile(&KeyboardProfile::default_full_keyboard());
    let mut recorder = LatencyRecorder::new(iterations);
    let clock = MonotonicClock::new();
    let mut dropped = 0_u64;
    let mut missed_taps = 0_u64;
    let mut taps = 0_u64;

    // Phase 1: walk every pad key so lookup costs are representative.
    for index in 0..iterations {
        let key = PhysicalKey::PAD_BLOCK[index % PhysicalKey::PAD_BLOCK.len()];
        let timestamp = clock.now_us();
        let probe = LatencyProbe::start();
        let action = keyboard.key_down(key, timestamp);
        if let Some(event) = action.event() {
            taps += 1;
            if producer.push(event).is_err() {
                dropped += 1;
            }
        } else {
            missed_taps += 1;
        }
        probe.finish(&mut recorder);
        let _ = keyboard.key_up(key, clock.now_us());
        while consumer.pop().is_some() {}
    }

    // Phase 2: fast repeated taps of one key. Every down/up pair must produce
    // exactly one trigger; a dropped or suppressed tap is a missed fast tap.
    let fast = 20_000.min(iterations);
    for _ in 0..fast {
        let timestamp = clock.now_us();
        let probe = LatencyProbe::start();
        let down = keyboard.key_down(PhysicalKey::Digit1, timestamp);
        let up = keyboard.key_up(PhysicalKey::Digit1, clock.now_us());
        if down.event().is_some() {
            taps += 1;
        } else {
            missed_taps += 1;
        }
        if let Some(event) = down.event() {
            if producer.push(event).is_err() {
                dropped += 1;
            }
        }
        if up.event().is_none() {
            // Digit1 is a pad; its key-up must always release.
            missed_taps += 1;
        }
        probe.finish(&mut recorder);
        while consumer.pop().is_some() {}
    }

    let Some(summary) = recorder.percentiles() else {
        eprintln!("no samples recorded");
        std::process::exit(2);
    };
    println!("drumpad keyboard latency spike");
    println!("  samples          {}", summary.count);
    println!("  p50              {} us", summary.p50);
    println!("  p90              {} us", summary.p90);
    println!("  p95              {} us", summary.p95);
    println!("  p99              {} us", summary.p99);
    println!("  max              {} us", summary.max);
    println!("  mean             {} us", summary.mean);
    println!("  triggers         {taps}");
    println!("  missed taps      {missed_taps}");
    println!("  queue drops      {dropped}");
    println!(
        "  budget gate      {} (< 2000 us p99)",
        if summary.within_key_budget() {
            "PASS"
        } else {
            "FAIL"
        }
    );
    if dropped > 0 || missed_taps > 0 || !summary.within_key_budget() {
        std::process::exit(1);
    }
}
