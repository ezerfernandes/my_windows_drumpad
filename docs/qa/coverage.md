# Coverage gate

Owner: QA pair (`coder-8` / `cleaner-8`). This is the mechanical gate for the coverage freeze; it is not a substitute for the behavioral gates in `docs/qa/test-plan.md`.

## Gate

```powershell
cargo llvm-cov --workspace --summary-only
```

The **`TOTAL`** row's **line coverage must be > 80.00%**. A test failure aborts `cargo llvm-cov` before it prints a summary, so "all workspace tests pass" is a prerequisite for the number to exist at all.

Current CI (`.github/workflows/ci.yml`) does **not** run coverage; the gate is manual for this campaign. If it is meant to survive the campaign, it needs a CI step owned by the foundation pair.

## Baseline (task-provided, arithmetically consistent)

| Crate | Line coverage | Lines |
| --- | ---: | ---: |
| drumpad-app | 70.91% | 16,286 |
| drumpad-audio | 62.23% | 4,925 |
| drumpad-tools | 62.54% | 4,170 |
| drumpad-model | 83.12% | 5,817 |
| drumpad-samples | 85.48% | 4,270 |
| drumpad-input | 92.89% | 6,513 |
| drumpad-engine | 93.90% | 3,509 |
| **TOTAL** | **77.05%** | **45,490** |

Cross-checks: the per-crate line counts sum to 45,490, and the implied covered lines sum to 35,051 (`35,051 / 45,490 = 77.0521%`). The baseline is internally consistent.

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

## Wave log

Each row is a gate run on a named tree state. A run on a dirty, mid-edit tree is recorded as **invalid** and not used for sign-off.

| Wave | Tree | TOTAL lines | Result | Notes |
| --- | --- | ---: | --- | --- |
| baseline | task-provided (`896afa5` / `ed48b7f`) | 77.05% | below gate | 35,051 / 45,490 |
| 1 | `15ceb1c` + dirty waves | — | **invalid** | `cargo llvm-cov` aborted: two app UI tests failed on a pre-commit snapshot; current tree passes 265/0/1 |
| 1 | shared tree 07:47Z | — | **invalid** | `drumpad-tools` bin test targets mid-edit (missing named lifetime in `fixture_gen.rs` test helper, since fixed); 40 files dirty across app/audio/input/model/samples/tools |
| — | pending | — | — | definitive run once all waves commit and the tree is frozen |

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
