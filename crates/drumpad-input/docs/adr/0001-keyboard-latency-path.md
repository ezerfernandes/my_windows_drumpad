# ADR 0001: Keyboard capture path

- Status: Proposed (draft for `docs/adr/`; shared docs are owned by the
  foundation pair)
- Date: 2026-11-07
- Related: E06-T03, E06-T04, E06-T07, design doc §4.6 and §6

## Context

Drumpad must turn a key press into a `TriggerEvent` in the audio queue in under
2 ms on a normal keyboard and on the Finding 35-key single-hand keypad. The UI
is winit plus egui. Three capture paths were considered:

1. **eframe/egui per-frame input.** egui consumes events once per frame. At
   60 Hz that is up to 16.7 ms, plus frame jitter, before anything reaches the
   engine. This cannot meet the budget, though it remains fine for menus and
   text fields.
2. **Custom winit event-loop hook.** Install a winit application handler and
   forward `WindowEvent::KeyboardInput` to `drumpad-input` as soon as it is
   delivered, before the next egui frame is built.
3. **Windows low-level keyboard hook (`WH_KEYBOARD_LL`).** A global hook with
   its own message pump. It can be fast, but it captures keys system-wide,
   requires explicit consent, can interact badly with other software, and adds
   its own hook-chain latency.

## Decision

Use the **custom winit event-loop hook** as the primary capture path. Convert
winit's physical `KeyCode` to `drumpad_input::keyboard::PhysicalKey` at the
edge, feed `KeyboardInput`, and push the resulting `InputEvent` into the
lock-free queue immediately. egui still receives the same winit events for
menus and text entry, but pad hits do not wait for a frame.

Keep the low-level hook as an explicit, opt-in fallback behind a settings flag
that is off by default. It is only considered if the reference-hardware
measurement shows the winit path missing p99 < 2 ms.

## Consequences

- Pad latency is decoupled from the UI frame rate.
- The app owns the platform key-code conversion; `drumpad-input` stays free of
  winit and can be tested without a window.
- Global capture stays off by default; no key is observed while Drumpad is not
  focused, which is the safer default for a stage machine.
- The low-level hook requires a separately tested code path and clear user
  disclosure if it is ever enabled.
- No layer caches per-frame keyboard state for performance, so a missed key-up
  must be handled explicitly: focus loss and `release_all` clear held keys.

## Evidence

- `examples/keyboard_latency_spike.rs` measures the in-process leg (lookup,
  velocity, event construction, queue push) at p99 = 1 us over 100,000 events
  and 20,000 fast taps on the development machine, with zero missed taps and
  zero queue drops.
- OS-to-handler and queue-to-callback legs still require the reference hardware
  run described in `docs/keyboard-input.md`; the results table there is the
  release gate.
- A frame-driven path cannot represent a 2 ms budget at any common refresh
  rate: one 60 Hz frame alone is 16.7 ms.
