# MVP smoke checklist

Owner: `drumpad-qa`. This is the shortest human-runnable path that proves the
MVP makes sound, responds to the reference keyboard, and exposes enough
diagnostics to debug a failure. It is deliberately manual: no UI automation and
no compatibility matrix.

Record one row per run in the table at the end. A failure is a blocker for the
owning team, not something QA patches in place.

## Before you start

- Windows 11 x64, wired keyboard or the Finding 35-key keypad.
- Alesis Nitro Max connected over USB-MIDI with **Local Mode OFF**, or no MIDI
  device at all for the keyboard-only pass.
- PC onboard 3.5 mm output connected to the Nitro Max Aux In (or headphones for
  a quick check).
- No exclusive-mode or ASIO setting; v1 starts in WASAPI shared mode at 48 kHz,
  128 frames.

## Run

1. Build and launch from the repository root:

   ```powershell
   cargo run -p drumpad-app
   ```

   Expected: the window opens without a console error and paints within a few
   seconds.

2. **Window and grid.** The window opens with a 4x4 pad grid. Every pad shows a
   name and its key label. The status bar shows the active kit name, the
   keyboard profile, and the selected pad.

3. **Demo kit and audio.** The kit name is the demo/factory kit. Hit a pad with
   the mouse: it flashes and makes a sound. If the kit is silent, note whether
   the demo kit is installed and what the status bar says.

4. **Keyboard pads.** In order, press `1 2 3 4`, `Q W E R`, `A S D F`,
   `Z X C V`. Each key flashes and plays its pad. A key press must not be
   swallowed by the UI and a fast 8-hit roll must not drop hits.

5. **Panic.** Hold a loop or a long sample and press `F1`. All voices stop with
   a short fade; no click and no stuck voice. The status bar must not show the
   metronome as running after panic.

6. **Map overlay.** Press `F5`. The keyboard map overlay appears and lists the
   active profile. Press `F5` again: the overlay closes. `Escape` also closes
   it.

7. **Devices visible.** Open Settings (or the diagnostics block in the status
   bar). Confirm the active audio device name, sample rate, buffer size, and
   the MIDI device (or "none"). The values must match what Windows reports.

8. **Clean exit.** Close the window with the title-bar close button and then
   press `Alt+F4` on a second launch. The process exits within two seconds, the
   console has no panic/backtrace, and no audio device stays held (a second
   launch opens the device without a restart).

9. **Diagnostics.** Note the default device, buffer, exclusive/shared mode, and
   any xrun or callback-load line the app prints. If the app has a diagnostics
   view, capture it; otherwise copy the console output.

## Pass criteria

All of the following must be true:

- [ ] `cargo run -p drumpad-app` launches without an error dialog or panic.
- [ ] The 4x4 grid paints and the kit name, profile, and selected pad are
      visible.
- [ ] All 16 keyboard keys flash and play; no dropped fast hits.
- [ ] `F1` stops all voices without a click or stuck note.
- [ ] `F5` opens and closes the map overlay.
- [ ] The active audio device, sample rate, and buffer are visible.
- [ ] Closing the window exits cleanly, and a relaunch reopens the device.
- [ ] The run is recorded below with the default device and buffer.

## Failure routing

| Symptom | Owning team | Bead area |
| --- | --- | --- |
| Window does not open, panic on launch, exit hangs | app shell / UI | E08 |
| Grid or overlay does not paint, key labels wrong | UI | E08 |
| No sound, wrong device, shared-mode failure, xruns | audio backend | E02 |
| Keys do not flash/play, dropped hits, F1/F5 not routed | input / app | E06 |
| Demo kit missing or silent | model/samples | E05 |
| Engine clicks, stuck voices, wrong pad plays | audio engine | E03 |

Post the failing step, the console output, the default device, the buffer, and
the owning team on thread `TEAM-drumpad-qa-20261107124803-9638`.

## Run record

| Date | Build / commit | Default device | Mode / rate / buffer | Result | Blockers (owner) | Operator |
| --- | --- | --- | --- | --- | --- | --- |
| pending | pending | pending | pending | pending | pending | pending |

## Known limitations

- The physical Nitro Max and Finding keypad rows stay `pending` until the
  reference hardware is attached; the keyboard-only pass is valid but does not
  prove the Aux In chain.
- This checklist does not replace the analog latency protocol
  (`docs/qa/latency-protocol.md`) or the compatibility matrix.
- The headless counterpart of this checklist is
  `crates/drumpad-tools/tests/mvp_effects.rs` (trigger/release/panic/overlay
  effects, 16-pad coverage, status line, default audio format).
