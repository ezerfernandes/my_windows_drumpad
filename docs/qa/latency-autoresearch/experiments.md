# Experiments — latency autoresearch

One row per iteration, in loop order. The metric, the Verify command and the guard are pinned in
[`summary.md`](summary.md) and identical in every row. Raw rows:
[`loop-261108-1034.tsv`](loop-261108-1034.tsv).

## Iteration 0 — baseline (commit `ed48b7f`)

```text
cargo run -q -p drumpad-tools --bin latency_proxy -- --hits 1200 --detail
mode=shared requested_frames=128
negotiated: callback_period_frames=512 buffer_frames=1126 rate=48000
key_to_queue: p50=0.0000 ms p99=0.0003 ms
midi_to_callback: p99=0.3086 ms
audio period+buffer: 10.6667 ms + 23.4583 ms
proxy_e2e: p50=34.1250 ms p99=34.1253 ms
```

The app requested `share_mode: Shared`; the Realtek shared engine period is fixed at 512 frames
with a 1126-frame buffer, so the software path was ~34 ms before any analog stage.

## Iteration 1 — exclusive mode as the app default (commit `293b86a`) — **keep**

Hypothesis 1: the MVP pins WASAPI shared mode; exclusive measured 256/256 against 1126/512.

Change: `AudioSettings::default().share_mode = ShareMode::Exclusive`, keeping
`fallback_to_shared: true`, `buffer_frames: 128`, `exclusive_bits: 16` and
`low_latency_shared: true`. The negotiated mode is now shown in the status bar.

```text
mode=exclusive requested_frames=128
negotiated: callback_period_frames=256 buffer_frames=256 rate=48000
key_to_queue: p50=0.0000 ms p99=0.0003 ms
midi_to_callback: p99=0.2646 ms
audio period+buffer: 5.3333 ms + 5.3333 ms
proxy_e2e: p50=10.6667 ms p99=10.6670 ms
```

`34.1253 → 10.6670 ms` (**−23.4583 ms, −68.7 %**). Guard green.

### Iteration 1b — review fix-forward (commit `fae65da`) — **keep**

The verifier found that enabling the "WASAPI exclusive" checkbox created a control that does
nothing: `AudioPrefs` is never fed into `MvpAudioRuntime`, so the checkbox could be unticked
while the stream stayed exclusive. The control is read-only again with text that says the mode is
chosen automatically and shown in the status bar. `AudioPrefs::default().exclusive` stays `true`
so persisted preferences match reality. Stale "shared mode" doc comments in `runtime.rs` were
corrected and the fake-backend period assertion is now labelled as a fake-backend expectation.
Metric unchanged (10.6670 ms); this is a truthfulness fix, not a latency experiment.

## Iteration 2 — smallest accepted period (commit `293b86a`) — **no-op**

Hypothesis 2: request the smallest buffer the endpoint accepts, never silently fail or play
off-pitch.

Already satisfied. `open_exclusive` requests the aligned period near the configured buffer and
then takes `.max(min_period)`, with the `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` retry using the
reported buffer size. The capability table records that the Realtek endpoint clamps every request
below 256 frames up to 256, so requesting 256 instead of 128 negotiates the same values.
`MvpAudioRuntime` refuses to run if the device negotiates a different sample rate, so a wrong
pitch is an explicit error rather than a silent failure.

Metric 10.6669 ms — flat, as predicted. No code change.

## Iteration 3 — key path (commit `293b86a`) — **no-op**

Hypothesis 3: remove per-frame waits, extra queue hops and locks between the key event and the
engine queue.

Already satisfied. The path is synchronous inside the window event handler:

```text
WindowEvent::KeyboardInput
  -> AppShell::keyboard()            (modifier gate, shortcut table, inspector capture)
  -> AppState::handle_key()          (KeyRouter)
  -> dispatch(Action::TriggerPad)    -> Effect::Trigger
  -> apply(Effect::Trigger)          -> MvpAudioRuntime::trigger()
  -> EngineHandle::note_on()         -> lock-free SPSC push
```

`MvpAudioRuntime::trigger` takes one mutex, but it is only ever touched by the UI thread, so it
is uncontended. There is no `request_redraw`, no frame wait, and no intermediate queue: the
event handler pushes to the engine before it returns. `key_to_queue` p99 is 0.0003 ms, i.e. the
measurement is already at timer resolution.

Metric 10.6671 ms — flat. No code change.

## Iteration 4 — callback hygiene and MMCSS (commit `293b86a`) — **no-op**

Hypothesis 4: the audio callback must stay allocation-, lock- and syscall-free, and the stream
thread must hold the real-time class.

Already satisfied. `open()` calls `crate::priority::boost_audio_thread()` before opening the
stream, which enters MMCSS "Pro Audio" at `AVRT_PRIORITY_CRITICAL`. The render loop uses
preallocated `f32_scratch` / `byte_scratch`, `Engine::process` uses fixed arrays
(`pending: [Option<(u64, InputEvent)>; 64]`, a stack `due` array), and `StreamDiagnostics`
records atomics only. The backend spike measured 87 late callbacks in 8 s without MMCSS and zero
with it, so this is load-bearing, not optional.

Metric 10.6669 ms — flat. No code change.

## Why the loop stops

Iterations 2–4 are 10.6669 / 10.6671 / 10.6669 ms: a 0.0002 ms spread on the same commit. At
48 kHz the endpoint floor is 256 frames exclusive, so `period + buffer` is fixed at 10.667 ms and
the only remaining term is a 0.0003 ms queue push. There is no code change left that this metric
can rank. Continuing would produce commits the metric cannot justify, so the loop stops at a
plateau rather than at the 20-iteration budget.

## Real-device check of the assumption (not an iteration)

The proxy trusts the capability table, so the negotiated values were confirmed directly with
`cargo test -p drumpad-audio --test hardware exclusive_mode_falls_back_or_runs -- --ignored --nocapture --test-threads=1`:

```text
device_name: "Speakers (Realtek(R) Audio)"  sample_format: I16
share_mode: Exclusive
buffer_frames: 256  callback_period_frames: 256  stream_latency_ms: Some(5.3333)
fell_back: false
callbacks: 95  frames_rendered: 24320  last_interval_us: 5419
```

The exclusive request lands on 256/256 on the real endpoint, so the proxy's 10.667 ms is what
the hardware actually negotiates, not a table guess. This probe takes the endpoint from other
clients for its 0.5 s run, so it must not be run while someone is playing.

