# Cross-crate interface contract

This is the shared vocabulary for the first implementation wave. Keep public APIs small and additive. If a change crosses a crate boundary, post it on your team thread and the orchestrator will relay it to the owning pair.

## Dependency direction

```text
drumpad-model
   ^        ^
   |        |
drumpad-samples   drumpad-input
   ^        ^
   |        |
drumpad-engine
   ^
   |
drumpad-audio

drumpad-app -> model, samples, input, engine, audio
drumpad-tools -> model, samples, engine
```

No UI dependency in model, samples, input, engine, or audio. No Windows dependency in model or samples.

## drumpad-model

Owns the durable vocabulary:

- `Kit`, `KitId`
- `Pad`, `PadId`, `Layer`, `PlaybackMode`, `ChokeGroup`
- `MidiMapping`, `KeyMapping`, `KeyboardProfile`
- `Velocity` (1..=127)
- Model error types

Keep serialization types here. Do not put audio buffers, device handles, or UI types here.

## drumpad-samples

Owns decoded audio:

- `SampleBuffer`: immutable, interleaved `f32`, channel count, frame count, sample rate, shared ownership suitable for the audio thread
- `DecodedSample`: source metadata plus a `SampleBuffer`
- `SampleCache`, `PeakCache`
- Sample error types

The engine consumes `SampleBuffer` values. The sample crate does not know about pads or kits.

## drumpad-input

Owns source events:

- `TriggerEvent { pad: PadId, velocity: Velocity, source: TriggerSource, timestamp_us: u64 }`
- `ReleaseEvent { pad: PadId, source: TriggerSource, timestamp_us: u64 }`
- `TriggerSource`: `Keyboard`, `Midi`, `UiPreview`
- Keyboard and MIDI modules produce these events

The engine depends on this crate for the event types. Input does not depend on the engine or app.

## drumpad-engine

Owns the real-time path:

- `PlaybackEngine`: owns the voice pool, sample bank, mixer, limiter, and meters
- `EngineConfig`: sample rate, block size, max voices
- `EngineCommand`: pad parameter changes, sample-bank swap, stop-all
- `EngineSnapshot`: meters, voice count, callback load, xruns
- `process(&mut self, output: &mut [f32], frames: usize)`

The engine consumes `TriggerEvent`, `ReleaseEvent`, `SampleBuffer`, and model pad settings. It does not own a device or a window. The audio crate calls `process` from the callback.

## drumpad-audio

Owns the device path:

- `AudioBackend`, `AudioDevice`, `StreamConfig`, `AudioError`
- A fake backend for tests
- WASAPI implementation on Windows

The audio callback holds the engine and writes to the device. It does not know about UI or kit files.

## drumpad-app

Owns composition and UI:

- `app::run()` entry point
- Wires input -> engine -> audio
- Owns UI state, kit browser, inspector, waveform editor, settings screens
- Reads `settings.rs` and `logging.rs` from the foundation pair

The app is the only crate that knows about all the others.

## drumpad-tools

Owns offline and diagnostic tooling:

- Offline renderer
- Benchmarks
- Latency harness
- Test fixtures

## Change process

- Additive changes inside your crate: proceed.
- New dependency between crates: post on your team thread, name the exact type and method, and wait for the owning pair or orchestrator to confirm.
- Shared root files (`Cargo.toml`, `Cargo.lock`, `.github`, `README.md`, `docs/adr`, `docs/dev`, `docs/release`) belong to the foundation pair.
- Do not edit another pair's crate. Use Agent Mail reservations and the orchestrator as the tie-breaker.
