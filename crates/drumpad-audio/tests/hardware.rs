//! Hardware smoke tests. These need a real Windows render device, so they are
//! ignored by default. Run with:
//!
//! ```text
//! cargo test -p drumpad-audio --test hardware -- --ignored --nocapture
//! ```

use drumpad_audio::backend::{AudioBackend, AudioCallback, WindowsBackend};
use drumpad_audio::{ShareMode, StreamConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

struct Silence {
    blocks: Arc<AtomicU32>,
}

impl AudioCallback for Silence {
    fn process(&mut self, out: &mut [f32], frames: usize) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        for sample in out.iter_mut().take(frames * 2) {
            *sample = 0.0;
        }
    }
}

fn run_for(_stream: &mut drumpad_audio::RenderStream, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires a Windows audio render device"]
fn default_device_opens_and_renders_shared() {
    let backend = WindowsBackend;
    let devices = backend.list_devices().expect("list devices");
    assert!(!devices.is_empty(), "no render devices");
    println!("devices: {devices:#?}");

    let blocks = Arc::new(AtomicU32::new(0));
    let mut stream = backend
        .open_stream(
            StreamConfig {
                buffer_frames: 128,
                share_mode: ShareMode::Shared,
                ..StreamConfig::default()
            },
            Box::new(Silence {
                blocks: Arc::clone(&blocks),
            }),
        )
        .expect("open shared stream");
    println!("shared format: {:#?}", stream.format());
    run_for(&mut stream, Duration::from_millis(500));
    let snapshot = stream.diagnostics().snapshot();
    stream.stop();
    println!("shared snapshot: {snapshot:#?}");
    assert!(snapshot.callbacks > 0, "stream never ran");
    assert!(blocks.load(Ordering::Relaxed) > 0, "callback never ran");
    assert!(
        snapshot.xruns < snapshot.callbacks / 2 + 5,
        "too many xruns while idle: {snapshot:#?}"
    );
}

#[test]
#[ignore = "requires a Windows audio render device"]
fn start_stop_one_hundred_times_on_a_real_device() {
    let backend = WindowsBackend;
    for _ in 0..100 {
        let mut stream = backend
            .open_stream(
                StreamConfig {
                    buffer_frames: 128,
                    share_mode: ShareMode::Shared,
                    ..StreamConfig::default()
                },
                Box::new(Silence {
                    blocks: Arc::new(AtomicU32::new(0)),
                }),
            )
            .expect("open shared stream");
        stream.stop();
        assert!(!stream.is_running());
    }
}

#[test]
#[ignore = "requires a Windows audio render device"]
fn exclusive_mode_falls_back_or_runs() {
    let backend = WindowsBackend;
    let blocks = Arc::new(AtomicU32::new(0));
    let mut stream = backend
        .open_stream(
            StreamConfig {
                buffer_frames: 128,
                share_mode: ShareMode::Exclusive,
                fallback_to_shared: true,
                ..StreamConfig::default()
            },
            Box::new(Silence {
                blocks: Arc::clone(&blocks),
            }),
        )
        .expect("open exclusive or fallback stream");
    println!("exclusive/fallback format: {:#?}", stream.format());
    run_for(&mut stream, Duration::from_millis(500));
    let snapshot = stream.diagnostics().snapshot();
    stream.stop();
    println!("exclusive snapshot: {snapshot:#?}");
    assert!(snapshot.callbacks > 0);
    assert!(blocks.load(Ordering::Relaxed) > 0, "callback never ran");
}
