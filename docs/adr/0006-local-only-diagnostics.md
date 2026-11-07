# ADR 0006: Local-only diagnostics

- Date: 2026-11-07
- Status: Accepted

## Context

A stage instrument must be diagnosable when it fails on a user's machine, but
the app stores sample names and paths that can be personal. The design scope
explicitly excludes telemetry, accounts, and cloud sync. There is no server to
receive crash reports.

## Decision

Diagnostics are local-only.

- `tracing` writes structured logs to daily rolling files in the app log
  directory. Retention defaults to 14 days and is configurable from 1 to 365.
- `thiserror` defines typed errors; the error model lives in `drumpad-model` and
  avoids OS and UI dependencies.
- A panic hook writes a local crash report containing the app version, OS and
  architecture, thread name, panic message, source location, and a backtrace.
  The hook writes a file, so it works in a GUI build with no console. The app
  shell records the active audio and MIDI device summary with
  `set_device_context` after opening streams; the report includes it when set.
  Keeping that seam in the app crate avoids a foundation dependency on the audio
  and input crates.
- Crash reports replace the current user's profile path with `%USERPROFILE%`
  before writing.
- The app contains no telemetry, analytics, or network transport. No user sample
  paths, file names, or personal data are sent anywhere. Users share a log or
  crash file manually if they choose.

Settings are versioned JSON (`schema_version`) with unknown fields and unknown
versions rejected, so a hand edit or a newer build fails visibly instead of
silently changing behavior.

## Alternatives considered

- **Sentry or another crash-reporting service**: excellent triage, but adds a
  network path and an explicit contradiction of the no-telemetry scope.
- **Windows Error Reporting**: collects dumps automatically, but requires
  registration and sends data outside the machine.
- **Windows Event Log**: local, but awkward to bundle and harder for users to
  find than a log folder.
- **No diagnostics**: simplest privacy story, but makes field failures
  impossible to support.

## Consequences

- Support asks for the log and crash files from the app data directory; nothing
  is uploaded automatically.
- Log retention is bounded, so disks do not fill over months of use.
- `RUST_LOG` can raise verbosity for a debugging session without changing
  settings.
- Any future telemetry or remote crash reporting needs a new ADR and an explicit
  opt-in, not an incremental change to this one.
