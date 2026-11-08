# Latency budget and measurement protocol (E11-T01)

Owner: coder (`drumpad-qa`). Machine-readable baseline:
`crates/drumpad-tools/baselines/latency-baseline.json`. Rendered table:
`docs/qa/latency-baseline.md`.

This document defines the latency targets, the exact statistics that decide a
pass, and a loopback procedure anyone can repeat on the reference hardware.
The numbers in the design document §6 are the authority; this document adds
the operational detail needed to reproduce them.

## 1. Targets and tolerances

All targets assume Windows 11 x64, WASAPI shared mode, 48 kHz, 128-frame
blocks, and the reference chain below. A run at another format must state its
format in the baseline table; the block-relative target is recomputed from the
actual block duration.

> **Mode note (2026-11-08):** the app now defaults to WASAPI exclusive mode with
> a shared fallback; see `docs/audio/backend-spike.md` and
> `docs/qa/latency-autoresearch/summary.md`. The shared-mode assumption above
> still defines the reference row, but every run must record the negotiated
> mode, buffer and callback period next to its format, because the
> block-relative target depends on the period the endpoint actually grants.

| Metric | Definition | Target | Pass statistic | Minimum retained hits | Warm-up discarded |
| --- | --- | --- | --- | ---: | ---: |
| `key_to_queue` | Key/keypad event observed by the app until the trigger is visible on the engine queue | < 2.0 ms | p99 ≤ 2.0 ms **and** max ≤ 4.0 ms | 1000 | 100 |
| `midi_to_callback` | MIDI event observed by the app until the callback renders the first sample of the hit | < 1 audio block | p99 ≤ 1 block | 1000 | 100 |
| `audio_block` | Measured callback block duration against nominal | 128 frames @ 48 kHz = 2.667 ms | p50 within ±10 % of nominal | 100 | 10 |
| `end_to_end` | Physical input until the analog marker crosses the loopback capture | < 10 ms | p99 ≤ 10.0 ms **and** max ≤ 15.0 ms | 200 | 20 |

Notes:

- `midi_to_callback` is one block at 48 kHz / 128 frames = 2.667 ms. At
  44.1 kHz it is 2.902 ms; at 96 kHz it is 1.333 ms. The harness recomputes it
  from the stream format.
- The hard ceilings exist so a single 40 ms OS hiccup cannot hide behind a
  healthy p99. They are not additional averages.
- Percentiles are **nearest rank** over the sorted retained samples:
  `index = ceil(percentile / 100 * n) - 1`, clamped to the last sample. The
  same definition is implemented in `crates/drumpad-tools/src/latency.rs` and
  can be reproduced by hand from the raw capture.
- Outliers are **never deleted**. Keep every retained sample, report the max,
  and fail the run when the max exceeds the ceiling. If a sample is traced to
  a documented environment fault (driver restart, Windows Update, thermal
  throttle), archive the run, mark it `invalid` with the reason, and repeat
  it. Do not edit the capture.
- Repeat every configuration three times. The configuration passes only when
  at least two of the three runs pass. Report all three runs, including the
  failing one.
- The first run after a cold boot is excluded from the verdict and archived
  separately; driver initialisation is not representative of performance.

Why the hit counts differ: the software metrics are cheap and silent, so 1000
retained hits cost a few seconds and give a stable p99. The end-to-end metric
needs a physical hit or a scripted keypress per sample, with a human at the kit
and a 96 kHz capture for each run; 200 retained hits is the smallest set that
still resolves p99 (the second-worst hit) over a realistic two-minute pass.
The hard ceilings (4 ms for `key_to_queue`, 15 ms for `end_to_end`) are protocol
decisions, not engine constants: they sit at roughly twice the budget so a
single OS hiccup fails the run instead of hiding behind a healthy p99.

## 2. Reference chain

```text
Finding 35-key single-hand USB keyboard ─┐
                                        ├─> Windows 11 x64 ─> WASAPI shared ─> PC onboard 3.5 mm ─> Nitro Max Aux In ─> phones / line out
Alesis Nitro Max pads ─> USB-MIDI ──────┘
```

- **Keyboard:** Finding 35-key single-hand USB keypad (ASIN B09HF5WDR2), wired,
  direct USB port, 16-pad block `1 2 3 4 / Q W E R / A S D F / Z X C V`.
  Also record one row for a full-size wired keyboard.
- **MIDI:** Alesis Nitro Max over USB-MIDI, short cable, direct port, class
  driver. **Local Mode must be OFF.** With Local Mode OFF the module bypasses
  its internal sounds, so the app is the only drum source and there is no flam
  between module and app audio. A run with Local Mode ON is a compatibility
  observation, never a latency baseline.
- **Output:** PC onboard 3.5 mm output, fixed Windows volume (recommended
  70 %), all enhancements and spatial audio disabled, no Bluetooth audio.
