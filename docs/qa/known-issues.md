# Known issues and limitations — v1

Living list. E11-T06 requires known limitations to be documented; E11-T10 requires them in the release notes. Add a row when an issue is accepted, remove it only when the fix ships.

Status: `open`, `workaround`, `fixed-in-next`, `waived` (owner + reason).

| # | Area | Issue | Impact | Workaround | Status | Tracked by |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Install | Artifact is not EV-signed in the first release; SmartScreen warns | Install friction | Verify SHA-256, choose More info → Run anyway | waived (release owner) | E11-T09 |
| 2 | Performance | v1 does not time-stretch loops; only quantized starts | Tempo changes do not follow audio | Use loops at the kit tempo | waived (design non-goal) | E10-T06 |
| 3 | Platform | Windows 11 x64 only; Windows 10 and ARM64 are unsupported | No install on other platforms | None | waived (design non-goal) | E01 |
| 4 | MIDI | ASIO and MIDI-out are post-v1 | No ASIO latency path; no external clock | Use WASAPI shared/exclusive | waived (design non-goal) | E12 |

## Template

```markdown
| N | area | one-sentence issue | user impact | workaround or "none" | status | bead |
```

## Rules

- A limitation that affects latency, stability, or sound is not waived without measured evidence.
- The release notes copy this table verbatim for every `open`/`waived` row.
- If a workaround exists, it must also appear in `docs/qa/user-manual.md` §11.
