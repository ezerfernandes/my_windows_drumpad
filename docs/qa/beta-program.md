# Beta program — v1

Part of E11-T10. The beta exists to collect hardware evidence the build machine cannot produce: real Windows 11 builds, real audio devices, the Finding keypad, and Nitro Max kits, plus an 8-hour soak on hardware QA does not own.

## 1. Goals and exit criteria

| Goal | Measure | Exit |
| --- | --- | --- |
| Hardware coverage | At least 5 external testers with distinct audio devices | ≥ 5 completed reports |
| Latency evidence | p50/p95/p99 from at least 3 machines, shared mode | Filled rows in `docs/qa/latency-baseline.md` |
| Compatibility | OS-2 (23H2) and at least 2 audio devices covered | Matrix rows `pass` or `waived` |
| Stability | No P0 defect open; no crash report without a repro | Release candidate sign-off |
| Usability | A new tester can install, set up, and play from the manual alone | ≥ 4 of 5 report success without help |

## 2. Tester profile

Recruit for difference, not enthusiasm:

1. Nitro Max owner with the onboard 3.5 mm → Aux In chain (the reference path).
2. Finding 35-key single-hand keypad owner.
3. Laptop with Realtek onboard audio and Windows 11 23H2.
4. Desktop with a USB audio interface.
5. Low-spec or high-DPI machine (different failure modes).

Each tester needs: Windows 11 x64, a keyboard, and the ability to run PowerShell commands for log collection.

## 3. Onboarding message

Send with the build:

- Artifact hash and download link.
- The user manual (`docs/qa/user-manual.md`).
- What to test first: install, wizard, play all 16 pads, Nitro Max path if owned.
- How to report: issue template below, one issue per problem.
- Privacy: logs are local; ask before sharing sample paths.

## 4. Issue template

```markdown
### Summary
One sentence.

### Environment
- Drumpad version / artifact SHA-256:
- Windows build (`winver`):
- CPU / RAM:
- Audio device + driver, sample rate, buffer, mode:
- MIDI device, Local Mode:
- Keyboard/keypad model:

### Steps
1.
2.

### Expected

### Actual

### Attachments
- Diagnostics snapshot (Settings → Advanced):
- Log folder zip:
- Crash report (if any):

### Frequency
always / often / once. Times seen:
```

Triage labels: `beta/p0` (crash, no sound, stuck voices), `beta/p1` (latency miss, device loss, wrong mapping), `beta/p2` (UI, docs), `beta/question`.

## 5. Latency report form

Ask every tester with a MIDI kit to run the protocol in `docs/qa/latency-protocol.md` and paste:

| Field | Value |
| --- | --- |
| Machine / CPU | |
| Windows build | |
| Audio device, rate, buffer, mode | |
| MIDI device | |
| p50 / p95 / p99 key→queue (ms) | |
| p50 / p95 / p99 MIDI→callback (ms) | |
| End-to-end estimate (ms) | |
| Xruns during the run | |
| Notes (flam, clicks, dropouts) | |

## 6. Triage and response

- Acknowledge within 2 business days.
- Reproduce on the reference machine before accepting a fix; otherwise ask for the log bundle.
- P0: stop the beta, fix, reissue, tell every tester.
- P1: fix before the release candidate; a waiver needs the release owner's name.
- P2: may ship; add to known issues in the release notes.
- Close only when the reporter confirms or the fix is verified on the reporter's machine data.

## 7. Release candidate sign-off

| Gate | Owner | Status |
| --- | --- | --- |
| ≥ 5 completed beta reports | QA | pending |
| Latency baseline rows from ≥ 3 machines | QA | pending |
| No open P0; every P1 fixed or waived | QA + release owner | pending |
| Compatibility matrix filled | QA | pending |
| Soak pass archived | QA | pending |
| Release notes include known issues | release owner | pending |
| Installer and portable ZIP verified | foundation pair | pending |

Sign-off is recorded in `docs/qa/release-checklist.md` §8. Rollback: restore the previous artifact as current, update the release notes, and open a bead for the regression.
