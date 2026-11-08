Implement the audio backend and real-time playback engine beads for the Drumpad v1 WBS.

Beads: E02-T01 through E02-T07 (WASAPI backend, device lifecycle, callback, diagnostics) and E03-T01 through E03-T08 (sample/voice model, lock-free queues, voice pool, one-shot/gate/loop playback, gain/pan/choke, limiter, metering, RT-safety).

Start with E02-T01 (backend spike and latency measurements) and E03-T01 (sample buffer and voice model), then work down the dependencies. Use Windows 11 x64, 48 kHz, 128-frame target, WASAPI shared mode first, exclusive mode only if measurements justify it.

Own and reserve `crates/drumpad-audio/**`, `crates/drumpad-engine/**`, and `docs/audio/**`. The model/samples crates belong to another pair. Agree on public sample/model interfaces through your thread and the orchestrator; do not edit their files. The app crate belongs to the UI pair.

For each bead: claim it, implement it, run the relevant checks, update the acceptance criteria, and close it only when the criteria are met. Run `br sync --flush-only` after tracker changes. Do not commit or push.
