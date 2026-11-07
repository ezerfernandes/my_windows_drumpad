# Acceptance evidence map

For each QA bead, this maps every acceptance-criteria checkbox to the artifact or command that proves it. A checkbox is only ticked when the evidence exists in the repo and was produced by a run, not by intent.

Last updated: 2026-11-07 (before implementation landed — evidence placeholders are intentionally `pending`).

## E01-T06 — Benchmark and offline render harness

| AC | Evidence | Status |
| --- | --- | --- |
| `cargo bench` reports stable baseline numbers | `cargo bench -p drumpad-tools --bench audio` output pasted in the bead; two consecutive runs with delta; parameters (block, rate, voices) printed in the bench id | pending |
| Offline renderer produces deterministic output for fixtures | `cargo run -p drumpad-tools --bin offline_render -- --sequence tests/fixtures/sequences/one_shot.json --out a.wav` twice; `Get-FileHash a.wav,b.wav` equal; fixture list committed | pending |
| The harness runs without an audio device | tier 0–3 run on a machine with no output device; `cargo test --workspace` and the render command both pass with devices disabled | pending |

## E11-T01 — Latency budget and measurement protocol

| AC | Evidence | Status |
| --- | --- | --- |
| Targets and tolerances are written down | `docs/qa/latency-protocol.md` §targets matches design doc §6: < 2 ms key→queue, < 1 audio block MIDI→callback, 128 frames @ 48 kHz = 2.67 ms, < 10 ms end to end; percentile and outlier rules stated | pending |
| Protocol includes the Finding 35-key keyboard and the Nitro Max Aux In chain | same doc names the full chain (Finding keypad → Windows 11 → WASAPI shared → onboard 3.5 mm → Nitro Max Aux In), Local Mode OFF, loopback method, drift handling | pending |
| Baseline results are stored with hardware details | `docs/qa/latency-baseline.md` rows with OS build, driver, device, buffer, mode, p50/p95/p99, date, operator; raw captures archived; unmeasured rows say `pending` | pending |

## E11-T02 — Core unit test suite

| AC | Evidence | Status |
| --- | --- | --- |
| Core coverage ≥ 80 % | `cargo llvm-cov --workspace --lib --summary-only` report, pasted; scope model/samples/engine/input | pending |
| Tests run in under 30 s | `cargo test --workspace --lib` wall time on CI, pasted | pending |
| No test requires audio hardware | run on a machine with no output/MIDI device; zero device opens | pending |

## E11-T03 — Offline audio golden tests

| AC | Evidence | Status |
| --- | --- | --- |
| Golden tests cover one-shot, gate, loop, choke, limiter | fixture table in `docs/qa/test-plan.md` §3 each has a passing test; test names map 1:1 | pending |
| Failures identify the affected fixture | failure message shows fixture path, metric, expected vs actual, first offending frame; demonstrated by a temporary forced failure | pending |
| Tests stable on CI | three consecutive CI runs identical; no wall-clock/threading in assertions | pending |

## E11-T04 — Fake MIDI and audio integration harness

| AC | Evidence | Status |
| --- | --- | --- |
| A full trigger sequence runs without hardware | `cargo test --workspace --test fake_backend` (or equivalent) on a device-less machine | pending |
| The harness is part of CI | job runs the suite on windows-latest; link to run | pending |
| Failures include the sequence and voice state | forced-failure run shows scripted sequence, trigger log, voice table | pending |

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
