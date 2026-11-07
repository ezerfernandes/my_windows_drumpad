# Drumpad

A low-latency Rust drum sampler for Windows 11 x64, inspired by the Roland SPD-SX. It is a performance sampler, not a DAW.

## v1 scope

- 16 pads in a 4x4 grid.
- Full keyboard or single-handed gamer keypad, with per-device profiles, visible key labels, and a map overlay.
- Reference one-hand keyboard: Finding 35-key single-hand USB keypad (ASIN B09HF5WDR2). The 16 pads use `1 2 3 4`, `Q W E R`, `A S D F`, `Z X C V`.
- Alesis Nitro Max over USB-MIDI, with app audio returning through the Nitro Max Aux In.
- Sample import for WAV, AIFF, FLAC, MP3, and OGG.
- Kits, velocity layers, choke groups, hi-hat articulations, loops, and non-destructive sample editing.
- Native winit and egui UI. WASAPI shared mode first, exclusive mode only if measurements require it.
- Windows 11 x64 only.

### Finding keypad mapping

| Key | Action |
| --- | --- |
| `1 2 3 4` / `Q W E R` / `A S D F` / `Z X C V` | 16 pads |
| F1 | Panic / stop all |
| F2 | Metronome toggle |
| F3 | Previous kit |
| F4 | Next kit |
| F5 | Keyboard map overlay |
| `5` `T` `G` `B` | Unmapped |
| Space | Unmapped |

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
                              Nitro Max phones / audio outputs
```

Set the Nitro Max Local Mode to OFF. The module then sends MIDI to the computer and bypasses its internal sounds, so Drumpad is the only drum source. The module still mixes the app audio from its Aux In into its phones and line outputs.

The USB port on the Nitro Max sends MIDI only. It does not carry audio. The reference path uses the PC onboard 3.5 mm output. A USB audio interface remains a fallback if the onboard jack is noisy.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `drumpad-model` | Kits, pads, mappings, config, error types |
| `drumpad-samples` | Decode, resample, waveform peaks, content cache |
| `drumpad-engine` | Voice pool, mixer, choke groups, limiter, meters |
| `drumpad-audio` | Windows audio devices, stream lifecycle, WASAPI backend |
| `drumpad-input` | Keyboard and MIDI capture, profiles, hotplug |
| `drumpad-app` | winit and egui shell, views, app state, persistence |
| `drumpad-tools` | Offline renderer, benchmarks, latency harness |

The audio callback owns a preallocated voice pool and reads triggers from a lock-free queue. No allocation, locking, or file access runs on the audio thread. Samples are decoded and resampled off-thread, then published as immutable buffers.

## Plan and tracker

- Design and WBS: `docs/superpowers/specs/2026-11-07-windows-drum-sampler-design.md`
- Tracker: beads_rust in `.beads/`

Useful commands:

```sh
cargo build --workspace
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
pwsh ./scripts/package-portable.ps1

bv --robot-triage
br ready --json
br show my_windows_drumpad-2zf.1 --json
br update my_windows_drumpad-2zf.1 --claim --json
br sync --flush-only
```

## Documentation

- Design and WBS: `docs/superpowers/specs/2026-11-07-windows-drum-sampler-design.md`
- Architecture decisions: [docs/adr/README.md](docs/adr/README.md)
- Windows developer setup: [docs/dev/windows-setup.md](docs/dev/windows-setup.md)
- Cross-crate interfaces: [docs/architecture/interfaces.md](docs/architecture/interfaces.md)
- Third-party notices: [docs/release/THIRD_PARTY_NOTICES.md](docs/release/THIRD_PARTY_NOTICES.md)

## Status

Foundation in progress. The Cargo workspace, Windows CI, typed error model, local
logging and crash capture, app paths, settings and portable mode, release
profile, portable ZIP packaging, and architecture decision records are in
place. The audio, engine, samples, input, and UI crates are being implemented by
their own teams.
