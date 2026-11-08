Implement the test harness, latency protocol, benchmark, and release-gate beads for the Drumpad v1 WBS.

Beads: E01-T06 (benchmark and offline render harness) and E11-T01 through E11-T10 (latency protocol, core tests, golden audio tests, fake MIDI/audio harness, UI smoke tests, compatibility matrix and soak testing, startup/load budgets, user manual/setup guide, installer/signing/licenses, beta and release checklist).

Start with E11-T01 (latency budget and measurement protocol), E01-T06 (benchmark/offline harness), and E11-T04 (fake MIDI/audio harness), then work down the dependencies.

Own and reserve `crates/drumpad-tools/**`, `tests/**`, `benches/**`, and `docs/qa/**`. You may read any crate, but do not edit crates owned by other pairs. Ask the owning team for test hooks or public APIs through your thread and the orchestrator.

Reference hardware:
- Windows 11 x64.
- Finding 35-key single-hand keyboard.
- Alesis Nitro Max over USB-MIDI with Local Mode OFF.
- PC onboard 3.5 mm output into Nitro Max Aux In.
- WASAPI shared mode first, exclusive only if measurements justify it.

For each bead: claim it, implement it, run the relevant checks, update the acceptance criteria, and close it only when the criteria are met. Run `br sync --flush-only` after tracker changes. Do not commit or push.
