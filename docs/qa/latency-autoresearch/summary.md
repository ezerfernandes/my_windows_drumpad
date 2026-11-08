# Latency autoresearch — keyboard to first audible sample

Run: `TEAM-two-pack-20261108073433-e73a` (coder `OliveCrane`, verifier `CreamPlateau`).
Machine: ASUSTeK ROG Strix G533QS, Windows 11, onboard Realtek High Definition Audio.
Loop log: [`loop-261108-1034.tsv`](loop-261108-1034.tsv). Experiment detail:
[`experiments.md`](experiments.md).

## Result

| | mode | period | buffer | proxy p50 | proxy p99 |
|---|---|---|---|---|---|
| before | shared | 512 frames (10.667 ms) | 1126 frames (23.458 ms) | 34.1250 ms | **34.1253 ms** |
| after | exclusive | 256 frames (5.333 ms) | 256 frames (5.333 ms) | 10.6667 ms | **10.6670 ms** |

**p99 34.1253 ms → 10.6670 ms, −23.4583 ms (−68.7 %).** Repeatability over four runs of the
same commit: 10.6669 / 10.6670 / 10.6671 / 10.6669 ms (spread 0.0002 ms).

`key_to_queue` p99 was 0.0003 ms before and after: the whole win is the WASAPI sharing mode.
The app's keyboard path was already sub-microsecond, so no code change on that path can move
the audible result.

## Metric (pinned before the baseline, unchanged every iteration)

```text
proxy_e2e_p99_ms = key_to_queue_p99_ms + negotiated_callback_period_ms + negotiated_buffer_ms
```

- `key_to_queue_p99_ms` is measured live over 1200 hits (100 warm-up discarded, nearest-rank
  percentiles) through the real input trigger, engine queue and mixer with a caller-driven fake
  audio backend. It is the same measurement `latency_probe` reports.
- `negotiated_callback_period_ms` and `negotiated_buffer_ms` come from
  [`device-capabilities.json`](device-capabilities.json), a table transcribed from the real
  hardware runs in [`docs/audio/backend-spike.md`](../../audio/backend-spike.md). The table
  **fails closed**: a request that was never measured on the reference endpoint is an error, so
  a candidate cannot win by asking for a buffer the driver would silently clamp.
- Verify command:

  ```powershell
  cargo run -q -p drumpad-tools --bin latency_proxy -- --json docs/qa/latency-autoresearch/iteration-<N>.json
  ```

  stdout is exactly `proxy_e2e_p99_ms=<number>`.

### What the proxy does not measure

The proxy is **not** an analog loopback. It excludes:

- the USB keyboard scan interval and the Windows input stack before the app sees the event;
- the DAC, the 3.5 mm output, and the Nitro Max Aux In / headphone path.

It also assumes the reference endpoint negotiates the same values the capability table records.
That assumption was confirmed on the real device during this run (see below). The analog loopback
procedure in [`docs/qa/latency-protocol.md`](../latency-protocol.md) remains the release gate.

## Real-device confirmation

`cargo test -p drumpad-audio --test hardware exclusive_mode_falls_back_or_runs -- --ignored --nocapture --test-threads=1`
opened the real default endpoint and reported:

```text
device_name: "Speakers (Realtek(R) Audio)"
sample_rate: 48000, channels: 2, sample_format: I16
share_mode: Exclusive
buffer_frames: 256, callback_period_frames: 256
stream_latency_ms: Some(5.3333)
fell_back: false, fallback_reason: None
callbacks: 95, frames_rendered: 24320, xruns: 2, late_callbacks: 2 (start-up), last_interval_us: 5419
```

So the exclusive request really lands on 256 frames with a 5.333 ms driver-reported stream
latency — the capability table was not optimistic, and the proxy's 10.667 ms is the value this
endpoint actually negotiates. The callback interval (5419 µs) matches the 256-frame period.

Caveat: opening an exclusive stream takes the endpoint away from other clients, so this probe
briefly interrupted the human's running app. It is not a safe command while someone is playing.


