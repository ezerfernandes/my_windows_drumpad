# ADR 0005: Sample storage

- Date: 2026-11-07
- Status: Accepted

## Context

Drum samples are short. The audio callback must never read from disk or block on
I/O, and importing the same file twice should not duplicate storage. Import must
stay off the UI thread and support WAV, AIFF, FLAC, MP3, and OGG.

## Decision

Decode and resample samples off-thread, then preload them into memory as
immutable buffers published to the engine. Keep a content-addressed cache keyed
by SHA-256 plus target sample rate. Kit manifests reference relative sample paths
and hashes; the cache deduplicates repeated imports and survives restarts.
Generate waveform peaks separately and cache them.

## Alternatives considered

- **Stream from disk**: lower memory, but adds file I/O and unpredictable latency
  to playback, which the real-time contract forbids.
- **OS file cache only**: not deterministic and not portable across machines.
- **SQLite blobs**: centralized storage but duplicates the kit manifest and
  makes user recovery harder.
- **Store decoded PCM in kits**: huge files, no dedupe, and platform-specific
  rates.

## Consequences

- Memory grows with unique sample content. Import validates duration, sample
  rate, channels, and file size so a pathological file cannot exhaust the
  process.
- The import worker owns decode, resample, hash, and peak generation; the UI only
  submits jobs and consumes progress events.
- The cache is content-addressed, so duplicate imports reuse one entry and cleanup
  must never remove a sample a kit still references.
- A missing or changed sample is detected by hash and handled by the relink flow,
  not by a crash.
