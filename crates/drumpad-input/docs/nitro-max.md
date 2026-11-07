# Alesis Nitro Max integration

Reference kit: Alesis Nitro Max over USB-MIDI. The USB port sends MIDI only; it
does not carry audio. Drumpad is the sound source and the mix returns to the
module through the 3.5 mm Aux In.

## Signal chain

```text
Nitro Max pads
      |
      v
USB-MIDI  ->  PC running Drumpad  ->  PC onboard 3.5 mm output
                                          |
                                          v
                              Nitro Max Aux In (3.5 mm stereo)
                                          |
                                          v
                              Nitro Max phones / line outputs
```

- Set **Local Mode OFF**. The module then sends MIDI to the computer and
  bypasses its internal sounds, so Drumpad is the only drum source. This avoids
  flam between module sounds and app samples.
- With Local Mode ON, the module mixes its internal sounds with the Aux In
  audio. That is useful for layering percussion on the kit's own sounds, but
  the app path may arrive a few milliseconds later, so treat it as an
  intentional layering mode and test for flam.
- The Aux In has no separate gain control. Set the PC output level and the
  module master volume so the app does not clip the module input.
- The Nitro Max is class-compliant; no vendor driver should be required. Use a
  short USB cable and a direct port if the module is not detected reliably.

## Note map (template `MidiNoteMap::nitro_max_template()`)

Every documented Nitro Max pad and zone is mapped. Head and rim share a pad so
a rim hit articulates with its drum; the five hi-hat notes share the hi-hat pad
(pad 10) and carry an articulation.

| Note | Zone | Pad | Articulation |
| --- | --- | --- | --- |
| 36 | Kick | 1 | |
| 38 | Snare | 2 | |
| 40 | Snare rim | 2 | |
| 48 | Tom 1 | 3 | |
| 50 | Tom 1 rim | 3 | |
| 45 | Tom 2 | 4 | |
| 47 | Tom 2 rim | 4 | |
| 43 | Tom 3 | 5 | |
| 58 | Tom 3 rim | 5 | |
| 41 | Tom 4 (optional expansion) | 6 | |
| 39 | Tom 4 rim (optional expansion) | 6 | |
| 51 | Ride | 7 | |
| 49 | Crash 1 | 8 | |
| 57 | Crash 2 (optional expansion) | 9 | |
| 46 | Hi-hat open | 10 | open |
| 23 | Hi-hat half-open | 10 | half-open |
| 42 | Hi-hat closed | 10 | closed |
| 44 | Hi-hat pedal | 10 | pedal (chick) |
| 21 | Hi-hat splash | 10 | splash |

Pads 11-16 stay free for user samples. The map is editable: `MidiNoteMap`
exports/imports JSON, and MIDI learn captures a note or CC for a pad with an
explicit conflict policy.

## Hi-hat engine

`HiHatEngine` normalizes discrete notes and continuous pedals:

- Notes select the articulation directly (table above). Note 44 produces a
  pedal chick, note 21 a splash.
- CC4 controls continuous position when the module sends it. The default
  calibration is closed at or below 32, open at or above 96, with 8 counts of
  hysteresis, so a pedal resting on a threshold does not chatter.
- CC64 acts as a pedal switch (>= 64 is down). Closing emits one pedal chick;
  lifting the pedal emits nothing. Repeated values and bounces do not
  retrigger.
- `HiHatConfig` exposes the note numbers, thresholds, hysteresis, CC switches
  and CC velocity. `HiHatEngine::control_change` returns a trigger only when
  the articulation actually changes.
- The pad event carries `pad = hi-hat pad`; the `HiHat` router output also
  carries `articulation` and `sound` so the engine can choose the layer.

## Local Mode and gain staging checklist

- [ ] Nitro Max Local Mode is OFF for the app-only profile.
- [ ] Module master volume starts low; raise after the PC output is set.
- [ ] PC output is the onboard 3.5 mm jack (fallback: USB audio interface).
- [ ] Play the loudest kit at full velocity; the module input must not clip.
- [ ] If the module clips, lower the PC output first, then the app master gain.
- [ ] Confirm the module phones/line outputs carry the app mix only (Local Mode
      OFF) or an intentional layer (Local Mode ON).
- [ ] Test for flam with Local Mode ON by playing one pad and listening for a
      doubled transient.

## Latency and flam

Targets from the design: Nitro Max USB-MIDI input to audio callback under one
audio block, and full chain under 10 ms at 48 kHz / 128 frames. Measure with
the QA loopback protocol (E11) and record:

| Chain | p50 | p95 | p99 | Buffer | Date |
| --- | --- | --- | --- | --- | --- |
| Nitro Max note -> queue -> callback | pending | pending | pending | 128 @ 48 kHz | |
| Full chain (MIDI -> onboard 3.5 mm -> Aux In) | pending | pending | pending | 128 @ 48 kHz | |

`MidiLatencyTracker` reports per-device callback-to-queue latency and jitter
from midir timestamps. A physical loopback is still required for the analog leg.

## Hotplug and troubleshooting

- `HotplugManager` polls ports from the UI tick, closes a vanished connection,
  and reopens the saved device by name. The selection persists in
  `MidiPreferences` JSON.
- On disconnect the app must call `MidiRouter::release_all(now)` so no gated
  voice is stranded, then keep the keyboard path live; no restart is needed.
- No ports listed: check the cable and port, close other MIDI apps that may
  hold the device exclusively, and confirm the module is in USB-MIDI mode.
- Notes trigger the wrong pad: compare the incoming note in the MIDI monitor
  against the table above, then remap with MIDI learn.
- Hi-hat chatters: raise `HiHatConfig::hysteresis` or recalibrate
  `closed_threshold`/`open_threshold` for the module.
- Nothing audible but notes arriving: check Local Mode, the Aux In cable, the
  PC output device, and the module master volume in that order.

## Verification status

- Template mapping, note-off/velocity-zero, channel filtering, CC4/CC64
  hysteresis, pedal chick/splash, hotplug/reconnect, and monitor behavior are
  covered by `cargo test -p drumpad-input` with the fake backend.
- Physical verification requires a Nitro Max on the reference Windows 11
  machine: confirm the note numbers in the monitor, Local Mode OFF, Aux In
  monitoring, no clipping, no flam, and record the latency table above.
