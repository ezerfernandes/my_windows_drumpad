# Acceptance evidence map

For each QA bead, this maps every acceptance-criteria checkbox to the artifact or command that proves it. A checkbox is only ticked when the evidence exists in the repo and was produced by a run, not by intent.

Last updated: 2026-11-07 (MVP smoke run). Verified evidence is named; unmeasured hardware rows stay `pending`.

## E01-T06 — Benchmark and offline render harness

| AC | Evidence | Status |
| --- | --- | --- |
| `cargo bench` reports stable baseline numbers | `cargo bench -p drumpad-tools --bench audio` (10 samples): voice mixing 706–764 µs / 8×128-frame batch, resampling 1.03–1.18 ms, kit parse 94–101 µs. Numbers reported; a fixed-parameter three-run delta is still needed for "stable". | **pass (numbers)**, stability pending |
| Offline renderer produces deterministic output for fixtures | Two consecutive `offline_render standard` runs → byte-identical WAV (`0AB11F29…B8EF`) and identical `stats.output_sha256`; `fixture_gen --check` → 6 files match `tests/fixtures/manifest.json` | **pass** |
| The harness runs without an audio device | `cargo test -p drumpad-tools` (37 lib + 9 fake + 7 golden + 8 mvp) and every CLI above run without opening a device | **pass** |

## E11-T01 — Latency budget and measurement protocol

| AC | Evidence | Status |
| --- | --- | --- |
| Targets and tolerances are written down | `docs/qa/latency-protocol.md` §1 matches design doc §6: p99 < 2 ms key→queue (max ≤ 4 ms), p99 ≤ 1 block MIDI→callback, 128 frames @ 48 kHz = 2.667 ms, p99 < 10 ms end to end (max ≤ 15 ms); nearest-rank percentiles, warm-up, outlier and repeat rules stated | **pass** |
| Protocol includes the Finding 35-key keyboard and the Nitro Max Aux In chain | same doc §2 names Finding 35-key → Windows 11 x64 → WASAPI shared → onboard 3.5 mm → Nitro Max Aux In, Local Mode OFF, loopback equipment, marker train, drift fit | **pass** |
| Baseline results are stored with hardware details | `crates/drumpad-tools/baselines/latency-baseline.json` + `docs/qa/latency-baseline.md`: offline probe row measured on this machine (`key_to_queue` p99 0.0003 ms, `midi_to_callback` p99 0.250 ms); the four required physical rows say `pending` | **partial** — physical rows pending, AC not closeable on them |

## E11-T02 — Core unit test suite

| AC | Evidence | Status |
| --- | --- | --- |
| Core coverage ≥ 80 % | `cargo llvm-cov --workspace --lib --summary-only` not yet run | pending |
| Tests run in under 30 s | `cargo test --workspace --lib` exits 0 in well under 30 s (app 224, input 155, model 82, engine 51, samples 40, tools 37) | **pass** |
| No test requires audio hardware | every suite above runs headless; no device is opened by `model`, `samples`, `engine`, `input`, `tools` | **pass** |

## E11-T03 — Offline audio golden tests

| AC | Evidence | Status |
| --- | --- | --- |
| Golden tests cover one-shot, gate, loop, choke, limiter | `crates/drumpad-tools/tests/golden_audio.rs`: 7 tests, cases `one_shot`, `gate`, `loop`, `choke`, `limiter`, `pitch`, all pass against `tests/fixtures/golden/*.json` + `.f32` | **pass** |
| Failures identify the affected fixture | assertions print case name, metric, expected vs actual, first differing frame, and both hashes | **pass (by inspection)** — not yet demonstrated by a forced failure |
| Tests stable on CI | no CI run yet; the suite is deterministic locally (committed goldens, no clock/threading) | pending |

## E11-T04 — Fake MIDI and audio integration harness

