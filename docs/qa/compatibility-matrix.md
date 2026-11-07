# Compatibility matrix — Windows 11 x64

Every row must be filled from an actual run on the named hardware/OS combination. Where a row is not measured it says **pending** and the release candidate is blocked until it is filled or explicitly waived by the release owner.

Status values: `pass`, `fail`, `pending`, `n/a` (with reason), `waived` (with owner and date).

## 1. OS and platform

| ID | Windows build | Arch | CPU | RAM | Build | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| OS-1 | Windows 11 24H2 (26100) | x64 | | | rc | pending | Primary target |
| OS-2 | Windows 11 23H2 (22631) | x64 | | | rc | pending | Previous release |
| OS-3 | Windows 11 24H2 | x64 | | | portable ZIP | pending | Portable mode, marker file present |

Record `winver` output, `systeminfo` summary, and the exact artifact hash for each row.

## 2. Audio output

| ID | Device | Driver/stack | Rate | Buffer | Mode | xruns (10 min) | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| AU-1 | PC onboard 3.5 mm (Realtek) | WASAPI | 48 000 | 128 | shared | | pending | Reference path into Nitro Max Aux In |
| AU-2 | PC onboard 3.5 mm (Realtek) | WASAPI | 44 100 | 128 | shared | | pending | |
| AU-3 | PC onboard 3.5 mm (Realtek) | WASAPI | 96 000 | 128 | shared | | pending | |
| AU-4 | PC onboard 3.5 mm | WASAPI | 48 000 | 64 | shared | | pending | Lower-bound buffer |
| AU-5 | PC onboard 3.5 mm | WASAPI | 48 000 | 256 | shared | | pending | Fallback buffer |
| AU-6 | USB audio interface | WASAPI | 48 000 | 128 | shared | | pending | Fallback when onboard jack is noisy |
| AU-7 | HDMI/monitor output | WASAPI | 48 000 | 128 | shared | | pending | Must not be selected by default |

Verify for each: correct rate reported in diagnostics, no resampling surprise (Windows mixer), meters track, device loss and return recovers, settings persist across restart.

## 3. MIDI input

| ID | Device | Connection | Local Mode | Status | Notes |
| --- | --- | --- | --- | --- | --- |
| MI-1 | Alesis Nitro Max | USB, direct port | OFF | pending | Reference chain; all pads and zones |
| MI-2 | Alesis Nitro Max | USB through powered hub | OFF | pending | Documented limitation if unreliable |
| MI-3 | Alesis Nitro Max | USB, replug during playback | OFF | pending | No stuck voices, reconnects |
| MI-4 | Generic class-compliant MIDI keyboard | USB | n/a | pending | Note on/off, velocity 1 and 127 |
| MI-5 | No MIDI device attached | n/a | n/a | pending | App starts, keyboard still plays |

Nitro Max zone check (with template kit loaded): kick 36, snare 38, snare rim 40, tom 1 48 / rim 50, tom 2 45 / rim 47, tom 3 43 / rim 58, ride 51, crash 1 49, hi-hat open 46 / half-open 23 / closed 42 / pedal 44 / splash 21. Expansion: tom 4 41 / rim 39, crash 2 57.

## 4. Keyboard and keypad

| ID | Device | Profile | Status | Notes |
| --- | --- | --- | --- | --- |
| KB-1 | Standard US layout keyboard | Full keyboard | pending | `1 2 3 4 / Q W E R / A S D F / Z X C V` |
| KB-2 | Standard non-US layout keyboard | Full keyboard | pending | Physical key codes; layout must not change mapping |
| KB-3 | Finding 35-key single-hand keypad (ASIN B09HF5WDR2) | Finding 35-key | pending | Reference surface |
| KB-4 | Finding 35-key | Finding 35-key | pending | F1–F5 transport row |
| KB-5 | Finding 35-key | Finding 35-key | pending | `5 T G B Space` do nothing |
| KB-6 | Laptop keyboard | Full keyboard | pending | Rollover: see §6 |

## 5. Audio path through Nitro Max

| ID | Check | Status | Notes |
| --- | --- | --- | --- |
| NM-1 | PC onboard 3.5 mm → Nitro Max Aux In, audio audible on phones | pending | No clipping at module input |
| NM-2 | Local Mode OFF, app is the only drum source | pending | No flam against module sounds |
| NM-3 | Local Mode ON, flam observed and documented | pending | Tradeoff documented in user manual |
| NM-4 | Hi-hat open/half/closed/pedal/splash trigger correct layers | pending | |
| NM-5 | Gain staging: PC 70–80 %, module master sets final level | pending | |

## 6. Rollover and fast taps

Use the rollover test: hold eight pads in sequence and strike each pad at least 20 times in 10 s.

| Surface | Simultaneous held pads supported | Dropped fast taps | Status |
| --- | --- | --- | --- |
| Standard keyboard | | | pending |
| Finding 35-key keypad | | | pending |
| Nitro Max over USB-MIDI | | | pending |

A dropped tap is a hit that produces no trigger. Record the key/note, the sequence, and whether the note-off arrived.

## 7. Known limitations

Fill this list as rows fail or are waived. Each entry needs: scope, user impact, workaround, and the bead that tracks a fix.

| Limitation | Impact | Workaround | Tracked by |
| --- | --- | --- | --- |
| (none recorded yet) | | | |

## 8. Sign-off

| Field | Value |
| --- | --- |
| Build / commit | |
| Artifact SHA-256 | |
| Test date | |
| Operator | |
| Rows passed / total | |
| Waived rows | |
| Release blocked? | yes until all non-waived rows pass |
