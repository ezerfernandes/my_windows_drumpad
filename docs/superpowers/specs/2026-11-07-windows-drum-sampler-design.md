# Drumpad: Windows drum sampler design and work breakdown

- Date: 2026-11-07
- Status: Draft for review
- Working title: Drumpad
- Tracker: beads_rust in `.beads/`, with 12 epics and 94 work packages
- Scope: Windows 11 x64 v1, with post-v1 work tracked separately

This document is the design and work breakdown structure (WBS) for the first release.

## 1. Problem

Build a lightweight Rust drum sampler for Windows 11 x64, inspired by the Roland SPD-SX. It should turn a normal computer keyboard or a single-handed gamer keypad, plus an Alesis Nitro Max electronic drum kit, into a fast, stage-ready performance instrument. It is a sampler, not a digital audio workstation (DAW): no timeline, no multitrack recording, no mixing console.

## 2. Goals

- Low latency: target under 10 ms end to end at 48 kHz with a 128-frame buffer on reference hardware. Keyboard and keypad events should reach the audio queue in under 2 ms.
- Immediate, click-free one-shot playback with at least 64 voices, velocity layers, choke groups, and a master limiter.
- A 16-pad, 4x4 performance surface controlled by a full keyboard or a single-handed gamer keypad, not a dedicated pad controller.
- Keyboard profiles that make the 4x4 pad grid intuitive on both a regular keyboard and a one-hand keypad, with visible key labels and a map overlay.
- Native Alesis Nitro Max integration over USB-MIDI, plus the Aux In monitoring path, hi-hat articulations, and hotplug recovery.
- A native, dark, high-contrast UI that is usable on stage and never blocks the audio thread.
- A portable Windows 11 x64 app that is safe to install, update, and move between machines.

## 3. Non-goals for v1

- DAW timeline, multitrack recording, or a mixing console.
- VST hosting or an audio plugin format.
- Time-stretching. v1 may quantize loop starts, but does not stretch loops to tempo.
- Windows 10, macOS, or Linux releases. Windows 11 x64 is the only supported target.
- Cloud sync, accounts, telemetry, or online sample libraries.
- Audio input recording. This is post-v1.
- Dedicated pad-controller support. The keyboard and one-hand keypad are the primary surfaces.

## 4. Architecture

### 4.1 Crate boundaries

| Crate | Responsibility | Must not depend on |
| --- | --- | --- |
| `drumpad-model` | Kit and pad domain, mappings, config, error types | egui, winit, cpal, midir |
| `drumpad-samples` | Decode, resample, waveform peaks, content cache | UI, audio devices |
| `drumpad-engine` | Voice pool, mixer, choke groups, limiter, meters | UI, audio devices |
| `drumpad-audio` | Windows audio device enumeration, stream lifecycle, WASAPI backend, fake backend | UI |
| `drumpad-input` | Keyboard and MIDI capture, trigger abstraction, device hotplug | UI rendering |
| `drumpad-app` | winit and egui shell, views, app state, persistence wiring | Audio callback internals |
| `drumpad-tools` | Offline renderer, benchmarks, latency harness, fixture generators | Shipping code |

The dependency direction is `app -> audio/input/engine -> samples/model`. Model and samples stay testable without hardware and without a window. The split keeps parallel work in separate crates and avoids a single shared `lib.rs` bottleneck.

### 4.2 Threads and data flow

- UI thread: winit event loop, egui rendering, kit browsing, editing.
- Audio callback: real-time thread created by the audio backend. It performs no allocation, locking, logging, or file access.
- MIDI callback: midir callback thread. It parses messages and pushes trigger events into a lock-free queue.
- Import worker: decodes, resamples, hashes, and builds waveform peaks off the UI thread.

Data flow:

```text
keyboard / MIDI
      |
      v
TriggerEvent { pad, velocity, source, timestamp }
      |
      v
SPSC ring buffer
      |
      v
audio callback -> voice pool -> per-pad gain/pan -> master limiter -> device

UI edits -> command ring buffer -> audio callback
audio callback -> atomics / snapshots -> UI meters and diagnostics
```

Samples are immutable once published. A kit load builds a new sample bank and swaps it at a safe point. Voice state never owns a heap allocation.

### 4.3 Audio engine

- Voice pool: 64 voices by default, configurable from 16 to 256. Steal the oldest or quietest voice with a short fade.
- Playback modes: one-shot, gate, and loop. Loop points are sample-accurate and can crossfade.
- Interpolation: cubic by default, with a lower-cost linear option if profiling requires it.
- Mixing: equal-power pan, smoothed per-pad gain, choke groups with 5 to 10 ms fades.
- Master bus: soft-knee limiter or soft clipper with documented latency, plus peak and RMS meters.
- Diagnostics: xrun count, callback load, voice count, and estimated latency exposed as atomics.

