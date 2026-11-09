# Coverage gate

Owner: QA pair (`coder-8` / `cleaner-8`). This is the mechanical gate for the
coverage freeze; it is not a substitute for the behavioral gates in
`docs/qa/test-plan.md`.

## Gate

```powershell
cargo llvm-cov --workspace --summary-only
```

The **`TOTAL`** row's **line coverage must be > 80.00%**. A test failure aborts
`cargo llvm-cov` before it prints a summary, so "all workspace tests pass" is a
prerequisite for the number to exist at all.

Current CI (`.github/workflows/ci.yml`) does **not** run coverage; the gate is
manual for this campaign. If it is meant to survive the campaign, it needs a CI
step owned by the foundation pair.

## Baseline (task-provided, arithmetically consistent)

| Crate | Line coverage | Lines |
| --- | ---: | --- |
| drumpad-app | 70.91% | 16,286 |
| drumpad-audio | 62.23% | 4,925 |
| drumpad-tools | 62.54% | 4,170 |
| drumpad-model | 83.12% | 5,817 |
| drumpad-samples | 85.48% | 4,270 |
| drumpad-input | 92.89% | 6,513 |
| drumpad-engine | 93.90% | 3,509 |
| **TOTAL** | **77.05%** | **45,490** |

Cross-checks: the per-crate line counts sum to 45,490, and the implied covered
lines sum to 35,051 (`35,051 / 45,490 = 77.0521%`). The baseline is internally
consistent.

## Audit of the measurement (cleaner)

| Check | Result |
| --- | --- |
| `#[coverage(off)]` anywhere | none |
| `#[cfg(not(test))]` exclusions | none |
| `.cargo/config.toml` or CI flags hiding files | none |
| `llvm-cov` ignore-filename options | none |
| Ignored tests | 5, all with reasons: 4 `requires a Windows audio render device`, 1 `10-minute peak build; run with --release` |
| Deleted or weakened product code for coverage | none found in reviewed commits |
| Test style, tools crate | behavior-focused: CLI in/out, parse errors, determinism, WAV round-trips, golden metrics/hashes; no private-field or call-sequence assertions |

## Workspace gate status