## The change

`AudioSettings::default()` now requests WASAPI **exclusive** mode, with
`fallback_to_shared: true`. Everything else is unchanged: 48 kHz, 128-frame request (the
driver clamps to its 256-frame minimum), 16-bit exclusive, `low_latency_shared: true`.

- `crates/drumpad-audio/src/settings.rs` — the default request.
- `crates/drumpad-app/src/state/mod.rs` — `AudioStatus` carries the negotiated `share_mode`
  and the status bar prints it.
- `crates/drumpad-app/src/app/runner.rs` — fills it from the negotiated `StreamFormat`.
- `crates/drumpad-app/src/state/prefs.rs` — persisted preference default matches reality; the
  obsolete `ExclusiveNotImplemented` validation gate is gone.
- `crates/drumpad-app/src/ui/settings.rs` — the mode control stays read-only and says the mode
  is chosen automatically, because `AudioPrefs` is not wired into `MvpAudioRuntime` yet.
- `crates/drumpad-tools/tests/mvp_effects.rs`, `docs/audio/{mvp-runtime,latency-guide}.md` —
  assertions and docs updated.

### Trade-off the human must know

Exclusive mode **locks the endpoint**: other applications cannot play audio through that device
while Drumpad is running. When the endpoint is busy or rejects integer formats the stream falls
back to shared mode, keeps playing, and the status bar shows `shared` plus the fallback reason.
That is why `fallback_to_shared` stays on.

## Hypotheses that were already satisfied

Measured flat (10.6669–10.6671 ms) and left alone, because the metric cannot see them and the
rules say not to keep a change the metric cannot rank:

- **Smallest accepted period.** `open_exclusive` already aligns the request and clamps to the
  device minimum (`.max(min_period)`) with the `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` retry.
  Requesting 256 instead of 128 negotiates the same 256/256.
- **Key path.** `WindowEvent::KeyboardInput` → `keyboard()` → `dispatch(Action::TriggerPad)` →
  `apply(Effect::Trigger)` → `MvpAudioRuntime::trigger` → `EngineHandle::note_on` → lock-free
  SPSC push. No per-frame wait, no queue hop, one uncontended mutex.
- **Callback hygiene / MMCSS.** The stream thread enters MMCSS "Pro Audio" at CRITICAL before
  opening; `Engine::process` uses fixed arrays and the diagnostics are atomics only.

## Where the remaining latency lives

At 48 kHz the driver will not accept a period below 256 frames (5.333 ms), so the software
floor on this endpoint is 2 × 5.333 = 10.667 ms plus `key_to_queue`. The rest of the audible
delay is the DAC, the analog output and the module's Aux In — measurable only with the loopback
rig in the QA protocol.

Two untested directions, both outside this run's search space:

1. A 96 kHz exclusive stream. The driver minimum is expressed in time, so it would most likely
   stay 5.333 ms and cost twice the CPU — but it has never been measured.
2. A USB audio interface that accepts a smaller exclusive period.

## Reproducing

```powershell
# Metric (safe while the app runs; no device access)
cargo run -q -p drumpad-tools --bin latency_proxy -- --hits 1200 --detail

# Guard (needs a target dir the running app does not lock)
$env:CARGO_TARGET_DIR="C:\Users\ezers\AppData\Local\Temp\opencode\drumpad-ar-target"
cargo fmt --all --check
cargo test -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools
cargo clippy -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools --all-targets -- -D warnings

# Real device (requires the app to be closed)
cargo test -p drumpad-audio --test hardware -- --ignored --nocapture --test-threads=1
```

## Commits

| commit | what |
|---|---|
| `ed48b7f` | verifier harness + iteration-0 baseline evidence (cleaner) |
| `293b86a` | **experiment:** default to WASAPI exclusive with shared fallback and visible mode |
| `fae65da` | **experiment:** keep the mode control read-only, correct stale runtime docs (review fix-forward) |
