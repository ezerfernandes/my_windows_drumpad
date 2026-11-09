# Coverage freeze report

Owner: `drumpad-qa` (coder-8 / cleaner-8). This is the gate report for the
coverage-freeze campaign. The mechanical gate is:

```powershell
cargo llvm-cov --workspace --summary-only
```

The `TOTAL` row's **line coverage must be greater than 80.00%**. Baseline at
campaign start: **77.05%** (`35,051 / 45,490` lines). Hardware tests stay
`#[ignore]`; no product behavior may change in this campaign.

## Workspace gate status

| Run | Command | TOTAL line coverage | Result |
| --- | --- | ---: | --- |
| Baseline | `cargo llvm-cov --workspace --summary-only` | no TOTAL row | **blocked** by failing tests before report generation |
| Final | see [Latest gate](#latest-gate) | pending | pending |

The baseline workspace command aborts in two places that are **not** owned by
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

## Latest gate

<!-- GATE_RESULT -->

## Commits

| Commit | Contents |
| --- | --- |
| `01b16b1` | `test(tools): cover fixture/offline/probe/soak CLI behavior` |
| `3fd148f` | `test(tools): cover fixture and sequence behavior` |

No push. Commits are path-scoped to `crates/drumpad-tools/**`.

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
