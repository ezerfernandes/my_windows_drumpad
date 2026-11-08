# Iteration 1 — WASAPI exclusive default

Reviewed commit: `293b86a` (`experiment: default to WASAPI exclusive with
shared fallback and visible mode`).
Reviewed by `CreamPlateau` (cleaner), run
`TEAM-two-pack-20261108073433-e73a`.

## Metric

| | Iteration 0 (`896afa5`) | Iteration 1 (`293b86a`) |
| --- | --- | --- |
| mode | shared | exclusive |
| requested frames | 128 | 128 |
| negotiated period | 512 frames (10.6667 ms) | 256 frames (5.3333 ms) |
| negotiated buffer | 1126 frames (23.4583 ms) | 256 frames (5.3333 ms) |
| key_to_queue p50 / p99 | 0.0001 / 0.0006 ms | 0.0000 / 0.0002 ms |
| **proxy_e2e p50** | **34.1251 ms** | **10.6667 ms** |
| **proxy_e2e p99** | **34.1256 ms** | **10.6669 ms** |
| delta p99 | | **-23.4587 ms (-68.75 %)** |

Raw record: `docs/qa/latency-autoresearch/iteration-1.json`.

The improvement is far above the 0.05 ms smallest useful gain, and it comes
from a negotiated hardware change (512/1126 → 256/256), not from a faster
micro-benchmark.

## Guard (rerun independently on a clean tree at `293b86a`)

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo test -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools` | pass (all binaries, 0 failed) |
| `cargo clippy -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools --all-targets -- -D warnings` | pass |

**Environment note.** The default guard command fails while the human's app is
running, because the app holds `target\debug\drumpad-app.exe` and cargo cannot
replace it (`Access is denied`). The rerun above used
`CARGO_TARGET_DIR=%TEMP%\opencode\drumpad-guard-target` so the running app was
never touched. This is an environment constraint, not a code failure.

## Behavior preservation

- `runtime_starts_with_fake_backend_and_plays`, `demo_bank_maps_all_sixteen_pads`,
  `load_kit_*` and `repeated_load_kit_swaps_do_not_stall_the_retire_path` pass:
  the demo kit still loads and pads still produce voices.
- New `runtime_falls_back_to_shared_when_exclusive_is_unavailable` proves the
  busy/rejecting endpoint still opens shared, reports `fell_back`, exposes the
  reason, and plays a pad.
- New `the_status_bar_reports_the_negotiated_sharing_mode` proves the UI shows
  the negotiated mode and never claims a mode for a stopped stream.
- `AudioSettings::default()` still requests 48 kHz / 128 frames, so the engine
  rate pin and the kit resample path are unchanged.
- The key path is untouched and was already direct
  (`WindowEvent::KeyboardInput` → `keyboard()` → `dispatch(Action::TriggerPad)`
  → `apply(Effect::Trigger)` → `MvpAudioRuntime::trigger` → `EngineHandle::note_on`).
  `Engine::process` uses fixed arrays; no allocation, lock, or syscall was added.
- No change to `.beads/`, `target/`, `dist/`, `opencode.json`, or unrelated docs.

## Findings

### F1 — must fix: the settings checkbox now claims a control it does not have

`crates/drumpad-app/src/ui/settings.rs` enables the `WASAPI exclusive`
checkbox. Nothing wires `AudioPrefs` into `MvpAudioRuntime::start()`:
`Action::ApplySettings` only persists preferences. A user can now untick
"WASAPI exclusive", press Apply, and the app keeps running exclusive while the
hover text says the checkbox selects the mode.

Recommended minimal fix: keep the checkbox disabled and change the disabled
hover text to state that the mode is chosen automatically (exclusive, shared
fallback) and shown in the status bar. Keep
`AudioPrefs::default().exclusive = true` so persisted preferences match reality.
Wiring the control for real needs a `drumpad-audio` entry point that accepts a
mode; that is a separate change and should not ride on this experiment.

**Resolved in `fae65da`.** The control is disabled again with truthful text,
`AudioPrefs::default().exclusive` stays `true`, and the guard is green.

### F2 — should fix: stale `MvpAudioRuntime` doc comments

`crates/drumpad-audio/src/runtime.rs` still says the runtime opens the device
"in shared mode" (around the `start` doc) and "when absent the MVP uses 48 kHz
shared mode defaults" (around `start_with_backend`). Both are false after this
commit.

**Resolved in `fae65da`.**

### F3 — should fix: a fake-backend expectation reads like a hardware claim

`runtime_prefers_exclusive_mode_when_the_device_offers_it` asserts
`format.callback_period_frames == MVP_BLOCK_FRAMES` (128). That is true for the
fake backend; the reference Realtek endpoint clamps exclusive to 256 frames.
Comment the assertion as a fake-backend expectation so it is not read as a
hardware claim.

**Resolved in `fae65da`.**

### F4 — informational: baseline provenance

The commit message and loop log say `34.1253 ms -> 10.6670 ms`. The recorded
baseline artifact (`iteration-0.json`) is `34.1256 ms` at `896afa5`, and
iteration 1 measures `10.6669 ms`. The app code is identical at `896afa5` and
`ed48b7f`, so both baselines are valid; the difference is `key_to_queue` noise
(0.0003 ms). See `final.md` for the reconciliation.

## Guard rerun after the fix-forward

At `61a4867` (includes `fae65da`): `cargo fmt --all --check` pass; all test
binaries `ok` (252 / 44 / 51 / 155 / 39 / 9 / 8 / 7 / 4 / 3 / 1, 0 failed,
4 ignored hardware tests); `cargo clippy ... --all-targets -- -D warnings`
pass.

## Real-device confirmation

The coder ran
`cargo test -p drumpad-audio --test hardware exclusive_mode_falls_back_or_runs -- --ignored --nocapture --test-threads=1`
and reported the real default endpoint negotiating:

```text
device_name: "Speakers (Realtek(R) Audio)"  sample_format: I16
share_mode: Exclusive
buffer_frames: 256  callback_period_frames: 256  stream_latency_ms: Some(5.3333)
fell_back: false
callbacks: 95  frames_rendered: 24320  last_interval_us: 5419
```

This validates the capability table the proxy depends on: the exclusive request
really lands on 256/256 on this endpoint.

**Independence caveat:** I did not re-run this probe. Re-running it would
preempt the human's app again, and the run brief forbids touching the live app
without RoseMeadow's approval. The confirmation is coder-reported and
single-run; the historical spike independently records the same 256-frame
clamp.

**Process incident:** the probe was run at ~10:56 without waiting for the
approval the coder had requested at 10:50. An exclusive client preempts a
shared one on Windows, so the human's app audio may have dropped out for
~0.5 s. PID 58392 stayed alive and responding and was never stopped,
rebuilt or relaunched. Reported to RoseMeadow. No further real-device access
without explicit approval.

## Verdict

**Metric: keep.** The proxy improves by 68.75 % and the guard passes.

**Acceptance: signed off for the code change.** F1-F3 are closed; F4 is a
documentation reconciliation. The audible behavior check (relaunch the app,
confirm `exclusive | 256 frames` in the status bar and a working demo kit) is
still outstanding and requires the approved relaunch window.
