# QA test plan — Drumpad v1

Owner: cleaner (`drumpad-qa`). Implementation of the harnesses lives with the coder; this plan defines what the harnesses must prove and how they are gated.

Scope: E11-T02 through E11-T07, plus the verification side of E01-T06 and E11-T01. Hardware-only gates are in the E11-T01 latency protocol and the E11-T06 compatibility matrix; this plan covers everything that must run without a person at the kit.

## 1. Test tiers

| Tier | Name | Runs on | Command | Budget |
| --- | --- | --- | --- | --- |
| 0 | Crate unit tests | CI, no devices | `cargo test --workspace --lib` | < 30 s wall |
| 1 | Offline integration | CI, no devices | `cargo test --workspace --test '*'` | < 60 s wall |
| 2 | Golden audio | CI, no devices | `cargo test -p drumpad-tools --test golden_audio` | < 60 s wall |
| 3 | Bench baselines | CI on `main`, no devices | `cargo bench -p drumpad-tools --bench audio` | < 10 min wall |
| 4 | Soak | Nightly / release candidate, real backend | `cargo run --release -p drumpad-tools --bin soak` | 8 h |
| 5 | Manual hardware | Release candidate | `docs/qa/latency-protocol.md` | per run |

Tier 0–3 must pass on a machine with **no audio device, no MIDI port, and no GPU**. Any test that opens a device belongs in tier 4+ and must be `#[ignore]` by default.

## 2. Determinism rules

These are hard requirements, not preferences. A test that breaks one is a bug in the harness.

1. **No wall clock.** Use a logical sample counter for time. `Instant::now()` is allowed only in tier 4 reporting, never in assertions.
2. **No ambient randomness.** Every randomized path (round-robin, humanization, dither) takes a seeded RNG; tests pass a literal seed.
3. **No sleeps, no thread races.** Offline rendering drives `process()` block by block on one thread. Async tests synchronize on channels with timeouts.
4. **Fixed fixtures.** Golden inputs are checked into `tests/fixtures/` and hashed. Tests must not download or generate fixtures from the system clock, locale, or `%TEMP%` content.
5. **Bit-exact where possible, tolerance where physics decides.** Deterministic DSP should match sample-for-sample; resampling and limiter comparisons may use a documented tolerance (see §5).
6. **Stable ordering.** Report iteration order (fixture names, voice ids) from sorted collections so failure output is diffable.
7. **No `#[ignore]` without a reason string.** Format: `#[ignore = "needs WASAPI device; see docs/qa/latency-protocol.md"]`.

## 3. Bead-to-gate map

### E11-T02 — Core unit test suite

| Gate | Requirement |
| --- | --- |
| Coverage | ≥ 80 % line coverage over `drumpad-model`, `drumpad-samples`, `drumpad-engine`, `drumpad-input` |
| Runtime | Tier 0 completes in under 30 s |
| Hardware | Zero device opens; assert by running with no devices present |
| Content | Kit invariants, mapping conflicts, velocity curves, voice allocation/stealing, queue behavior, sample metadata |

Coverage command (tooling optional, must not gate CI until installed):

```powershell
cargo llvm-cov --workspace --lib --summary-only
```

Minimum test *cases* per area, so coverage is not gamed by one giant test:

- `drumpad-model`: valid/invalid velocity bounds, duplicate pad ids, choke references, key/midi mapping conflicts, config round-trip.
- `drumpad-samples`: mono/stereo decode metadata, corrupt input returns typed error, resample length/edge handling, peak generation matches source.
- `drumpad-engine`: voice allocation and stealing, polyphony cap, choke fade length, gain/pan law, limiter ceiling, queue full/empty behavior.
- `drumpad-input`: trigger/release construction, note-off without note-on, velocity 1 and 127, monotonic timestamps.

### E11-T03 — Offline audio golden tests

Required fixtures, one per path:

| Fixture | Path exercised | Assertions |
| --- | --- | --- |
| `one_shot.wav` | one-shot playback | peak, RMS, first non-zero sample index |
| `gate.wav` | gate + note-off release | release begins within fade window; tail decays |
| `loop.wav` | loop wrap | seam continuity; no discontinuity above threshold |
| `choke.wav` | choke group | older voice fades 5–10 ms; newer voice survives |
| `limiter.wav` | hot full-scale mix | output ≤ 0 dBFS; no sample at or above 1.0 |
| `pitch.wav` | tune/resample | duration ratio within 1 sample per second of expected |

