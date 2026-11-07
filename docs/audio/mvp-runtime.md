# MVP audio runtime

`drumpad_audio::MvpAudioRuntime` is the app-facing vertical-slice facade. It
decodes the embedded demo kit, resamples it to 48 kHz, builds the playback
engine, and opens the default Windows render device in shared mode.

## Start

```rust
use drumpad_audio::MvpAudioRuntime;

let mut audio = MvpAudioRuntime::start()?;
```

`start()` returns an error if the demo kit cannot be decoded or the default
device cannot open. The stream is already running when it returns.

## Play

```rust
// pad_index 0..=15 (clamped), velocity 1..=127 (clamped)
audio.trigger(0, 100);   // kick
audio.release(0);        // only matters for gate-mode pads
audio.panic();           // stop every voice
```

`trigger`/`release` use the canonical input event queue with
`TriggerSource::UiPreview`; the audio thread applies them sample-accurately
inside the next block.

## Status

```rust
audio.is_running();
audio.format();          // negotiated device, rate, buffer, latency
audio.state();           // Running / Reconnecting / Failed / Stopped
audio.snapshot();        // callbacks, xruns, load, buffer, period
audio.engine_snapshot(); // peak/RMS meters, active voices, voice limit
audio.pad_names();       // demo kit display names, indexed 0..16
```

Call `audio.tick()` from the UI at ~10 Hz to process audio-system events such
as device loss. Drain `audio.drain_events()` to show dialogs.

## Minimal app wiring

```rust
let mut audio = MvpAudioRuntime::start()
    .map_err(|error| format!("audio unavailable: {error}"))?;

// In the winit keyboard handler, map the physical key to a pad index:
//   1 2 3 4 / Q W E R / A S D F / Z X C V -> 0..15
match key {
    PadKey::Pad(index) => audio.trigger(index, 100),
    PadKey::PadRelease(index) => audio.release(index),
    PadKey::Panic => audio.panic(),
    PadKey::Other => {}
}
```

## Scope and limits

- **48 kHz shared mode** is used for the MVP so it never takes exclusive
  control of the endpoint. Exclusive mode remains available through
  `AudioSettings`/`StreamConfig` for the latency work (see
  [latency-guide.md](latency-guide.md)).
- The engine is built at 48 kHz. If a device negotiates a different rate, the
  runtime stops and returns an explicit error rather than playing at the wrong
  pitch.
- Only the first velocity layer of each demo pad is installed. Velocity
  layers, kit editing, and per-pad sample replacement are follow-ups.
- Device-loss recovery uses `AudioSystem`'s state machine, but the MVP does not
  surface a reconnect dialog; `tick`/`state` are ready for the UI to do so.
- The demo WAVs are decoded through temporary files at startup and removed
  immediately afterwards. Import runs on the UI thread for the MVP.
