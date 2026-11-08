Implement the foundation/platform beads for the Drumpad v1 WBS.

Beads: E01-T02 (Windows CI), E01-T03 (logging, error model, crash capture), E01-T04 (app paths, settings, portable mode), E01-T05 (release profile, icon, portable ZIP), E01-T07 (ADRs and developer setup guide).

The orchestrator already created the Cargo workspace and crate stubs; `cargo build --workspace` passes. Verify E01-T01 and fix only what blocks these beads.

Own and reserve the paths listed in your team YAML. The app shell/UI, audio/engine, model/samples, input, and QA crates belong to other pairs. Do not edit them.

For each bead: claim it, implement it, run the relevant checks, update the acceptance criteria, and close it only when the criteria are met. Run `br sync --flush-only` after tracker changes. Do not commit or push.
