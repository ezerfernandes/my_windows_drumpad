# QA docs index

Owner: cleaner (`drumpad-qa`). These documents define the release gates for Drumpad v1. They are written against `docs/superpowers/specs/2026-11-07-windows-drum-sampler-design.md`; anything that depends on the shipping build is marked `[verify]`.

| File | Bead | What it covers |
| --- | --- | --- |
| `test-plan.md` | E11-T02…T07 | Test tiers, determinism rules, bead-to-gate map, tolerances, CI shape |
| `mvp-smoke.md` | MVP | Human-runnable launch/play/panic/overlay checklist with run record |
| `review-checklist.md` | all QA handoffs | Mechanical review gate and sign-off record |
| `latency-protocol.md` | E11-T01 | Targets, tolerances, loopback protocol (owned by coder) |
| `latency-baseline.md` | E11-T01 | Hardware baseline table (owned by coder) |
| `compatibility-matrix.md` | E11-T06 | OS, audio, MIDI, keyboard, rollover matrix |
| `soak-test-procedure.md` | E11-T06 | 8-hour soak load, metrics, pass criteria, results template |
| `user-manual.md` | E11-T08 | Install, setup, playing, Nitro Max chain, troubleshooting |
| `license-audit.md` | E11-T09 | Dependency/sample license policy, notices, installer/signing |
| `release-checklist.md` | E11-T10 | Candidate identity, build/test/functional gates, sign-off, rollback |
| `beta-program.md` | E11-T10 | Tester profile, issue template, latency report, triage, sign-off |
| `known-issues.md` | E11-T06/T10 | Accepted limitations, waivers, workarounds |
| `acceptance-evidence.md` | all QA beads | AC checkbox → proof mapping |

## Status

Documents are drafts until the build exists. The blockers are real, not clerical: E11-T05/T06/T07 need the UI and soak tooling, E11-T08 is blocked by the first-run wizard and the Nitro Max template, and E11-T09/T10 need the foundation release profile and a measured candidate. Nothing here is marked pass on hardware that was not run.

## Rules

- A row that was not measured says `pending`. A waiver needs a named owner and a reason.
- Numbers come from artifacts in the repo, not from memory.
- Tolerance or target changes require a note in this file and a thread post.
- Keep this table current when a document is added or renamed.