### 4.4 Sample pipeline

- Decode WAV, AIFF, FLAC, MP3, and OGG with symphonia into f32.
- Preserve mono and stereo; optionally downmix to mono.
- Resample offline to the device rate with rubato and cache by content hash plus rate.
- Generate multi-resolution min/max peaks for the waveform view.
- Store samples in a content-addressed cache. Kit files reference relative paths and hashes.
- Import runs on a worker with progress and cancellation; missing files are recoverable through a relink flow.

### 4.5 Kit and pad model

- A kit contains 16 pads, each with one or more velocity layers, playback mode, gain, pan, tune, choke group, MIDI note, and keyboard key.
- Kit files are versioned JSON manifests with relative sample paths and SHA-256 hashes.
- Writes use temp file, fsync, and atomic rename. Rotating backups and debounced autosave protect against crashes.
- Migrations upgrade older kits and refuse versions from the future.

### 4.6 Input

- Keyboard uses physical key codes, not characters, so the keyboard layout does not matter.
- Default full-keyboard map: `1 2 3 4`, `Q W E R`, `A S D F`, `Z X C V`. The 4x4 pad grid mirrors this block.
- The reference one-hand keyboard is the Finding 35-key single-hand USB keypad (ASIN B09HF5WDR2). Its QWERTY block contains the same 4x4 grid, so the default mapping carries over directly.
- On the Finding keypad, F1-F5 map to transport actions. `5`, `T`, `G`, `B`, and `Space` stay unmapped to avoid accidental hits. The left macro column may only control lighting, so the app does not rely on it.
- A one-hand keypad profile maps the 16 pads to the keys on a gamer keypad. The mapping editor captures physical keys, detects conflicts, and saves or loads profiles per device.
- The pad grid shows the active key label on every pad. A map overlay shows the full keyboard or keypad layout and highlights pads as keys are pressed.

Finding 35-key transport row:

| Key | Action |
| --- | --- |
| F1 | Panic / stop all |
| F2 | Metronome toggle |
| F3 | Previous kit |
| F4 | Next kit |
| F5 | Keyboard map overlay |
| 5, T, G, B | Unmapped |
| Space | Unmapped |
- Key down and key up are separate events. Auto-repeat is suppressed.
- A dedicated spike compares eframe per-frame input, a custom winit event-loop hook, and a Windows low-level hook on both a regular keyboard and a one-hand keypad. The chosen path must keep key-to-queue latency under 2 ms.
- MIDI uses midir input with device enumeration, channel filtering, MIDI learn, velocity curves, and hotplug recovery.
- The Nitro Max sends discrete hi-hat notes: 46 open, 23 half-open, 42 closed, 44 pedal, and 21 splash. The hi-hat engine also supports continuous CC4 or CC64 position for modules that send it.

### 4.7 Nitro Max monitoring and signal chain

The Nitro Max USB port sends MIDI only. It does not carry audio. The app uses the PC onboard 3.5 mm output and returns the mix to the module through the 3.5 mm stereo Aux In:

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

- Set Nitro Max Local Mode to OFF. The module then sends MIDI to the computer and bypasses its internal sounds, so the app is the only drum source. This avoids flam between module sounds and app samples.
- With Local Mode ON, the module's internal sounds and the app's Aux In audio are mixed. That is useful for adding percussion to the kit's own sounds, but the app path may arrive a few milliseconds later. Document the tradeoff and test for flam.
- The Aux In has no separate gain control. Set the PC output level and the module master volume so the app does not clip the module input. A USB audio interface can replace the PC output if the onboard jack is noisy.
- Nitro Max note map: kick 36, snare 38, snare rim 40, tom 1 48, tom 1 rim 50, tom 2 45, tom 2 rim 47, tom 3 43, tom 3 rim 58, ride 51, crash 1 49, hi-hat open 46, hi-hat half-open 23, hi-hat closed 42, hi-hat pedal 44, hi-hat splash 21. Optional expansion pads: tom 4 41, tom 4 rim 39, crash 2 57.
- Use a short USB cable and a direct port if the module is not detected reliably. The Nitro Max is class-compliant, so no vendor driver should be required.

### 4.8 UI

- winit plus egui, integrated directly so keyboard events can be forwarded before the next frame.
- Views: Performance, Edit, Settings, and a kit browser.
- Pad grid: 4x4, sample name, active key and MIDI hints, hit flash, velocity ring, level meter, and selection.
- Inspector: sample slot, layers, mode, volume, pan, tune, choke group, MIDI note, and keyboard key.
- Performance mode: full-screen, large pads, hidden chrome, and the F1-F5 transport row for panic, metronome, previous kit, next kit, and map overlay.
- Visual language: dark stage theme, high contrast, color-coded pads, DPI-aware layout, and explicit error recovery.

