# Architecture decision records

Each ADR records one cross-cutting choice, the alternatives considered, and the
consequences the team accepts. ADRs are immutable once accepted: supersede one
with a new numbered record instead of editing history.

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-audio-backend.md) | cpal WASAPI shared mode first; exclusive mode only if measurements require it | Accepted |
| [0002](0002-ui-stack.md) | winit plus egui, integrated directly | Accepted |
| [0003](0003-keyboard-latency-path.md) | Physical key codes through a custom winit event-loop path | Accepted |
| [0004](0004-kit-format.md) | Versioned JSON kit manifests with relative paths and SHA-256 hashes | Accepted |
| [0005](0005-sample-storage.md) | Preload samples into RAM behind a content-addressed cache | Accepted |
| [0006](0006-local-only-diagnostics.md) | Local-only logs and crash reports, no telemetry | Accepted |
| [0007](0007-release-profile.md) | LTO release profile, embedded resources, portable ZIP | Accepted |

## Adding an ADR

1. Copy the structure of an existing record.
2. Number it with the next free four-digit prefix.
3. Keep it to the decision, alternatives, and consequences.
4. Link it from this table in the same pull request.
