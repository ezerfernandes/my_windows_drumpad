# ADR 0007: Release profile and packaging

- Date: 2026-11-07
- Status: Accepted

## Context

The first release is portable-first: a user should be able to unzip a folder,
run the app, and move it between machines. The executable must look like a
Windows GUI application (no console window, icon, version metadata in file
properties), behave well under LTO, and still produce useful crash backtraces.

## Decision

- Release profile: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`,
  `strip = "debuginfo"`.
- Windows subsystem: GUI in release builds, console in debug builds so
  developers see startup errors.
- Embed icon and version resources with `winresource` from the app crate's build
  script. Version metadata follows the workspace package version.
- Package a portable ZIP with `scripts/package-portable.ps1`: `Drumpad.exe`,
  `README.md`, `THIRD_PARTY_NOTICES.md`, and a `drumpad.portable` marker so the
  unzipped folder runs in portable mode. CI uploads the ZIP as an artifact.

## Alternatives considered

- **`strip = "symbols"`**: smallest binary, but erases the function names that
  make crash backtraces actionable. `strip = "debuginfo"` keeps the symbol table
  and drops DWARF only.
- **`panic = "abort"`**: smaller and slightly faster, but the crash report and
  future per-view recovery are easier to reason about with unwinding. The
  default unwind behavior is kept.
- **Installer only**: an MSI/EXE installer is a release task (E11-T09), but the
  first release should not require one.
- **UPX or other packers**: break signing and backtraces; not worth the size.

## Consequences

- Release builds are slower because of fat LTO and one codegen unit; CI caches
  artifacts and still finishes inside the job timeout.
- Crash reports keep readable function names while shipping without DWARF.
- The portable marker makes the ZIP self-contained: data lives in
  `DrumpadData/` beside `Drumpad.exe` and moving the folder moves the app.
- Third-party notices are generated from the resolved dependency graph so the
  ZIP ships with accurate license attribution.
