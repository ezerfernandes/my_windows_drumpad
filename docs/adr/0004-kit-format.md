# ADR 0004: Kit format

- Date: 2026-11-07
- Status: Accepted

## Context

Kits contain 16 pads with velocity layers, playback settings, mappings, and
sample references. They must survive crashes, upgrades, and portable moves
between machines, and users must be able to inspect and hand-edit them when
something goes wrong.

## Decision

Store kits as versioned JSON manifests. Each manifest declares a format version
and references samples by relative path plus SHA-256 content hash. Writes use a
temporary file, flush, and atomic rename, with rotating backups and debounced
autosave. Migrations upgrade older versions and refuse versions from the future.

## Alternatives considered

- **RON**: nicer to hand-edit for Rust developers, but a smaller ecosystem and
  no serde_json tooling for external validation.
- **SQLite**: strong querying and crash safety, but opaque to users, harder to
  diff, and overkill for 16 pads.
- **Binary format**: compact and fast, but not inspectable and a migration
  liability.
- **TOML**: good for settings, less natural for nested pad and layer arrays.

## Consequences

- The model crate owns the schema and migrations; serde is the serialization
  boundary.
- Relative paths plus hashes make portable kits work after a folder move and
  give the relink flow a way to identify a moved sample.
- Atomic writes and backups mean a crash during save leaves the last good kit
  intact.
- Unknown fields are rejected in a known version so typos surface; a future
  minor version can add optional fields with defaults, gated by its version
  number and a migration.
