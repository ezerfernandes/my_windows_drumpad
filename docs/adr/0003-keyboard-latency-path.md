# ADR 0003: Keyboard input and latency path

- Date: 2026-11-07
- Status: Accepted

## Context

The primary performance surfaces are a full keyboard and the Finding 35-key
single-hand USB keypad. Fast taps and chords must not be lost, auto-repeat must
not retrigger pads, and keyboard layout must not change which pad fires. The
budget is key-to-audio-queue under 2 ms.

## Decision

Handle keyboard input through physical key codes in a custom winit event-loop
path, not through egui's per-frame input. Key-down and key-up are separate
events; auto-repeat is suppressed; triggers are pushed to the lock-free audio
queue immediately. Use the default 4x4 map `1 2 3 4`, `Q W E R`, `A S D F`,
`Z X C V` and the F1-F5 transport row. Keep the mapping in per-device profiles.
Compare this path, eframe per-frame input, and a Windows low-level hook in the
keyboard latency spike (E06-T03), and adopt the low-level hook only if measured
p99 requires it.

## Alternatives considered

- **eframe / egui per-frame input**: simplest, but input is consumed once per
  frame, which adds up to a frame of latency and can reorder fast taps.
- **Windows low-level keyboard hook**: captures keys system-wide before the
  event loop, but global capture is a privacy and support risk and must be
  opt-in if it is ever used.
- **Raw Input**: good for multiple keyboards, but more plumbing than v1 needs.
- **Character events**: layout-dependent and wrong for a pad grid; physical key
  codes are the only correct choice for a one-hand keypad.

## Consequences

- The app must resolve pending triggers before rendering; the UI never owns the
  keyboard trigger path.
- Focus loss stops all voices, so alt-tab or lock cannot leave stuck notes.
- The Finding keypad's `5`, `T`, `G`, `B`, and `Space` stay unmapped to avoid
  accidental hits; only the QWERTY block and F1-F5 are used.
- The spike records p50 and p99 for both the full keyboard and the one-hand
  keypad, and the chosen path is documented in the input ADR when it lands.