- **Monitoring:** 3.5 mm stereo into the Nitro Max Aux In. The Aux In has no
  trim, so gain-staging is set once and recorded in the baseline notes: the
  loudest test hit must not light the module's clip indicator. Do not change
  the module master volume between runs.
- **Power:** laptop on AC, Windows power plan **High performance** (or
  **Ultimate**), sleep and USB selective suspend disabled for the test.
- **Background:** close browsers, chat clients, DAWs, RGB utilities, and
  streaming software. Disable Wi-Fi and Bluetooth where practical. Keep the
  same OS build for a comparison run.

## 3. Metric timestamps

Each hit produces a tuple of timestamps. All software timestamps use
`QueryPerformanceCounter` converted to seconds; the audio frame position is
the engine playhead.

| Timestamp | Taken from | Meaning |
| --- | --- | --- |
| `t_input` | winit `KeyEvent` / MIDI receive callback | Event first visible to the app |
| `t_queue` | engine queue push | Trigger is on the lock-free queue |
| `t_render` | engine frame → seconds | First frame of the hit inside a rendered block |
| `t_submit` | WASAPI submission log | QPC time the containing block was handed to the driver |
| `t_capture` | loopback detector | Analog marker crossing in the capture file |

Derived metrics:

- `key_to_queue = t_queue - t_input`
- `midi_to_callback = t_render - t_input`
- `end_to_end = t_capture - t_input` after mapping the capture clock onto the
  app clock (section 5)
- `output_path = t_capture - t_submit` for the marker train; this isolates the
  WASAPI buffer plus DAC/analog path and is reported alongside the baseline

**Documented exclusion:** `t_input` is the moment the OS delivers the event to
the app. The USB keyboard scan interval and the OS input stack before delivery
are outside this measurement. A typical wired keyboard reports every 1–8 ms;
use a 1000 Hz-capable wired keyboard for the reference row and record the model
and polling rate. The MIDI path is class-compliant USB-MIDI, whose delivery is
also before `t_input`; the same exclusion applies. Do not silently present
`end_to_end` as including those intervals.

## 4. Software metrics (`key_to_queue`, `midi_to_callback`)

These run entirely in the app plus the offline harness; no analog loopback is
needed.

1. Build in release mode: `cargo build --release -p drumpad-app` (the app owns
   the measurement mode; the harness in `drumpad-tools` owns the statistics).
2. Enable measurement mode. It logs one JSON line per hit:
   `{"pad":3,"source":"keyboard","t_input_ns":...,"t_queue_ns":...,"t_render_ns":...}`
   plus the stream format `{sample_rate, block_frames, mode}`. The log must not
   run on the audio thread; the engine records numbers into atomics and a
   non-real-time thread drains them.
3. Play 1200 hits per source (1000 retained after warm-up):
   - keyboard: a 16-pad roll, at least 60 hits per pad, alternating rows;
   - MIDI: a repeated single-pad roll and a 16-pad sweep on the Nitro Max.
4. Feed the log to `latency_report` through the harness adapter (the adapter
   accepts the JSON-lines log and emits a `Measurement` per metric).
5. Verify that no hit reports a negative interval. A negative value indicates
   a timestamp-source bug; the run is invalid.

`key_to_queue` is the software gate for the input path. `midi_to_callback`
additionally includes queue wait and block scheduling, so it must be measured
with the audio device running. A run with the device stopped is not a valid
`midi_to_callback` baseline.

## 5. End-to-end loopback

The loopback measures everything from `t_input` through the analog output,
including the Aux In mix. It needs physical hardware, so it is a release-gate
run and never a CI test.

### 5.1 Equipment

- USB audio interface with a 96 kHz, 24-bit capture path and a known input
  latency (record the model and driver).
- 3.5 mm stereo to dual 6.35 mm TS (or a 3.5 mm splitter).
- Balanced attenuator or a −20 dB pad for the Nitro Max phones/line output.
- Short shielded cables. No Bluetooth, no USB hub between interface and PC if
  it can be avoided.

Two loopback variants are recorded:

| Variant | Patch | What it isolates |
| --- | --- | --- |
| Direct | PC 3.5 mm → interface line in (attenuated) | WASAPI buffer, DAC, onboard analog output |
| Full chain | PC 3.5 mm → Nitro Max Aux In; Nitro Max phones → interface in (attenuated) | Adds the module's Aux In mixer and headphone amp |

The **full chain** row is the reference row for `end_to_end`. The direct row is
used to split output-path latency from the module's contribution.

### 5.2 Marker signal

The app plays a marker train: 10 impulses, 1.0 s apart, one sample at 0.9 FS
followed by silence. A single-sample impulse has broadband energy and a sharp
first crossing; do not low-pass it before detection. For each impulse the app
logs `t_submit` and the engine frame of the impulse.

Detector: first sample whose absolute value exceeds 10 % of the largest sample
in a 100 ms window after the expected arrival. Report the crossing at the
capture's native rate (96 kHz = 10.4 µs per sample). Keep the detector code in
`drumpad-tools` so every operator uses the same threshold.

