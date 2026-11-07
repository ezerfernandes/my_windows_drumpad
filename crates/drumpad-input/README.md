# drumpad-input

Keyboard, single-hand keypad, and USB-MIDI capture for Drumpad. This crate owns
the source-agnostic trigger vocabulary (`TriggerEvent`, `ReleaseEvent`,
`TriggerSource`), the lock-free queue the audio callback drains, keyboard
profiles and the mapping editor model, MIDI parsing and routing, the Nitro Max
hi-hat engine, and the hotplug/latency diagnostics that surround them.

Nothing here depends on egui, winit, cpal, or the audio callbacks. The app
converts platform key codes to `PhysicalKey` at the edge; the engine consumes
`InputEvent` from the queue.

## Shared types

`TriggerEvent`, `ReleaseEvent`, and `InputEvent` use the canonical
`drumpad_model::ids::{PadId, Velocity}` types, re-exported from
`drumpad_input::trigger`. Input does not define a second pad or velocity
identity. `PadIdExt`/`VelocityExt` add capture-side helpers (`PadId::ALL`,
`PadId::from_index`, `Velocity::from_midi`, `Velocity::clamp_signed`); use
`trigger::pad_label` for one-based user-facing labels, because the model's
`Display` is the raw zero-based wire value.

The typed `keyboard::PhysicalKey` enum is the input-side view. `to_model`,
`from_model`, and the `From` impls convert it losslessly to
`drumpad_model::PhysicalKey` (PC scan code) for kit persistence.
`KeyboardProfile::to_model_profile` and `from_model_profile` preserve pad
bindings when a profile travels with a kit; transport actions and the explicit
unmapped list stay input-side metadata.

## Default keyboard map

The 4x4 pad grid mirrors the physical block:

```text
+---+---+---+---+
| 1 | 2 | 3 | 4 |     pad  1   2   3   4
+---+---+---+---+
| Q | W | E | R |     pad  5   6   7   8
+---+---+---+---+
| A | S | D | F |     pad  9  10  11  12
+---+---+---+---+
| Z | X | C | V |     pad 13  14  15  16
+---+---+---+---+
```

| Key | Action |
| --- | --- |
| `1 2 3 4` / `Q W E R` / `A S D F` / `Z X C V` | 16 pads, row major, index 0-15 |
| `F1` | Panic / stop all |
| `F2` | Metronome toggle |
| `F3` | Previous kit |
| `F4` | Next kit |
| `F5` | Keyboard map overlay |
| `5` `T` `G` `B` `Space` | Unmapped |

Keys are identified by `PhysicalKey` (scan codes), never by characters, so an
AZERTY, Dvorak, or JIS layout still plays the same physical grid. `Map`/`Alt`
combinations are not treated as pads; the app forwards only unmodified key
events.

### Finding 35-key single-hand keypad

Reference hardware: Finding 35-key single-hand USB keypad, ASIN B09HF5WDR2.
Its QWERTY block contains the same 4x4 grid, so the default mapping carries
over unchanged. `F1`-`F5` are the transport row, the fifth column and `Space`
stay unmapped, and the left macro column may only control lighting, so no
app behavior depends on it.

Key rollover and ghosting: the keypad must report at least 6-key rollover for
chords; validate a specific unit with the procedure in
[`docs/keyboard-input.md`](docs/keyboard-input.md). The capture path keeps every
simultaneously held key independent, so a keyboard that reports the chord is
never the reason a hit drops.

## Profiles

`KeyboardProfile` is versioned JSON and stores physical keys plus actions.
`ProfileLibrary` holds the full-keyboard and Finding 35-key profiles as one
document, so a profile set can travel with a kit.

- `KeyboardProfile::default_full_keyboard()`
- `KeyboardProfile::finding_35_key()`
- `KeyboardProfile::conflicts()` reports duplicate keys, pads, and transport
  actions before a profile goes live.
- `MappingEditor` implements arm-pad, capture-next-key, reject-or-confirm
  conflicts, reset to defaults.
- `KeyMap::new` validates and sorts a profile for allocation-free lookup; the
  app swaps a new `KeyMap` in without restarting.

## Keyboard and MIDI events
```rust
use drumpad_input::keyboard::{KeyboardInput, KeyboardProfile};
use drumpad_input::trigger::event_queue;

let (producer, consumer) = event_queue(1024);
let mut keyboard = KeyboardInput::default_with_profile(
    &KeyboardProfile::finding_35_key(),
);

let action = keyboard.key_down(
    drumpad_input::keyboard::PhysicalKey::Digit1,
    0,
);
if let Some(event) = action.event() {
    producer.push(event).ok();
}
let _event = consumer.pop();
```

Auto-repeat is suppressed until key up, key up produces a gate release, and
`focus_lost`/`release_all` clears every held key so alt-tab, lock screen, or a
missed key-up cannot leave a voice stuck.

## MIDI and the Alesis Nitro Max

- `MidiParser` handles running status, splits across chunks, ignores system
  realtime bytes, and treats note-on velocity zero as note-off.
- `MidiInputBackend` wraps midir (`MidirBackend`, feature `midir-backend`,
  default on) with a `FakeMidiBackend` for tests and hardware-free CI.
- `MidiRouter` applies the channel filter, MIDI learn, velocity curves, the
  note map and the hi-hat engine, then emits the same `InputEvent` types as the
  keyboard.
- `MidiNoteMap::nitro_max_template()` covers every Nitro Max pad and zone:
  kick 36, snare 38/40, toms 48/50, 45/47, 43/58, 41/39, ride 51, crash 49/57,
  hi-hat 46/23/42/44/21.
- `HiHatEngine` supports the five discrete hi-hat notes plus continuous CC4 and
  pedal CC64 with hysteresis, pedal chick and splash.
- `HotplugManager` reconnects the saved device by name, and
  `MidiRouter::release_all` clears held notes on unplug.
- `MidiMonitor` is a bounded, pausable log for the Settings screen;
  `MidiLatencyTracker` reports per-device latency and jitter percentiles.

Setup, gain staging, Local Mode guidance and the full note map are in
[`docs/nitro-max.md`](docs/nitro-max.md). Keyboard spike and rollover notes are
in [`docs/keyboard-input.md`](docs/keyboard-input.md).

## Verification

```sh
cargo test -p drumpad-input
cargo clippy -p drumpad-input --all-targets -- -D warnings
cargo fmt -p drumpad-input -- --check
cargo run -p drumpad-input --release --example keyboard_latency_spike
```

The last command measures the in-process capture path (key lookup, velocity,
event construction, queue push) and prints p50/p95/p99. The OS-to-handler leg
of the 2 ms keyboard budget must be measured on the reference hardware; see
`docs/keyboard-input.md`.
