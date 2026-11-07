# Cleaner review checklist

This is the gate a `drumpad-qa` handoff must pass before the cleaner signs it off. It is deliberately mechanical: run the commands, record the result, and only then read the prose.

Owner: cleaner. Reviewed artifacts: E01-T06, E11-T01, E11-T04 first, then E11-T02/T03/T05/T07.

## 1. Always

- [ ] `git status --short` shows no files outside `crates/drumpad-tools/**`, `tests/**`, `benches/**`, `docs/qa/**` (the two team-owned paths).
- [ ] No edit to a crate owned by another pair; APIs consumed, not changed.
- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo test --workspace` passes.
- [ ] New public API matches `docs/architecture/interfaces.md`, or the thread records the agreed change.
- [ ] No new workspace dependency without a thread post naming it and the reason.
- [ ] No `unwrap()`/`expect()` on device, file, or network I/O in shipping code paths.
- [ ] Tracker: bead claimed, AC updated item by item, `br sync --flush-only` run.

## 2. Determinism

- [ ] Harness runs three times in a row with identical output (same fixture set, same numbers, same ordering).
- [ ] No `Instant::now()`, `SystemTime::now()`, unseeded `rand`, or thread scheduling in assertions.
- [ ] Fixtures are committed and hashed; no generation from wall clock, locale, or machine state.
- [ ] Randomized behavior takes an explicit seed; the seed appears in failure output.
- [ ] Failure output names the fixture/metric and prints expected vs actual.

## 3. Hardware independence

- [ ] `cargo test --workspace` passes with no audio device and no MIDI port attached.
- [ ] Every `#[ignore]` has a reason string pointing at the manual protocol.
- [ ] The fake audio/MIDI backend implements the real interface, not a test-only shortcut.
- [ ] No test opens a WASAPI/WinMM handle, and no test sleeps to wait for a callback.

## 4. E01-T06 — benchmark and offline render

- [ ] `cargo bench -p drumpad-tools --bench audio` produces numbers and completes without a device.
- [ ] Bench names map to the AC: voice mixing, resampling, kit loading.
- [ ] Bench parameters are recorded in the output (block size, sample rate, voice count).
- [ ] Two consecutive runs are within the noise band; the run-to-run delta is written in the bead.
- [ ] `cargo run -p drumpad-tools --bin offline_render -- <fixture> <out.wav>` renders deterministically: two runs produce byte-identical output, or a documented diff.
- [ ] The renderer accepts a trigger sequence file and a config, not hard-coded constants.
- [ ] Exit codes are meaningful: 0 success, non-zero with a typed message on bad args/fixtures.

## 5. E11-T01 — latency budget and measurement protocol

- [ ] Targets match the design doc §6 exactly: key/queue < 2 ms, MIDI-to-callback < 1 block, 128 frames @ 48 kHz = 2.67 ms, end-to-end < 10 ms.
- [ ] Tolerances state what counts as a pass: percentile used (p50/p95/p99), number of hits, warm-up, outlier handling.
- [ ] Protocol names the reference chain: Finding 35-key keyboard → Windows 11 x64 → WASAPI shared → onboard 3.5 mm → Nitro Max Aux In.
- [ ] Local Mode OFF is called out, with the consequence (module internal sounds bypassed, app is the only sound source).
- [ ] Loopback method is physically executable: what plays, what records, how the marker is aligned, how clock drift is handled.
- [ ] Baseline table has columns for hardware, OS build, driver, buffer size, mode, per-percentile numbers, date, and operator.
- [ ] Raw captures are archived with the table, not just summaries.
- [ ] Any baseline row that is not measured says `pending` and the bead is not closed on it.

## 6. E11-T04 — fake MIDI/audio harness

- [ ] One full scripted sequence runs to completion with no hardware.
- [ ] Sequence covers note-on, note-off, velocity 0, running status, malformed bytes, and keyboard down/up.
- [ ] Asserts trigger count per pad, voice count over time, per-block output RMS, and no stuck voices at the end.
- [ ] Failure output dumps the scripted sequence, trigger log, and voice table.
- [ ] Harness is callable from CI and from an integration test in `tests/`.
- [ ] Fake backend can simulate: device loss mid-sequence, queue full, and callback overrun.

## 7. Sign-off record

Append one row per review. Do not overwrite history.

| Date | Bead | Handoff commit/worktree state | Commands run | Result | Notes |
| --- | --- | --- | --- | --- | --- |
| — | — | — | — | — | — |
