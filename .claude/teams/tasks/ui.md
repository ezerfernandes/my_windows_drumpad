Implement the app shell and pad UI beads for the Drumpad v1 WBS.

Beads: E08-T01 through E08-T11 (winit/egui shell, navigation, pad grid, inspector, kit browser, settings UI, performance mode, visual design/errors, drag/drop, first-run wizard, keyboard map overlay) and E09-T01 through E09-T06 (waveform widget, trim/audition, fades/normalize/gain/reverse, loop points, non-destructive edit model, undo/redo and pitch).

Start with E08-T01 (custom winit + egui app shell) and E08-T03 (pad grid), then work down the dependencies. Expose an `app::run()` entry point; the foundation pair owns `main.rs`.

Reference UI:
- 16 pads in a 4x4 grid.
- Finding 35-key keyboard: pads 1-4/Q-R/A-F/Z-V, F1 panic, F2 metronome, F3 previous kit, F4 next kit, F5 map overlay, 5/T/G/B/Space unmapped.
- Dark, high-contrast stage theme. Performance mode shows the F1-F5 transport row and a map overlay.

Own and reserve `crates/drumpad-app/Cargo.toml`, `crates/drumpad-app/src/app/**`, `crates/drumpad-app/src/ui/**`, and `crates/drumpad-app/src/state/**`. Do not edit `main.rs`, `logging.rs`, or `settings.rs`; the foundation pair owns them. The model, samples, engine, audio, and input crates belong to other pairs; agree on public interfaces through your thread and the orchestrator.

For each bead: claim it, implement it, run the relevant checks, update the acceptance criteria, and close it only when the criteria are met. Run `br sync --flush-only` after tracker changes. Do not commit or push.