Failure output must name the fixture, the metric, expected vs actual, and the first offending frame. A bare `assert_eq!` over a 2-second buffer is not acceptable.

Golden data policy: store hashes plus small reference buffers (`tests/fixtures/golden/*.f32`) for short renders; long renders store metrics only. Regenerating goldens is a separate, reviewed commit with the before/after metric table in the message.

### E11-T04 — Fake MIDI and audio integration harness

The fake backend must implement the same interfaces as the real one, not a parallel test-only path:

- Fake audio backend: caller-driven `process()` with a configurable block size, no thread, no device, records output blocks.
- Fake MIDI source: scripted bytes/events fed in deterministic order, including note-on, note-off, velocity 0, running status, and malformed input.
- Scripted keyboard source: ordered key down/up events with logical timestamps.
- Assertions: trigger count per pad, voice count over time, output RMS per block, no stuck voice after the sequence.
- Failure output: dump the scripted sequence, the resulting trigger log, and the voice table (pad, start block, state) for the failing block.

### E11-T05 — UI smoke tests

- Use `egui_kittest` with the software backend; no GPU and no window may be required.
- Flows: pad grid renders 16 pads, click triggers a preview, inspector edits a value, settings round-trip, kit browser lists and loads, keyboard navigation moves selection.
- Snapshot policy: committed snapshots, reviewed on change, diff images published as CI artifacts on failure.
- Time budget: whole suite under 60 s; a single flow's timeout fails the test rather than hanging CI.

### E11-T06 — Compatibility matrix and soak

Doc-level gates live in `docs/qa/compatibility-matrix.md` and `docs/qa/soak-test-procedure.md`. The automated part is the soak runner:

- Duration 8 h, release build, fake + real backend modes.
- Traffic: continuous 16-pad rolls with random velocities, choke pairs, gate note-offs, MIDI hotplug every 10 min, UI animation at 60 fps, kit switches every 15 min.
- Sampled every 10 s: process working set, handle count, thread count, xrun count, callback load p99, voice count.
- Pass: no crash, no unbounded RSS growth (allow ≤ 5 % drift after warm-up), handles/threads flat, xrun count not increasing across the last hour, no stuck voices after each hotplug.
- Output: `docs/qa/soak-results/<date>-<build>.md` plus the raw CSV.

### E11-T07 — Startup, load, and responsiveness budgets

| Metric | Target | Measurement |
| --- | --- | --- |
| Cold start to first frame | < 2.0 s | release build, process start → first `process()` call, median of 10 |
| Typical kit load | < 1.0 s | release build, 16 pads × 1 sample, median of 10 |
| UI frame time during playback | ≤ 16.6 ms p99 | 60 s playback trace, no frame over 33 ms |

These are measured by a harness in `crates/drumpad-tools` that writes a table to stdout and a JSON artifact; a budget miss fails with the measured value in the message.

## 4. CI shape

```yaml
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace --lib
- cargo test --workspace --test '*'
- cargo bench -p drumpad-tools --bench audio -- --warm-up-time 1 --measurement-time 2 --sample-size 10
```

Bench gating starts as a report-only job on `main`; it becomes a hard gate once three consecutive baselines are within 10 %. Do not gate pull requests on wall-clock numbers from a shared runner.

## 5. Tolerances

| Comparison | Tolerance |
| --- | --- |
| Deterministic DSP, same build | bit-exact |
| Golden one-shot / gate | peak ±0.5 dB, RMS ±0.5 dB, onset ±1 sample |
| Resample length | ±1 frame per 1 s of audio |
| Limiter ceiling | < 1.0 absolute, no NaN/Inf |
| Peak cache | max abs error ≤ 1e-6 |
| Meter vs reference | ±1 dB |

Tolerance changes require a note in `docs/qa/README.md` explaining why.

## 6. Definition of done for any QA harness

1. Runs with no audio device, MIDI port, or GPU unless it is explicitly tier 4+.
2. Deterministic across at least three consecutive runs on the same machine.
3. Failure message identifies the fixture, the metric, and expected vs actual.
4. Wired into the CI list above, or has a documented reason it is manual.
5. No `unwrap()` on device or file I/O; typed errors surface as test failures with context.
6. Measured runtime is recorded in the bead before closing it.