| AC | Evidence | Status |
| --- | --- | --- |
| A full trigger sequence runs without hardware | `crates/drumpad-tools/tests/fake_harness.rs`: 9 tests pass (note-on/off, velocity 0, running status, malformed bytes, keyboard down/up, overrun steal, all-notes-off drain) | **pass** |
| The harness is part of CI | `.github/workflows/ci.yml` runs `cargo test --workspace --all-targets` on windows-latest | **pass (wiring)** — CI run not yet observed |
| Failures include the sequence and voice state | `failure_dump_names_the_script_log_and_voice_table` asserts the dump contains MIDI script, keyboard script, trigger log, voice table, per-pad state | **pass** |

## E11-T05 — UI smoke tests

| AC | Evidence | Status |
| --- | --- | --- |
| Critical flows have automated coverage | pad grid, inspector, settings, kit browser, keyboard navigation each have a kittest case | pending |
| Snapshots reviewed on change | snapshot policy in `docs/qa/test-plan.md` §3; diff artifact on a deliberate change | pending |
| Tests run in CI without a GPU | software backend, no window; CI run on windows-latest | pending |

## E11-T06 — Compatibility matrix and soak testing

| AC | Evidence | Status |
| --- | --- | --- |
| Windows 11 x64 matrix filled and linked from release notes | `docs/qa/compatibility-matrix.md` rows `pass`/`waived`; release notes link | pending |
| Nitro Max USB-MIDI + onboard 3.5 mm Aux In path verified | matrix NM-1…NM-5 `pass` with the tested module and cable chain recorded | pending |
| Finding 35-key keyboard maps and rolls over correctly | matrix KB-3…KB-5 + §6 rollover row; dropped-tap count recorded | pending |
| Soak shows no leak, crash, or xrun growth | `docs/qa/soak-results/<date>-<build>.md` with 8 h table per `soak-test-procedure.md` §4 | pending |
| Known limitations documented | `docs/qa/known-issues.md` current and copied into release notes | pending |

## E11-T07 — Startup, load, and responsiveness budgets

| AC | Evidence | Status |
| --- | --- | --- |
| Cold start under 2 s | release build, start → first frame, median of 10, artifact JSON | pending |
| Typical kit loads under 1 s | release build, 16 pads × 1 sample, median of 10 | pending |
| UI stays at 60 fps during playback | 60 s trace, p99 ≤ 16.6 ms, no frame > 33 ms | pending |

## E11-T08 — User manual, setup guide, and troubleshooting

| AC | Evidence | Status |
| --- | --- | --- |
| A new user can set up and play from the docs | `docs/qa/user-manual.md` §1–§4 covers install → first run → audio → keyboard; beta exit criterion "≥ 4 of 5 testers succeed without help" | pending (doc drafted) |
| Nitro Max guide covers USB-MIDI, Local Mode, Aux In, hi-hat calibration | manual §5, including note map and calibration steps | pending (doc drafted) |
| Troubleshooting covers device loss and missing samples | manual §11 entries plus relink flow §7 | pending (doc drafted) |

## E11-T09 — Installer, signing, and licenses

| AC | Evidence | Status |
| --- | --- | --- |
| Installer installs and uninstalls cleanly on Windows 11 x64 | `docs/qa/release-checklist.md` §7 run on a clean 24H2 VM, recorded | pending |
| Portable ZIP remains available | release artifacts list contains both installer and ZIP; hashes published | pending |
| All dependencies and samples attributed | generated `THIRD-PARTY-NOTICES.txt` from `Cargo.lock`; `cargo deny check licenses` pass; sample table filled | pending |

## E11-T10 — Beta program and release checklist

| AC | Evidence | Status |
| --- | --- | --- |
| At least five external testers run the beta | beta tracker with 5 completed issue-template reports, distinct hardware | pending |
| Release notes list known issues | release notes copy `docs/qa/known-issues.md` rows | pending |
| A release candidate is signed off or blocked with reasons | `docs/qa/release-checklist.md` §8 filled with decision, blocking failures, waivers | pending |

## Rules for ticking a box

1. Paste the command and the tail of its output into the bead.
2. For hardware rows, record the device model, OS build, and date; a screenshot alone is not evidence of a measurement.
3. If evidence is `pending`, the bead stays open. Do not close on a plan.
4. If a criterion is deliberately waived, record the owner, the reason, and the date in the bead and in `docs/qa/known-issues.md`.
