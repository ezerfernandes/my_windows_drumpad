Implement the kit/pad model, persistence, and sample pipeline beads for the Drumpad v1 WBS.

Beads: E05-T01 through E05-T08 (domain model, kit format, atomic save/autosave, migrations, CRUD, portable kits, validation, demo kit) and E04-T01 through E04-T07 (decoder, channel handling/resampling, waveform peaks, content cache, async import, drag/drop validation, missing-sample relink).

Start with E05-T01 (domain model and invariants) and E04-T01 (decoder and format support), then work down the dependencies.

Own and reserve `crates/drumpad-model/src/kit/**`, `crates/drumpad-model/src/mappings/**`, `crates/drumpad-model/src/lib.rs`, `crates/drumpad-model/Cargo.toml`, and all of `crates/drumpad-samples/**`. Do not edit `drumpad-model/src/error.rs` or `drumpad-model/src/config/**`; the foundation pair owns them. The engine and app crates belong to other pairs; agree on public types through your thread and the orchestrator.

For each bead: claim it, implement it, run the relevant checks, update the acceptance criteria, and close it only when the criteria are met. Run `br sync --flush-only` after tracker changes. Do not commit or push.