## 5. Key decisions and alternatives

| Decision | Chosen path | Alternatives considered | Why |
| --- | --- | --- | --- |
| Audio backend | cpal WASAPI shared first; WASAPI exclusive only if the latency spike requires it | `wasapi` crate only, ASIO, cubeb | Shortest path to a working stream; exclusive mode is a scoped follow-up |
| UI stack | winit + egui | plain eframe, Slint, Tauri | Direct event-loop access for keyboard latency; custom pad grid is straightforward |
| Input surfaces | Full keyboard and Finding 35-key one-hand keypad profiles | Dedicated pad controller, MIDI pad | Matches the hardware the user already has; profiles keep the 4x4 grid intuitive |
| MIDI | midir | raw WinMM, `windows` crate | Mature input support and a simple fake-backend seam |
| Nitro Max monitoring | USB-MIDI trigger input plus PC onboard 3.5 mm audio into the Aux In | USB audio, audio interface only | The module has no USB audio; Aux In is the built-in monitoring path |
| Kit format | JSON manifest | RON, SQLite | Portable, inspectable, easy to migrate |
| Sample storage | Preload into RAM, content-addressed cache | Stream from disk | Drum samples are small; preloading removes I/O from the audio path |
| Platform scope | Windows 11 x64 | Windows 10, cross-platform v1 | Keeps the first release focused on WASAPI and Windows packaging |

## 6. Latency budget and measurement

Targets on reference hardware:

- Keyboard or keypad event to audio queue: under 2 ms.
- Nitro Max USB-MIDI input to audio callback: under one audio block.
- Audio callback buffer: 128 frames at 48 kHz, 2.67 ms.
- End-to-end keyboard, Finding keypad, or Nitro Max to analog output, including the onboard 3.5 mm to Aux In path: under 10 ms.

The audio backend spike runs on Windows 11 x64 and records shared and exclusive mode results at 64, 128, and 256 frames. The QA epic defines a loopback measurement protocol for the Nitro Max USB-MIDI and Aux In chain and stores baseline results with hardware details. With Local Mode OFF, the module's internal sounds are bypassed, so the app path is the only latency that matters. If shared-mode WASAPI misses the target, exclusive mode becomes a v1 requirement rather than a later optimization.

## 7. Work breakdown summary

The tracker contains 12 epics and 94 work packages. Estimates are rough focused-work minutes and total about 431 hours, before review, learning, and iteration. The v1 epics total about 395 hours; post-v1 adds about 36 hours.

| WBS | Bead | Epic | Tasks | Estimate (min) |
| --- | --- | --- | --- | --- |
| E01 | `my_windows_drumpad-2zf` | Foundation and engineering harness | 7 | 1260 |
| E02 | `my_windows_drumpad-eno` | Audio backend and device management | 7 | 1860 |
| E03 | `my_windows_drumpad-ong` | Real-time playback engine | 8 | 2100 |
| E04 | `my_windows_drumpad-pa7` | Sample pipeline and waveform cache | 7 | 1860 |
| E05 | `my_windows_drumpad-us5` | Kit and pad model with persistence | 8 | 2100 |
| E06 | `my_windows_drumpad-tl9` | Keyboard and single-hand keypad performance input | 8 | 1980 |
| E07 | `my_windows_drumpad-iba` | MIDI and Alesis Nitro Max integration | 10 | 2640 |
| E08 | `my_windows_drumpad-e1t` | App shell and pad UI | 11 | 3180 |
| E09 | `my_windows_drumpad-khs` | Sample editing and waveform | 6 | 1740 |
| E10 | `my_windows_drumpad-zc8` | Live performance features | 7 | 2160 |
| E11 | `my_windows_drumpad-74p` | Hardening, QA, and release | 10 | 2820 |
| E12 | `my_windows_drumpad-23p` | Post-v1: recording, effects, and advanced I/O | 5 | 2160 |

### 7.1 Delivery phases

1. Foundation: E01. One root task (`my_windows_drumpad-2zf.1`) unblocks the rest of the graph.
2. Vertical slice: audio backend, playback engine, keyboard input, minimal pad grid, and a hard-coded sample. Prove latency and sound before building editors.
3. Minimum viable product (MVP): sample import, kit persistence, Nitro Max USB-MIDI and Aux In integration, keyboard and one-hand keypad profiles, inspector, kit browser, settings, and first-run setup.
4. Polish: sample editing, velocity layers, round-robin, hi-hat editor, metronome, loop sync, setlists, and performance mode.
5. Hardening and release: automated coverage, latency gates, compatibility matrix, soak testing, docs, installer, beta.
6. Post-v1: recording, ASIO, multi-output, master effects, MIDI clock.

### 7.2 Critical path

