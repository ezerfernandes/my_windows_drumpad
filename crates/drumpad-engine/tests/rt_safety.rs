//! Real-time safety guardrails.
//!
//! The allocation test runs the engine with a counting global allocator: once
//! the stream is warm, `Engine::process` must not allocate or free a single
//! byte. The rapid-swap test fills the sample-bank retire path with far more
//! banks than it can hold, so a precheck error cannot silently turn into an
//! `Arc` drop (or a debug panic) on the audio thread.
//!
//! The source scan checks the real-time modules for calls that are never
//! acceptable in the callback (logging, filesystem, sleeping, mutexes) and for
//! panicking helpers in the non-test code.

use drumpad_engine::mixer::{Engine, EngineConfig, MAX_VOICES};
use drumpad_engine::queue::Command;
use drumpad_engine::voice::{Playback, SampleBank, SampleBuffer};
use drumpad_input::trigger::TriggerSource;
use drumpad_model::ids::{PadId, Velocity};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The allocation counters are process-wide, so tests in this binary must not
/// run concurrently with a measured block.
static SERIAL: Mutex<()> = Mutex::new(());

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn install_bank(engine: &mut Engine, handle: &mut drumpad_engine::mixer::EngineHandle) {
    let sample = SampleBuffer::from_mono(vec![0.05f32; 48_000], 48_000).unwrap();
    let mut bank = SampleBank::new();
    bank.push(Arc::new(sample));
    handle
        .send(Command::SwapSampleBank(Arc::new(bank)))
        .unwrap();
    for index in 0..16u8 {
        let pad = PadId::from_raw_unchecked(index);
        handle.send(Command::SetPadSample { pad, slot: 0 }).unwrap();
        handle
            .send(Command::SetPadPlayback {
                pad,
                playback: Playback::default(),
            })
            .unwrap();
    }
    let mut out = vec![0.0f32; 128 * 2];
    engine.process(&mut out, 128);
    engine.process(&mut out, 128);
}

#[test]
fn engine_process_allocates_nothing() {
    let _serial = SERIAL.lock().unwrap();
    let (mut engine, mut handle) = Engine::new(EngineConfig {
        max_voices: MAX_VOICES,
        voice_limit: 64,
        ..EngineConfig::default()
    });
    install_bank(&mut engine, &mut handle);

    // Fill the pool, then warm up envelopes and meters.
    for _ in 0..4 {
        for index in 0..16u8 {
            handle
                .note_on(
                    PadId::from_raw_unchecked(index),
                    Velocity::new(110).unwrap(),
                    TriggerSource::Keyboard,
                )
                .unwrap();
        }
    }
    let mut out = vec![0.0f32; 128 * 2];
    for _ in 0..16 {
        engine.process(&mut out, 128);
    }
    assert_eq!(engine.active_voice_count(), 64);

    // From here on, no allocation may happen on the audio path. Trigger
    // pushes, command pushes, and block processing are all included.
    ALLOCATIONS.store(0, Ordering::Relaxed);
    for block in 0..1_000u64 {
        if block % 32 == 0 {
            let pad = PadId::from_raw_unchecked((block / 32 % 16) as u8);
            handle
                .note_on(pad, Velocity::new(100).unwrap(), TriggerSource::Keyboard)
                .unwrap();
        }
        if block % 64 == 0 {
            handle
                .send(Command::SetPadGain {
                    pad: PadId::from_raw_unchecked((block / 64 % 16) as u8),
                    gain: 0.75,
                })
                .unwrap();
        }
        engine.process(&mut out, 128);
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    assert_eq!(
        allocations, 0,
        "Engine::process allocated {allocations} times in 1,000 blocks"
    );
}

#[test]
fn rapid_bank_swaps_do_not_free_on_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap();
    let (mut engine, mut handle) = Engine::new(EngineConfig {
        max_voices: MAX_VOICES,
        voice_limit: 64,
        ..EngineConfig::default()
    });
    install_bank(&mut engine, &mut handle);
    let mut out = vec![0.0f32; 128 * 2];
    // Warm up with a few blocks so no lazy initialization lands in the
    // measured region.
    for _ in 0..4 {
        engine.process(&mut out, 128);
    }

    // 64 distinct banks, all queued for one block. This overflows the retire
    // queue (8) + overflow slots (16) on purpose.
    let banks: Vec<Arc<SampleBank>> = (0..64)
        .map(|index| {
            let mut bank = SampleBank::new();
            let sample = SampleBuffer::from_mono(vec![index as f32 * 0.001; 128], 48_000).unwrap();
            bank.push(Arc::new(sample));
            Arc::new(bank)
        })
        .collect();
    for bank in &banks {
        handle
            .send(Command::SwapSampleBank(Arc::clone(bank)))
            .unwrap();
    }
    drop(banks);

    ALLOCATIONS.store(0, Ordering::Relaxed);
    DEALLOCATIONS.store(0, Ordering::Relaxed);
    engine.process(&mut out, 128);
    assert_eq!(
        ALLOCATIONS.load(Ordering::Relaxed),
        0,
        "rapid bank swaps allocated on the audio thread"
    );
    assert_eq!(
        DEALLOCATIONS.load(Ordering::Relaxed),
        0,
        "rapid bank swaps freed a bank on the audio thread"
    );

    // The UI drains retired banks and lets deferred swaps make progress.
    for _ in 0..256 {
        engine.process(&mut out, 128);
        let _ = handle.collect_retired_banks();
    }
    assert_eq!(handle.retired_bank_count(), 0);
    assert_eq!(handle.dropped_commands(), 0);
}

#[test]
fn real_time_modules_are_free_of_forbidden_calls() {
    let _serial = SERIAL.lock().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let modules = [
        "mixer/mod.rs",
        "voice/mod.rs",
        "limiter/mod.rs",
        "meter/mod.rs",
        "queue/mod.rs",
    ];
    for module in modules {
        let text = std::fs::read_to_string(root.join(module)).unwrap();
        let non_test = text.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "println!",
            "eprintln!",
            "std::fs::",
            "thread::sleep",
            "Mutex",
            "log::",
            "tracing::",
        ] {
            assert!(
                !non_test.contains(forbidden),
                "{module} contains forbidden real-time call {forbidden}"
            );
        }
        for panicking in ["unwrap(", "expect(", "panic!"] {
            assert!(
                !non_test.contains(panicking),
                "{module} uses {panicking} in real-time code"
            );
        }
    }
}
