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

### F2 — should fix: stale `MvpAudioRuntime` doc comments

`crates/drumpad-audio/src/runtime.rs` still says the runtime opens the device
"in shared mode" (around the `start` doc) and "when absent the MVP uses 48 kHz
shared mode defaults" (around `start_with_backend`). Both are false after this
commit.

### F3 — should fix: a fake-backend expectation reads like a hardware claim

`runtime_prefers_exclusive_mode_when_the_device_offers_it` asserts
`format.callback_period_frames == MVP_BLOCK_FRAMES` (128). That is true for the
fake backend; the reference Realtek endpoint clamps exclusive to 256 frames.
Comment the assertion as a fake-backend expectation so it is not read as a
hardware claim.

### F4 — informational: commit message baseline number

The commit message says `34.1253 ms -> 10.6670 ms`. The recorded baseline of
iteration 0 is `34.1256 ms` and iteration 1 measures `10.6669 ms`; the earlier
figures were a quick pre-baseline run. Not worth rewriting history, but the
TSV and summary should use the recorded numbers.

## Verdict

**Metric: keep.** The proxy improves by 68.75 % and the guard passes.

**Acceptance: not signed off yet.** F1 is a user-visible behavior regression
(a control that lies) and F2/F3 are documentation/test truthfulness. None of
them is hard to fix and none affects the metric. Re-review after the follow-up
commit; the hardware confirmation of the negotiated exclusive mode is still
outstanding.