### 5.3 Procedure

1. Patch the chosen variant. Set the interface input gain so the loudest
   impulse peaks at −12 dBFS; do not touch it during the run.
2. Start the capture (96 kHz, 24-bit, 30 s) and immediately start the marker
   train. Capture at least 2 s of silence before the first impulse.
3. Detect the 10 impulses. Fit capture time against logged `t_submit`:
   - slope → capture-clock ratio; report drift in ppm;
   - intercept → output path latency `L_out`.
4. If |drift| > 100 ppm, correct the capture by resampling to the app rate or
   discard the run and repeat with the interface as the clock master. Record
   the ppm in the raw notes even when correcting.
5. Keep the fitted `L_out` for the run. It is reported in the baseline notes,
   not asserted against a target.
6. Run the actual hits: 250 keyboard hits and 250 Nitro Max hits (200 retained
   each after 20 warm-up hits). For each hit, detect the onset in the capture
   and compute `end_to_end = t_capture - t_input`, using the fitted clock ratio
   to map capture time onto the app clock.
7. A hit whose detector confidence is low (two crossings within 1 ms, or no
   crossing in the window) is logged as `undetected`, kept in the raw file,
   and excluded from the percentile set. If more than 2 % of hits are
   undetected, the run is invalid.

Alignment does not use the acoustic room path. The capture is electrical, so
the only delay is the device and cabling; the deliberate 1 s spacing makes the
drift fit stable and keeps impulse windows from overlapping.

### 5.4 Clock drift

The app clock and the interface clock are independent. The marker train
measures the ratio to better than 1 ppm over 9 s. Always report:

- measured ppm,
- whether the run was corrected by resampling,
- the interface clock source (internal or word clock).

A drift-corrected run is valid only when the correction is a single linear
ratio over the whole capture. Do not warp each hit independently.

## 6. Audio block sanity check

`audio_block` does not measure latency; it proves the stream is configured as
reported. Record the callback's measured block period (QPC deltas between
callback entries) for 10 s. The p50 must be within ±10 % of
`frames / sample_rate`. A mismatch means the device ignored the requested
format or the shared-mode mixer inserted an extra period; fix the format
before interpreting any other metric. Report xruns and callback-load p99 in the
notes.

## 7. Raw captures and archiving

Every run produces:

```text
docs/qa/latency-raw/<date>-<machine>-<mode>-<rate>-<frames>/
  capture.wav           96 kHz loopback capture (or .flac, lossless)
  hits.jsonl            t_input / t_queue / t_render / t_submit per hit
  markers.json          detector output: capture index, confidence, ppm, L_out
  environment.txt       OS build, driver versions, power plan, volume, cables,
                        interface model, attenuator, operator, git revision
  verdict.json          latency_report output for this run
```

Raw captures live outside the source tree in normal development, but a release
candidate archives the whole folder under `docs/qa/latency-raw/` (or a linked
artifact store) and the baseline row's `raw_capture` column points at it. Never
store only summary numbers.

## 8. Baseline table

`crates/drumpad-tools/baselines/latency-baseline.json` is the source of truth.
Render it with:

```powershell
cargo run -p drumpad-tools --bin latency_report -- `
  crates/drumpad-tools/baselines/latency-baseline.json `
  --markdown docs/qa/latency-baseline.md
```

The rendered table has one row per metric and these columns: status, hardware,
OS build, driver, device, mode, rate/buffer, metric, p50, p95, p99, max, hits,
verdict, date, operator, raw capture. Required rows:

1. Reference desktop, onboard audio, WASAPI shared, 48 kHz, 128 frames.
2. Reference desktop, onboard audio, WASAPI shared, 44.1 kHz, 128 frames.
3. Reference desktop, onboard audio, WASAPI shared, 96 kHz, 128 frames.
4. Reference desktop, USB interface output, WASAPI shared, 48 kHz, 128 frames.
5. WASAPI exclusive row only if the backend spike adds exclusive mode.

Rows that have not been measured must say `pending`; they are not evidence of
compliance and a release cannot be signed off on them. The JSON validator
refuses `--require-pass` while any required row is pending.

Current status: the physical loopback rows are **pending** (no Nitro Max,
Finding keypad, or measurement interface attached to the development machine
that produced this document). The offline callback baseline from the
development machine is recorded separately in the JSON and is labelled
`offline harness`; it is not a substitute for the analog rows.

## 9. What this protocol does not cover

- USB keyboard scan interval and OS input delivery before `t_input` (see §3).
- Bluetooth audio or Bluetooth MIDI; both are unsupported for latency work.
- Acoustic monitoring through speakers; the reference path is wired Aux In.
- MIDI clock sync or DAW round-trip latency (post-v1).
- Long-run drift under thermal throttling (see the 8-hour soak, E11-T06).
