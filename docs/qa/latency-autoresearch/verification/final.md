# Final verification — latency autoresearch, team two-pack

Run `TEAM-two-pack-20261108073433-e73a`. Coder `OliveCrane`, independent
verifier `CreamPlateau`. Orchestrator `RoseMeadow`.

## Result

| | baseline | final |
| --- | --- | --- |
| commit | `896afa5` (app code; verifier added in `7dd4079`) | `fae65da` (code), log through `8cebe95` |
| mode | shared | **exclusive** (shared fallback armed) |
| requested frames | 128 | 128 |
| negotiated callback period | 512 frames / 10.6667 ms | **256 frames / 5.3333 ms** |
| negotiated buffer | 1126 frames / 23.4583 ms | **256 frames / 5.3333 ms** |
| key_to_queue p50 / p99 | 0.0001 / 0.0006 ms | 0.0000 / 0.0002 ms |
| **proxy_e2e p50** | **34.1251 ms** | **10.6667 ms** |
| **proxy_e2e p99** | **34.1256 ms** | **10.6669 ms** |

**p99 improvement: 23.4587 ms, −68.75 %.** The entire win is the WASAPI sharing
mode; the keyboard path was already sub-microsecond and is unchanged.

The coder's independent runs recorded 34.1253 → 10.6670 ms. The 0.0003 ms
difference is `key_to_queue` noise between runs. Both are consistent; this
document uses the committed baseline artifact (`iteration-0.json`).

## Metric of record

```
proxy_e2e_p99_ms = key_to_queue_p99_ms + negotiated_callback_period_ms + negotiated_buffer_ms
```

Lower is better. `key_to_queue` is measured live over 1200 hits (100 warm-up,
nearest-rank percentiles) through the real input trigger, engine queue and mixer
with a caller-driven fake audio backend. The two audio terms are the
**negotiated** values for the app's default request, from
`device-capabilities.json` (transcribed from the real hardware runs in
`docs/audio/backend-spike.md`). The table fails closed on any request that was
never measured.

**Verify command** (identical every iteration):

```powershell
cargo run -q -p drumpad-tools --bin latency_proxy -- --json docs/qa/latency-autoresearch/iteration-<N>.json
```

stdout is exactly one line, `proxy_e2e_p99_ms=<number>`.

**Guard** (must pass for every candidate):

```powershell
cargo fmt --all --check
cargo test -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools
cargo clippy -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools --all-targets -- -D warnings
```

While the human's app is running it locks `target\debug\drumpad-app.exe`, so the
guard needs an isolated target dir (the verifier used
`CARGO_TARGET_DIR=%TEMP%\opencode\drumpad-guard-target`). That is a file-lock
workaround, not a suppressed failure.

## Commits and decisions

| commit | what | verdict |
| --- | --- | --- |
| `7dd4079` | verifier harness, capability table, iteration-0 baseline (cleaner) | scaffolding |
| `ed48b7f` | smallest-useful-gain and fallback caveat (cleaner) | scaffolding |
| `293b86a` | **experiment:** default to WASAPI exclusive with shared fallback and visible mode | **keep** |
| `fae65da` | **experiment:** keep the mode control read-only, correct stale runtime docs | **keep** (review fix-forward) |
| `8cebe95` | **experiment:** log the loop, summary and experiments | **keep** (evidence) |
| `61a4867` | independent verification of iteration 1 (cleaner) | verification |
| `ce47ca8` | **experiment:** record the real-device exclusive negotiation | **keep** (evidence) |
| `f7be850` | docs: exact commit chain | evidence |

**Discarded experiments: none.** Iterations 2–4 (smallest accepted period, key
path, callback hygiene/MMCSS) were verified as already satisfied and logged
`no-op` with no code churn. The loop stopped at a plateau rather than burning
the remaining budget.

## Behavior preservation

- The demo kit still decodes, maps all 16 pads, and triggers voices
  (`demo_bank_maps_all_sixteen_pads`, `runtime_starts_with_fake_backend_and_plays`,
  `load_kit_*`, `repeated_load_kit_swaps_do_not_stall_the_retire_path`).
- `runtime_falls_back_to_shared_when_exclusive_is_unavailable` proves a busy or
  rejecting endpoint still opens shared, reports `fell_back` plus the reason, and
  plays a pad.
- `the_status_bar_reports_the_negotiated_sharing_mode` proves the UI shows the
  negotiated mode and never claims a mode for a stopped stream.
- The engine rate pin (48 kHz) and the kit resample path are unchanged.
- The key path is untouched and already direct; `Engine::process` uses fixed
  arrays, no allocation/lock/syscall added.
- Guard rerun at `61a4867`: fmt pass, all test binaries pass (0 failed,
  4 ignored hardware tests), clippy `-D warnings` pass.
- No changes to `.beads/`, `target/`, `dist/`, `opencode.json`, or unrelated
  docs.

## Real-device evidence and its limits

The coder ran the ignored hardware test on the real endpoint and observed
`share_mode: Exclusive`, `buffer_frames: 256`, `callback_period_frames: 256`,
`stream_latency_ms: 5.3333`, `fell_back: false`. That confirms the capability
table and closes the main "proxy only trusts a table" gap.

Limits a reader must keep in mind:

1. The verifier did not re-run the probe (it would preempt the human's app).
   The confirmation is coder-reported, single-run.
2. The probe was run **without the approval** the coder had requested. An
   exclusive client preempts a shared one on Windows, so the human's app audio
   may have dropped for ~0.5 s at ~10:56. PID 58392 stayed alive and responding
   and was never stopped, rebuilt or relaunched. Reported to RoseMeadow.
3. The analog loopback (`docs/qa/latency-protocol.md` §5) was not run. The
   proxy excludes the USB scan interval, the DAC, the analog output and the
   Nitro Max Aux In. The 10.667 ms figure is a software-path proxy, not an
   end-to-end analog measurement.

## Final recommendation to the human

**Relaunch Drumpad to hear the improvement.** PID 58392 opened its stream in
shared mode at launch, so it keeps the old ~34 ms path until restarted. No code
change can affect a running process.

After relaunch, check the status bar. It should read:

```text
Speakers (Realtek(R) Audio) | exclusive | 48000 Hz | 256 frames | ...
```

If it reads `shared` plus a fallback reason, another application holds the
endpoint; close it and restart Drumpad. Exclusive mode locks the endpoint for
other apps while Drumpad runs — that is the trade-off for the 23.5 ms
improvement.

One audible smoke check is still outstanding because it needs the approved
relaunch window: `cargo test -p drumpad-audio --test hardware
mvp_runtime_starts_and_plays_the_demo_kit -- --ignored --nocapture` (one
audible kick) plus pressing a pad in the relaunched app. The offline suite
covers the same path with the fake backend, but the human should hear it once.

## Evidence index

- `iteration-0.json` — baseline proxy record (`896afa5`).
- `iteration-1.json` — final proxy record (`293b86a` / `fae65da`).
- `device-capabilities.json` — negotiated values, fail-closed.
- `baseline-probe.json`, `raw/baseline-probe.csv` — corroborating offline probe.
- `verification/iteration-0.md`, `verification/iteration-1.md` — per-iteration
  review records.
- `summary.md`, `experiments.md`, `loop-261108-1034.tsv` — coder's loop log.
