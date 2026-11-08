# Latency autoresearch — independent verification record

Role: `cleaner` (`CreamPlateau`), team `two-pack`, run
`TEAM-two-pack-20261108073433-e73a`.
Coder: `OliveCrane`. Orchestrator: `RoseMeadow`.

This folder holds the **cleaner's** evidence: the pinned proxy definition, the
device capability table, and one verification record per reviewed experiment.
The coder owns the loop log (`loop-*.tsv`), `summary.md` and `experiments.md`
at the top level of `docs/qa/latency-autoresearch/`.

## Metric of record

```
proxy_e2e_p99_ms = key_to_queue_p99_ms + callback_period_ms + buffer_ms
```

Direction: **lower is better**. `p50` is reported with the same construction.

- `key_to_queue_p99_ms` is measured live by
  `crates/drumpad-tools/src/bin/latency_proxy.rs`: 1200 hits through the real
  input trigger, engine queue and mixer with a caller-driven fake audio
  backend, 100 warm-up hits discarded, nearest-rank percentiles from
  `drumpad_tools::latency`.
- `callback_period_ms` and `buffer_ms` are the **negotiated** values for the
  app's default stream request, looked up in
  `device-capabilities.json`. That table is transcribed from real hardware in
  `docs/audio/backend-spike.md` (E02-T01, 2026-11-07, Realtek onboard
  endpoint). A request that was never measured fails closed instead of being
  extrapolated.

The default request is read from `drumpad_audio::AudioSettings::default()`
with the same sample-rate and block pin that `MvpAudioRuntime::start` applies.
`MvpAudioRuntime::start()` passes no settings path, so that default is exactly
what the app requests.

### Why a proxy

The real metric is a physical loopback (`docs/qa/latency-protocol.md` §5): it
needs an analog capture chain and exclusive device access. The human's live app
(PID 58392) owns the output, so the loop must not open an exclusive stream.
The proxy keeps the software half measured and the audio half negotiated, and
it is identical across every iteration so the comparison is fair.

### What the proxy does not measure

- OS input stack and USB keyboard scan interval before `t_input`.
- WASAPI submission jitter and late callbacks.
- DAC, analog output, cable, and the Nitro Max Aux In.
- Per-endpoint differences other than the one capability table.

**Fallback caveat.** The proxy reads the *request*. If the endpoint is busy or
rejects integer exclusive formats, the app falls back to shared (34.1 ms) and
the status bar reports `shared` plus the fallback reason. The proxy would still
show 10.667 ms in that case, so the final hardware run must confirm the
negotiated `share_mode: exclusive` before anyone claims the win.

**Smallest useful gain.** 0.05 ms. `key_to_queue` p99 moves by a few
ten-thousandths of a millisecond between runs, so anything smaller is noise and
cannot justify keeping a code change.

A proxy win is a candidate, not acceptance. Final acceptance requires the
hardware run in `crates/drumpad-audio/tests/hardware.rs` inside one approved
stop/relaunch window, plus the full guard.

## Verify command

```text
cargo run -q -p drumpad-tools --bin latency_proxy -- --json docs/qa/latency-autoresearch/iteration-<N>.json
```

stdout is exactly one line: `proxy_e2e_p99_ms=<number>`. Components go to
stderr; add `--detail` to print them on stdout.

## Iteration 0 — baseline

Measured from a pristine detached worktree at `896afa5`, with only the
verifier added (the coder's working-tree changes were not present).

| Field | Value |
| --- | --- |
| commit | `896afa5` |
| mode | shared |
| requested frames | 128 |
| negotiated callback period | 512 frames (10.6667 ms) |
| negotiated buffer | 1126 frames (23.4583 ms) |
| key_to_queue p50 / p99 | 0.0001 ms / 0.0006 ms |
| midi_to_callback p99 | 0.6560 ms |
| **proxy_e2e p50** | **34.1251 ms** |
| **proxy_e2e p99** | **34.1256 ms** |

Raw record: `docs/qa/latency-autoresearch/iteration-0.json`.
Corroborating offline probe (same commit, `latency_probe`, 1200 hits):
`docs/qa/latency-autoresearch/baseline-probe.json`.

## Acceptance gate for every candidate

A candidate is only kept when **all** of these hold:

1. `proxy_e2e_p99_ms` improves and the negotiated values in the JSON are the
   ones the code actually asks for.
2. `cargo fmt --all --check` passes.
3. `cargo test -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools`
   passes.
4. `cargo clippy -p drumpad-app -p drumpad-audio -p drumpad-engine -p drumpad-input -p drumpad-tools --all-targets -- -D warnings`
   passes.
5. The change preserves behavior: the demo kit still loads, pads still trigger
   audible voices on the fake backend, exclusive-unavailable still falls back
   to shared, and the UI still reports the negotiated mode and buffer.
6. The diff contains no unrelated rewrites, no `.beads/`, no `target/`, no
   `dist/`, no logs.

A metric win alone is not acceptance.
