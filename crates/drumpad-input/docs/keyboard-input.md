# Keyboard and keypad input: spike, policy, and verification

This note covers E06-T03/T04/T07: the keyboard latency spike, the chosen capture
path, global hotkey policy, rollover/ghosting validation for the Finding 35-key
keypad, and the harness that reports percentiles.

## Candidates

The design doc names three candidate paths. They differ mainly in where the
event is turned into a `TriggerEvent`.

| Path | Where the event is consumed | Latency floor | Verdict |
| --- | --- | --- | --- |
| eframe/egui per-frame input | next `App::update` | 0-16.7 ms at 60 Hz, plus frame jitter | Fails the 2 ms budget; fine for menus only |
| Custom winit event-loop hook | `WindowEvent::KeyboardInput` handler, forwarded immediately | sub-millisecond on the app side | **Chosen** |
| Windows low-level hook (`WH_KEYBOARD_LL`) | system-wide hook callback | sub-millisecond but adds hook-chain and message-pump delay | Opt-in fallback only |

The eframe path cannot meet the budget because egui consumes events once per
frame. A 60 Hz frame is 16.7 ms, which is larger than the entire end-to-end
target. A custom winit loop receives `WM_KEYDOWN` in the window procedure and
can push to the input queue before the next egui frame is built. The low-level
hook is technically fast, but it captures keys system-wide, requires explicit
user consent and a permanent message pump, and can interfere with other apps.
It stays behind an explicit opt-in and is never the default.

The decision is recorded as a draft ADR in
[`adr/0001-keyboard-latency-path.md`](adr/0001-keyboard-latency-path.md) for the
foundation pair to publish under `docs/adr/` (shared docs belong to that pair).

## What the crate measures

`examples/keyboard_latency_spike.rs` measures the software leg this crate owns:
`PhysicalKey` lookup, velocity generation, `TriggerEvent` construction, and the
lock-free push into the audio queue. It runs 100,000 pad events followed by
20,000 fast down/up taps, drains the queue, and reports p50/p90/p95/p99, missed
taps, and queue drops.

Measured on the development machine (Windows 11 x64, Rust 1.98, release,
integrated keyboard, not the reference keyboard or keypad):

| p50 | p90 | p95 | p99 | max | missed taps | queue drops |
| --- | --- | --- | --- | --- | --- | --- |
| 0 us | 1 us | 1 us | 1 us | 18 us | 0 | 0 |

Sub-microsecond samples round to 0 us because the recorder stores whole
microseconds. The point of the result is that the in-process leg is roughly
three orders of magnitude below the 2 ms budget; the remaining budget belongs
to the OS, the window message pump, and the audio callback, which must be
measured on the reference hardware.

## Reference-hardware measurement protocol

Record one row per device: full keyboard and Finding 35-key keypad
(B09HF5WDR2).

1. Build release: `cargo build -p drumpad-input --release --examples`.
2. Run the app shell with the chosen winit hook and the app's key-to-callback
   harness enabled. The app stamps `MonotonicClock::now_us()` when the
   `KeyboardInput` handler starts and the audio callback records
   `now_us() - event.timestamp_us` when it drains the event.
3. Play 5 minutes of normal drumming plus 200 deliberate fast double taps per
   device.
4. Report p50, p95, and p99 from `LatencyRecorder`, plus the number of events
   the queue reported dropped and the number of taps that produced no trigger.
5. Store the row below with hardware details and date.

| Device | OS build | p50 | p95 | p99 | Drops | Missed taps | Date |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Full keyboard (model/TBD) | Windows 11 x64 | pending | pending | pending | pending | pending | |
| Finding 35-key keypad (B09HF5WDR2) | Windows 11 x64 | pending | pending | pending | pending | pending | |

The gate is p99 < 2000 us and zero missed taps. If the winit path misses the
gate, enable the low-level hook as an explicit fallback and repeat the run; if
that also misses, revisit the audio buffer size in the audio pair's spike.

## Rollover and ghosting validation (Finding 35-key keypad)

The capture path keeps every key the OS reports independent and never
synthesizes a release for a key that is still down. A dropped chord can
therefore only come from the keypad's own matrix. Validate the physical unit:

1. Open a key-tester view (the app's map overlay highlights keys as they are
   pressed) or any keyboard rollover tester.
2. Press the sixteen pad keys one at a time; confirm every hit registers.
3. Press adjacent pairs across rows (`1`+`Q`, `Q`+`A`, `A`+`Z`, `4`+`R`, and so
   on), then triples and four-note chords across rows.
4. Confirm the overlay shows all keys of each chord simultaneously. If a key
   disappears, note the chord and the missing key; that is a ghosting limit of
   the unit, not of the app.
5. Document the result for the unit in use. If the keypad is 6-key rollover,
   avoid chords larger than six keys; drumming rarely needs more than four.

## Focus loss and global capture policy

- `KeyboardInput::focus_lost(timestamp)` clears every held key and returns a
  `ReleaseEvent` per held pad. The app forwards those releases and additionally
  requests stop-all, so alt-tab, minimize, lock screen, or a missed key-up
  never leaves a gated voice ringing.
- Global capture is **off by default**. Drumpad only receives keys while its
  window is focused. The low-level hook exists as an explicit, documented
  opt-in for stage use and must be surfaced in Settings with the privacy and
  interference tradeoff; no code path enables it implicitly.
- Rebinding, profile reloads, and device changes go through
  `KeyboardInput::replace_map`, which releases held keys before the swap.

## Verification commands

```sh
cargo test -p drumpad-input
cargo clippy -p drumpad-input --all-targets -- -D warnings
cargo fmt -p drumpad-input -- --check
cargo run -p drumpad-input --release --example keyboard_latency_spike
```