| Run | Command | TOTAL line coverage | Result |
| --- | --- | ---: | --- |
| Baseline | `cargo llvm-cov --workspace --summary-only` | no TOTAL row | **blocked** by failing tests before report generation |
| Wave 1 | full gate attempt | no TOTAL row | **invalid**: blocked by app UI tests + `latency_cli` (see below) |
| Wave 2 | full gate attempt | no TOTAL row | **invalid**: instrumented build killed twice (`GATE_EXIT=-1`, no profraw); retry running |
| Final | see [Latest gate](#latest-gate) | pending | pending |

The workspace command currently aborts in two places that are **not** owned by
QA/tools:

1. `drumpad-app`: `ui::coverage_tests::clicking_a_pad_in_performance_mode_triggers_it`
   and `ui::coverage_tests::dragging_in_the_waveform_creates_an_ordered_selection`
   fail ("drag should leave a selection"). Owner: UI pair (coder-7 / cleaner-7).
2. `crates/drumpad-tools/tests/latency_cli.rs`: 27 tests fail under coverage
   with `cannot find the file specified (os error 2)` at `latency_cli.rs:43`
   (the helper cannot launch the `latency_proxy`/`latency_report` binaries from
   the coverage target directory). Owner: latency pair (coder-9 / cleaner-9).

Until both are fixed, `cargo llvm-cov --workspace` cannot produce a TOTAL row
and the gate cannot be signed off. Package-scoped numbers below are the
authoritative QA/tools evidence in the meantime.

## QA/tools package

Target: `drumpad-tools` **≥ 75%** line coverage. The package-scoped command
excludes the two latency-pair bins (owned by coder-9) and their in-progress
integration test, so the number measures the QA/tools ownership surface:

```powershell
cargo llvm-cov -p drumpad-tools --lib --bins `
  --test fake_harness --test golden_audio --test mvp_effects --summary-only
```

| Wave | Commit | Tools TOTAL lines | Notes |
| --- | --- | ---: | --- |
| Baseline | `8458ca6` | **70.68%** (1,962 / 2,776) | before this campaign's tests |
| 1 | `01b16b1` | **89.34%** | bins covered; measurement included latency bins |
| 2 | `3fd148f` | **84.06%** (2,985 / 3,551) | authoritative surface: lib + owned bins + three owned integration tests |

The wave-1 figure used `--skip proxy_ --skip report_` on the whole package and
therefore also ran the latency-pair binaries' own tests, which inflates the
package total. Wave 2 selects the owned test targets explicitly; **84.06% is
the number to trust** for the QA/tools surface.

### Per-file coverage (wave 2)

| File | Baseline | Wave 2 | Target |
| --- | ---: | ---: | --- |
| `src/bin/fixture_gen.rs` | 0.00% | **92.49%** | CLI behavior |
| `src/bin/latency_probe.rs` | 0.00% | **92.65%** | CLI behavior |
| `src/bin/offline_render.rs` | 0.00% | **95.73%** | CLI behavior |
| `src/bin/soak.rs` | 0.00% | **75.00%** | stub failure only |
| `src/fixture.rs` | 75.14% | **92.83%** | WAV/manifest/error paths |
| `src/sequence.rs` | 77.86% | **98.31%** | parser/load/validation |
| `src/fake.rs` | 84.60% | 84.60% | MIDI/keyboard harness |
| `src/offline.rs` | 85.99% | 89.61% | renderer |
| `src/latency.rs` | 94.95% | 94.23% | stats/baseline model |

`bin/latency_proxy.rs` and `bin/latency_report.rs` are excluded from the table:
they belong to the latency pair and are covered by their own tests.

### Behavior covered in this campaign

- **`fixture_gen`:** help, unknown option, missing option value, default root,
  generation into a temp dir, `--check` success, tampered fixture mismatch,
  missing fixture, blocked root directory.
- **`offline_render`:** help, wrong positional count, unknown option, missing
  option values, unreadable/invalid config JSON, invalid config values,
  missing sequence, unknown fixture, missing WAV fixture, JSON report schema,
  WAV output, byte-identical repeated renders, WAV-path fixture input,
  stdout report, unwritable output/report paths.
- **`latency_probe`:** help, unknown/missing/below-minimum arguments, default
  paths, one sample per hit, JSON baseline (1 measured + 3 pending), raw CSV
  row count, Markdown rendering, exit 2 on a budget miss, unwritable
  JSON/CSV/Markdown paths.
- **`soak`:** the stub fails with its stable message and exit 1.
- **`fixture`:** unknown lookups, single-fixture playback modes, zero-rate
  synthesis, integer WAV scaling, empty/wide/missing WAV errors, blocked
  parent writes, SHA-256 file/buffer helpers, resample wrapper, manifest JSON
  round-trip, typed error display.
- **`sequence`:** file load and IO source, `from_events` validation with
  event positions, text round-trip for every action, keyword aliases, every
  parse error, error display, empty/non-empty accessors, due-cursor, action
  targets.

All tests are deterministic: temp dirs, no wall-clock assertions, no audio
device, no network. The latency probe's CLI smoke run accepts either documented
verdict exit (0 or 2) and asserts only that outputs are written; its
budget/decisive tests use synthetic samples so a loaded machine cannot make the
suite flaky.

## Wave log

Each row is a gate run on a named tree state. A run on a dirty, mid-edit tree is
recorded as **invalid** and not used for sign-off.

| Wave | Tree | TOTAL lines | Result | Notes |
| --- | --- | ---: | --- | --- |
| baseline | task-provided (`896afa5` / `ed48b7f`) | 77.05% | below gate | 35,051 / 45,490 |
| 1 | `15ceb1c` + dirty waves | — | **invalid** | `cargo llvm-cov` aborted: two app UI tests failed on a pre-commit snapshot; current tree passes 265/0/1 |
| 1 | shared tree 07:47Z | — | **invalid** | `drumpad-tools` bin test targets mid-edit (missing named lifetime in `fixture_gen.rs` test helper, since fixed); 40 files dirty across app/audio/input/model/samples/tools |
| 1 (tools) | `01b16b1` | 89.34% (tools) | package pass | bins covered; latency-pair tests included |
| 2 (tools) | `3fd148f` | 84.06% (tools) | package pass | authoritative owned surface |
| final | pending | — | — | definitive run once all waves commit and the tree is frozen |

## Rules for the definitive run

1. `git status --short` must be empty (or list only files owned by the wave being measured and disclosed).
2. `cargo test --workspace` must pass first; a red test makes the gate unmeasurable.
3. Paste the whole `TOTAL` row and each crate row into the report; do not round up.
4. Re-run after every wave; record the commit SHA and the tree state next to the number.
5. Never use `--ignore-filename-regex`, `#[coverage(off)]`, deleted code, or ignored tests to move the number.

## Test-quality gate (cleaner review)

- Tests assert public behavior: inputs/outputs, state transitions, error paths, emitted `Action`/`Effect`s, file round-trips, CLI stdout/stderr/exit codes, serialization.
- No assertions on private fields, internal call order, or pixel snapshots. UI tests drive `egui::Context::run` and assert actions/state.
- Deterministic: no wall clock, network, real audio device, or long sleeps. Hardware tests stay `#[ignore]` with a reason.
- Tests must fail for a real behavior change, not because a layout constant moved.

## Latest gate

<!-- GATE_RESULT -->

## Commits

| Commit | Contents |
| --- | --- |
| `01b16b1` | `test(tools): cover fixture/offline/probe/soak CLI behavior` |
| `3fd148f` | `test(tools): cover fixture and sequence behavior` |

No push. Commits are path-scoped to `crates/drumpad-tools/**` and
`docs/qa/coverage.md`.

## Reproduce

```powershell
# Full gate (must produce a TOTAL row > 80.00%)
cargo llvm-cov --workspace --summary-only

# QA/tools surface
cargo llvm-cov -p drumpad-tools --lib --bins `
  --test fake_harness --test golden_audio --test mvp_effects --summary-only

# Behavior tests only
cargo test -p drumpad-tools --lib --bins `
  --test fake_harness --test golden_audio --test mvp_effects
```
