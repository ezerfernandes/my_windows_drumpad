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
   name and its key label. On a first run the **Setup wizard** opens over the
   grid ("Step 1 of 6 — Welcome to Drumpad"); skip or finish it, then confirm
   the grid. The status bar shows `Kit: <name> | Audio: <device> | <rate> Hz |
   <frames> frames | <voices>` on the left and `Pad <n> | <profile>` on the
   right. **The whole window must be inside the screen work area** — the status
   bar is the MVP's only device readout.

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

7. **Devices visible.** Read the status bar: `Audio: <device> | <rate> Hz |
   <frames> frames`. If it says `Audio unavailable: <reason>`, record the
   reason. Open **Settings → Advanced → Show diagnostics** for the kit, load
   state, profile, frame counter, and version. The device and format must match
   Windows. A MIDI device name appears in Settings → MIDI, or `none`.

8. **Clean exit.** Close the window with the title-bar close button and then
   press `Alt+F4` on a second launch. The process exits within two seconds, the
   console has no panic/backtrace, and no audio device stays held (a second
   launch opens the device without a restart).

9. **Diagnostics.** Note the default device, buffer, exclusive/shared mode, and
   any xrun or callback-load line the app prints. If the app has a diagnostics
   view, capture it; otherwise copy the console output. The local log also
   records the stream start line:

   ```text
   MVP audio runtime started running=true rate=48000 format=Some(StreamFormat {
     device_name: "Speakers (Realtek(R) Audio)", share_mode: Shared,
     buffer_frames: ..., callback_period_frames: ... })
   ```

   Log file: `%LOCALAPPDATA%\Drumpad\Drumpad\logs\drumpad.log.<date>`.

## Pass criteria

All of the following must be true:

- [ ] `cargo run -p drumpad-app` launches without an error dialog or panic.
- [ ] **The window fits the screen work area**; nothing is clipped at 100 % and
      125 % display scaling.
- [ ] The 4x4 grid paints and the kit name, profile, and selected pad are
      visible.
- [ ] All 16 keyboard keys flash and play; no dropped fast hits.
- [ ] `F1` stops all voices without a click or stuck note.
- [ ] `F5` opens and closes the map overlay.
- [ ] The status bar shows the active audio device, sample rate, and buffer.
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
| 2026-11-07 | uncommitted dev build (`cargo build -p drumpad-app`, window title "Drumpad 0.1.0") | status bar off-screen; log has no device line | 48 kHz / 128 frames from defaults | **FAIL (window fit)** | window 1454x938 on a 1536x816 work area; bottom 173 px clipped, status bar not visible — app shell/UI (E08). Audio status could not be read. | MaroonCat |
| 2026-11-07 | `8458ca6` + local tree, separate target dir | `Speakers (Realtek(R) Audio)` from the log | shared / 48000 / buffer 1126, callback period 512 | **partial**: process launched and stayed in the window loop for 10 s; log shows `MVP audio runtime started running=true`, `share_mode: Shared`, no fallback, and earlier runs logged `window closed; shutting down` + `Drumpad closed cleanly`. Grid, audible pads, F1/F5, and window fit are not observable from the headless agent shell. | interactive rows and window fit need a human; `drumpad-app` fmt/clippy failures (UI/app) | MistyGrove (headless) |
| pending | pending | pending | pending | pending | pending | pending |

Headless counterpart on the same build:

```powershell
cargo test -p drumpad-tools --test mvp_effects
# 7 passed, 0 failed (trigger/release/panic/overlay effects, 16-pad sweep,
# status line, default 48 kHz/128-frame audio prefs)
```

## Known limitations

- **Window fit at 125 % scaling (MVP blocker, E08).** On the smoke machine the
  window opened at 1454x938 physical pixels while the work area is 1536x816
  (125 % display scaling, 1920x1080 panel). The bottom 173 px are off-screen,
  so the status bar — the only place the audio device and buffer are shown —
  cannot be read, and the bottom pad row is partially hidden. A human cannot
  complete step 7 until this is fixed. Owner: app shell/UI, E08.
- The physical Nitro Max and Finding keypad rows stay `pending` until the
  reference hardware is attached; the keyboard-only pass is valid but does not
  prove the Aux In chain.
- This checklist does not replace the analog latency protocol
  (`docs/qa/latency-protocol.md`) or the compatibility matrix.
- The headless counterpart of this checklist is
  `crates/drumpad-tools/tests/mvp_effects.rs` (trigger/release/panic/overlay
  effects, 16-pad coverage, status line, default audio format).
