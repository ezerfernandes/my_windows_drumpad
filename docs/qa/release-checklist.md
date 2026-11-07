# Release checklist — v1

Owner: QA. A release candidate ships only when every **blocking** row is `pass`. Rows marked `waived` need the release owner's name and a reason. Fill one copy per release candidate and archive it next to the artifact.

## 1. Candidate identity

| Field | Value |
| --- | --- |
| Version / tag | |
| Commit | |
| Portable ZIP SHA-256 | |
| Installer SHA-256 | |
| Build date | |
| Built by / CI run | |
| Release owner | |

## 2. Build and packaging

| # | Check | Blocking | Status | Evidence |
| --- | --- | --- | --- | --- |
| B-1 | `cargo build --release --workspace` succeeds on Windows 11 x64 | yes | | CI log |
| B-2 | `cargo fmt --all -- --check` clean | yes | | |
| B-3 | `cargo clippy --workspace --all-targets -- -D warnings` clean | yes | | |
| B-4 | `cargo test --workspace` passes | yes | | |
| B-5 | Portable ZIP contains `drumpad.exe`, README, third-party notices | yes | | ZIP listing |
| B-6 | Version metadata and icon present in Windows file properties | yes | | screenshot |
| B-7 | Installer installs and uninstalls cleanly as a per-user install | yes | | docs/qa/release-checklist.md §7 |
| B-8 | No debug symbols or test binaries in the package | yes | | ZIP listing |
| B-9 | Artifact hashes published with the release notes | yes | | release notes |

## 3. Test gates

| # | Gate | Blocking | Status | Evidence |
| --- | --- | --- | --- | --- |
| T-1 | E11-T02 core suite passes; coverage ≥ 80 % | yes | | `cargo llvm-cov` summary |
| T-2 | E11-T03 golden audio suite passes | yes | | CI log |
| T-3 | E11-T04 fake MIDI/audio integration suite passes | yes | | CI log |
| T-4 | E11-T05 UI smoke suite passes; snapshots reviewed | yes | | snapshot diff artifact |
| T-5 | E11-T07 budgets met: cold start < 2 s, kit load < 1 s, UI p99 ≤ 16.6 ms | yes | | budget artifact |
| T-6 | Latency gates from `docs/qa/latency-protocol.md` met | yes | | baseline table |
| T-7 | 8-hour soak pass, no leak/crash/xrun growth | yes | | `docs/qa/soak-results/<date>-<build>.md` |
| T-8 | Compatibility matrix filled; no unexplained `fail` | yes | | `docs/qa/compatibility-matrix.md` |

## 4. Functional smoke (manual, release build)

Run on the reference chain: Windows 11 x64, Finding 35-key keypad, Nitro Max over USB-MIDI with Local Mode OFF, onboard 3.5 mm → Aux In.

| # | Step | Blocking | Status |
| --- | --- | --- | --- |
| F-1 | Clean install reaches a playable state; wizard can be skipped and resumed | yes | |
| F-2 | Silent default kit loads on first run; factory demo kit plays | yes | |
| F-3 | All 16 pads trigger from `1 2 3 4 / Q W E R / A S D F / Z X C V` | yes | |
| F-4 | F1–F5 do panic, metronome, prev kit, next kit, map overlay | yes | |
| F-5 | Nitro Max pads and zones trigger the template; hi-hat zones correct | yes | |
| F-6 | Import WAV, AIFF, FLAC, MP3, OGG; assign; play | yes | |
| F-7 | Drag and drop assigns; unsupported file does not change the pad | yes | |
| F-8 | Create/duplicate/rename/delete/switch kit; unsaved prompt works | yes | |
| F-9 | Export/import kit round-trips; moved folder still plays | yes | |
| F-10 | Missing sample is flagged and relink restores playback | yes | |
| F-11 | Unplug/replug audio device recovers; no stuck voices | yes | |
| F-12 | Unplug/replug Nitro Max recovers; no stuck voices | yes | |
| F-13 | Alt-tab and lock/unlock leave no stuck notes | yes | |
| F-14 | Settings persist across restart; invalid device falls back | yes | |
| F-15 | Performance mode is usable at 1080p; exit restores view | no | |

## 5. Docs and attribution

| # | Check | Blocking | Status |
| --- | --- | --- | --- |
| D-1 | User manual covers setup, keyboard/keypad, Nitro Max, kits, samples, performance mode, troubleshooting | yes | |
| D-2 | Nitro Max guide covers USB-MIDI, Local Mode OFF, Aux In, hi-hat calibration | yes | |
| D-3 | Latency protocol and baseline are current | yes | |
| D-4 | Known limitations list is current | yes | |
| D-5 | Third-party notices complete; sample licenses recorded | yes | |
| D-6 | README links the manual and the QA docs | yes | |

## 6. Release notes

Must include:

- Version, date, supported platform (Windows 11 x64 only).
- Artifact hashes and download links.
- What changed since the last candidate.
- **Known issues** copied from the compatibility matrix and soak results, with workarounds.
- SmartScreen expectation and how to verify the hash.
- Rollback instruction: keep the previous portable ZIP available and republish it as the current release.

## 7. Installer verification steps

1. Install on a clean Windows 11 24H2 VM as a standard user. Confirm the install path, Start menu entry, and no admin prompt.
2. Launch, complete the wizard, load the demo kit, play one pad.
3. Uninstall from Windows Settings. Confirm the install directory is gone and no stray service or startup entry remains.
4. Confirm user data policy: kits/samples stay under the documented data directory unless portable mode was used; uninstall must state whether it removes them.
5. Reinstall over an existing install; confirm settings survive or the upgrade path is documented.

## 8. Sign-off and rollback

| Field | Value |
| --- | --- |
| Decision | ship / block |
| Blocking failures | |
| Waived rows (owner + reason) | |
| Release owner signature / date | |
| Rollback plan | previous artifact restored; release notes updated; issue filed |

A single blocking failure blocks the candidate. Do not ship with an unexplained latency, soak, or compatibility failure; waive only with measured evidence and a named owner.
