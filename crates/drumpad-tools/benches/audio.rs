//! Criterion benchmarks for E01-T06.
//!
//! Bench names carry their parameters so a stored baseline is self-describing:
//!
//! - `voice_mixing/16_voices_128_frames_48k`
//! - `resampling/44100_to_48000_mono_2s`
//! - `kit_loading/parse_demo_kit_json`
//!
//! None of them opens an audio device. Run with:
//!
//! ```powershell
//! cargo bench -p drumpad-tools --bench audio
//! ```

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use drumpad_model::ids::{PadId, Velocity};
use drumpad_samples::decode::AudioData;
use drumpad_tools::fixture::{self, FixtureBank};
use drumpad_tools::offline::{OfflineSession, RenderConfig};

const BLOCK_FRAMES: usize = 128;
const SAMPLE_RATE: u32 = 48_000;
const VOICES: usize = 16;
const BLOCKS_PER_ITERATION: usize = 8;

fn bench_voice_mixing(criterion: &mut Criterion) {
    let config = RenderConfig {
        sample_rate: SAMPLE_RATE,
        block_frames: BLOCK_FRAMES,
        ..RenderConfig::default()
    };
    let bank = FixtureBank::single("loop").expect("loop fixture");
    let mut session = OfflineSession::with_bank(config, bank).expect("session");
    let velocity = Velocity::new(127).expect("velocity");
    for pad in 0..VOICES {
        session
            .handle_mut()
            .note_on(
                PadId::from_raw_unchecked(pad as u8),
                velocity,
                drumpad_input::trigger::TriggerSource::UiPreview,
            )
            .expect("queue trigger");
    }
    let mut output = vec![0.0f32; BLOCK_FRAMES * 2];
    // One block applies the triggers; the loop samples then sustain.
    session.process_into(&mut output);

    let mut group = criterion.benchmark_group("voice_mixing");
    group.throughput(Throughput::Elements(
        (VOICES * BLOCK_FRAMES * BLOCKS_PER_ITERATION) as u64,
    ));
    group.bench_function("16_voices_128_frames_48k", |bencher| {
        bencher.iter(|| {
            for _ in 0..BLOCKS_PER_ITERATION {
                session.process_into(black_box(&mut output));
            }
        });
    });
    group.finish();
}

fn bench_resampling(criterion: &mut Criterion) {
    let mono = fixture::synthesize("gate", 44_100).expect("gate fixture");
    let audio = AudioData::new(44_100, 1, mono).expect("audio data");
    let frames = audio.frames() as u64;
    let mut group = criterion.benchmark_group("resampling");
    group.throughput(Throughput::Elements(frames));
    group.bench_function("44100_to_48000_mono_2s", |bencher| {
        bencher.iter(|| {
            let output =
                drumpad_samples::resample::resample(black_box(&audio), 48_000).expect("resample");
            black_box(output.frames());
        });
    });
    group.finish();
}

fn bench_kit_loading(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("kit_loading");
    group.bench_function("parse_demo_kit_json", |bencher| {
        bencher.iter(|| {
            let kit = drumpad_model::kit::demo::demo_kit().expect("demo kit");
            black_box(kit.pads.len());
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_voice_mixing,
    bench_resampling,
    bench_kit_loading
);
criterion_main!(benches);
