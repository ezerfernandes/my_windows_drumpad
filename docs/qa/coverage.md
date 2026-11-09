# Coverage gate

Owner: QA pair (`coder-8` / `cleaner-8`). This is the mechanical gate for the
coverage freeze; it is not a substitute for the behavioral gates in
`docs/qa/test-plan.md`.

## Gate

```powershell
cargo llvm-cov --workspace --bins --tests --summary-only
```

The `TOTAL` row's **line coverage must be greater than 80.00%**.

`--bins --tests` is required: it builds the package binaries that integration
tests launch through `CARGO_BIN_EXE_*` (the latency CLI tests), while skipping
the 10-minute peak-build bench. A test failure aborts `cargo llvm-cov` before it
prints a summary, so all workspace tests must pass first.

Current CI (`.github/workflows/ci.yml`) does **not** run coverage; the gate is
manual for this campaign. If it is meant to survive the campaign, it needs a CI
step owned by the foundation pair.

## Final result

Run on code tree `c6bd796` (the two docs commits after it are documentation
only). All tests passed; `cargo llvm-cov` exited 0.

| Crate | Lines | Covered | Line coverage |
| --- | ---: | ---: | ---: |
| drumpad-app | 16,842 | 13,939 | 82.76% |
| drumpad-audio | 5,827 | 4,248 | 72.90% |
| drumpad-tools | 5,880 | 5,450 | 92.69% |
| drumpad-model | 6,927 | 6,245 | 90.15% |
| drumpad-samples | 4,920 | 4,455 | 90.55% |
| drumpad-input | 7,733 | 7,551 | 97.65% |
| drumpad-engine | 3,509 | 3,295 | 93.90% |
| **TOTAL** | **51,638** | **45,183** | **87.50%** |

Functions: 85.29%. Regions: 86.85%. The gate is **PASS** (87.50% > 80.00%).

The line counts use the same `cargo llvm-cov` method as the baseline, so the
comparison is like-for-like. The baseline was **77.05%** (35,051 / 45,490); the
campaign added **10.45 percentage points**, with the largest gains in the app
(70.91% -> 82.76%), audio (62.23% -> 72.90%), tools (62.54% -> 92.69%), and
model (83.12% -> 90.15%).

## Audit of the measurement

| Check | Result |
| --- | --- |
| `#[coverage(off)]` anywhere | none |
| `#[cfg(not(test))]` exclusions | none |
| `.cargo/config.toml` or CI flags hiding files | none |
| `llvm-cov` ignore-filename options | none |
| Ignored tests | 5, all with reasons: 4 `requires a Windows audio render device`, 1 `10-minute peak build; run with --release` |
| Deleted or weakened product code for coverage | none found in reviewed commits |
| Test style | behavior-focused: public APIs, emitted `Action`/`Effect`s, state transitions, error paths, file round-trips, CLI stdout/stderr/exit codes; no private-field, call-sequence, or pixel-snapshot assertions |

## Test-quality gate

- Tests assert public behavior: inputs/outputs, state transitions, error paths,
  emitted `Action`/`Effect`s, file round-trips, CLI stdout/stderr/exit codes,
  serialization.
- No assertions on private fields, internal call order, or pixel snapshots. UI
  tests drive `egui::Context::run` and assert actions/state.
- Deterministic: no wall clock, network, real audio device, or long sleeps.
  Hardware tests stay `#[ignore]` with a reason.
- Tests must fail for a real behavior change, not because a layout constant
  moved.

## Reproduce

```powershell
# Full gate (must produce a TOTAL row > 80.00%)
cargo llvm-cov --workspace --bins --tests --summary-only

# Behavior tests only
cargo test --workspace
```

## Notes

- The two earlier UI test failures were fixed before the final run; both pass.
- The latency CLI integration tests now run because the gate builds the
  package binaries with `--bins`.
- The coverage line counts include instrumented `#[cfg(test)]` modules, exactly
  as the baseline did. No exclusions were added to game the denominator.
- The campaign is frozen; no feature or latency work is part of this gate.
