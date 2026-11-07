# Kit format v1

Drumpad kits are inspectable, versioned JSON documents. The authoritative
schema is [`crates/drumpad-model/schema/kit-v1.schema.json`](../crates/drumpad-model/schema/kit-v1.schema.json)
(embedded in the crate as `drumpad_model::kit::KIT_V1_SCHEMA`).

## Layout

A kit is a folder:

```text
<slug>-<kit-id-prefix>/
  kit.json
  samples/...
  .backups/
    kit.bak1.json      # newest previous manifest
    kit.bak2.json
```

Structurally corrupt manifests are moved out of the library:

```text
quarantine/<kit>-<timestamp>/
  quarantine-reason.txt
```

All sample paths in `kit.json` are relative to the kit folder and use forward
slashes. Absolute paths, drive prefixes, `..`, leading `/`, NUL bytes, and
Windows-reserved device names are rejected before a kit is used.

## Document

```json
{
  "format_version": 1,
  "id": "d3a0f0b1-5b1e-4b7a-9b5d-0d2c4e6f8a10",
  "metadata": {
    "name": "Drumpad Demo",
    "author": "Drumpad project",
    "description": "Factory demo kit.",
    "tags": ["demo"],
    "created_unix_ms": 1762531200000,
    "modified_unix_ms": 1762531200000
  },
  "pads": [
    {
      "id": 0,
      "name": "Kick",
      "layers": [
        {
          "id": 1,
          "sample": {
            "path": "samples/kick.wav",
            "hash": "9b817365a7a5...64 hex characters...",
            "original_name": "kick.wav"
          },
          "velocity": { "min": 1, "max": 127 },
          "gain_db": 0.0,
          "tune_cents": 0.0
        }
      ],
      "playback": "one_shot",
      "gain_db": 0.0,
      "pan": 0.0,
      "tune_semitones": 0.0,
      "choke_group": 1,
      "midi": { "note": 36, "channel": 1 },
      "key": { "key": 2 }
    }
  ]
}
```

- `format_version`: currently `1`. A higher version is refused with a typed
  error; older versions run through the migration chain.
- `id`: stable UUID. Renaming or moving a kit does not change it.
- `pads`: exactly 16 entries. Pad ids are unique and in `0..=15`.
- `layers`: at most 32 per pad. Velocity ranges are inclusive and in
  `1..=127`. Overlaps are allowed (round-robin and velocity layering); a pad
  with no layers is valid but silent.
- `playback`: `one_shot`, `gate`, or `loop`. Loop points are frame-accurate
  `start_frame` / `end_frame` pairs.
- `choke_group`: optional `0..=15`; pads that share a group choke each other.
- `midi`: optional `note` (`0..=127`) and one-based `channel` (`1..=16`).
  Note plus channel must be unique inside a kit.
- `key`: optional platform-neutral physical key code. Windows scan codes live
  in the low 16 bits; `0x100` marks extended keys. The default map is
  `1 2 3 4 / Q W E R / A S D F / Z X C V` (scan codes `0x02..0x05`,
  `0x10..0x13`, `0x1E..0x21`, `0x2C..0x2F`). Physical codes are used so the
  keyboard layout does not change the instrument.

## Hashes

`sample.hash` is a SHA-256 over canonical decoded PCM, not over the file
bytes. The digest is defined as:

```text
SHA256("drumpad-pcm-v1" || sample_rate_le_u32 || channels_le_u16 ||
       frames_le_u64 || interleaved_f32_le)
```

This stays stable across WAV, AIFF, FLAC, MP3, and OGG encodings of the same
audio. `ContentHash::of_bytes` is available for raw-file comparisons when
bundle import resolves name collisions.

## Unknown fields and compatibility

Readers ignore unknown fields inside a known version. This is deliberate:
optional fields can be added without breaking older builds. Structural or
semantic changes must increment `format_version` and add a migration step.
Because the version gate runs before interpretation, a future document is
never silently downgraded.

## Migrations

- v0 (pre-release draft) uses a top-level `version` field and a flat
  `pads[].sample_path`. It is upgraded to v1 by creating 16 default pads,
  converting sample paths into velocity layers, and carrying key/MIDI/gain/pan
  values forward.
- Before migrating, the loader copies the original manifest to
  `.backups/pre-migration-v<version>.json`.
- Each migration reports what changed so the app can log it.

## Writes and recovery

- Saves serialize to a temporary file in the kit folder, `fsync`, then rename
  over `kit.json`.
- The previous manifest is copied to `.backups/kit.bak1.json`; older backups
  rotate up to `kit.bak5.json`.
- Autosave debounces edits for one second, well inside the two-second budget.
- A manifest that is structurally corrupt is moved to
  `quarantine/<kit>-<timestamp>/` with a reason file instead of crashing the
  library. A document from a newer `format_version` is refused in place and is
  not quarantined, so a newer build can still open it.