`E01-T01 -> E02-T01 -> E02-T02 -> E02-T03 -> E02-T04 -> E03-T01 -> E03-T02 -> E03-T03 -> E03-T04 -> E08-T01 -> E08-T02 -> E08-T03`

This produces the first playable vertical slice. E04-T01/T02, E05-T01/T02/T03, E06-T03/T04, and E07-T01/T03 run in parallel once the workspace exists. The Nitro Max path continues through E07-T06, E07-T08, and E07-T10. Keyboard profiles continue through E06-T06, E06-T08, and E08-T11.

## 8. Risks and mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| WASAPI shared mode cannot meet the latency target | The product premise weakens | Run the backend spike first; add exclusive mode only if needed; keep ASIO as a post-v1 path |
| Egui per-frame input adds too much keyboard latency | Keyboard or keypad feels soft | Custom winit event-loop path; low-level hook as a last resort; measure p99 on both devices |
| One-hand keypad has fewer than 16 usable keys or poor rollover | Some pads cannot be played or chords drop | Profile model, key rollover and ghosting test, mapping editor, optional bank key |
| Nitro Max Local Mode stays on and app samples flam with module sounds | Layered hits sound sloppy | Make Local Mode OFF the primary setup; test for flam; use app-only kits |
| Aux In gain staging clips the module input | Distortion at the module | Gain-staging check, master limiter, and a setup guide |
| Real-time callback does allocations or locks | Dropouts and crackle | Preallocate, use SPSC queues, audit, add debug assertions and CI guardrails |
| Scope creep into DAW features | v1 never ships | Explicit non-goals; post-v1 epic; no timeline or recording in v1 |
| Sample licensing | Release blocked | Ship CC0 or original demo samples with recorded attribution |
| Windows 11 audio device variability | Poor first-run experience | Compatibility matrix, device fallback, clear diagnostics |

## 9. Testing and release gates

- CI on Windows: fmt, clippy with warnings denied, unit tests, release build.
- Core unit tests: kit invariants, mappings, velocity curves, voice allocation, queue behavior.
- Offline golden renders: one-shot, gate, loop, choke, and limiter paths.
- Fake MIDI and audio harness: deterministic end-to-end trigger sequences without hardware.
- UI smoke tests: pad grid, inspector, settings, kit browser, keyboard map overlay, keyboard navigation.
- Manual matrix: Windows 11 x64, onboard and USB audio, 44.1/48/96 kHz, one full keyboard, one physical one-hand keypad, and a Nitro Max with Local Mode OFF through the Aux In path.
- Soak test: 8 hours with voices, MIDI hotplug, and UI animation, with no leak or xrun growth.
- Release gates: latency targets met, no known P0/P1 bugs, portable ZIP and installer verified, docs complete.

## 10. Assumptions and open questions

These assumptions shaped the WBS. They are cheap to change before implementation starts.

- 16 pads in a 4x4 grid, controlled by a full keyboard or a single-handed gamer keypad, not a dedicated pad controller.
- The reference one-hand keyboard is the Finding 35-key single-hand USB keypad (ASIN B09HF5WDR2). Its QWERTY block contains the 4x4 pad grid.
- F1-F5 map to panic, metronome toggle, previous kit, next kit, and keyboard map overlay. `5`, `T`, `G`, `B`, and `Space` stay unmapped.
- v1 imports samples; recording is post-v1.
- The reference e-kit is an Alesis Nitro Max. Its USB port sends MIDI only. The app uses the PC onboard 3.5 mm output and returns audio through the Nitro Max 3.5 mm stereo Aux In.
- Nitro Max Local Mode is OFF for the primary profile, so the app is the only drum sound source and the module acts as a MIDI controller and monitor mixer.
- The Nitro Max hi-hat sends discrete notes for open, half-open, closed, pedal, and splash. Continuous CC4/CC64 support stays in the engine for other modules.
- Windows 11 x64 is the only v1 target.
- The app is standalone. VST hosting, DAW timelines, and plugin formats are out of scope.
- The UI is a native winit/egui app, not a browser shell.
- The first release is portable-first, with an installer as a release task.

Open questions for the next review:

- `Space` and `5`/`T`/`G`/`B` are unmapped. Should any of them become an optional second control layer later?
- Should the Nitro Max profile assume Local Mode OFF only, or also ship a mixed Local Mode ON profile for layering app samples over the module's sounds?
- Should v1 include a built-in metronome and loop sync, or defer them to a later release? They are in E10.
- Is a portable ZIP enough for the first release, or is an installer required on day one?

## 11. Tracker workflow

- Start with `bv --robot-triage` or `br ready --json`.
- The first actionable task is `my_windows_drumpad-2zf.1` (bootstrap the workspace).
- Claim work with `br update <id> --claim --json`.
- After tracker mutations, run `br sync --flush-only`.
- The design document is not committed. Commit it only when requested.
