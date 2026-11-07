# Soak test procedure — 8 hours

Gate for E11-T06. A release candidate is not signed off until one clean 8-hour run exists for the release build, on the reference hardware, with the results archived.

## 1. What the soak must exercise

| Load | Detail |
| --- | --- |
| Voices | Continuous 16-pad rolls, random velocities 1–127, overlapping hits, at least 32 voices active most of the time |
| Choke | Hi-hat pedal/closed pairs and other choke groups firing so chokes are exercised continuously |
| Gate | Held pads released on a schedule so gate fades run |
| Loops | At least one loop pad running for the full duration |
| MIDI hotplug | Nitro Max unplug/replug every 10 minutes |
| UI | Animation at 60 fps: pad hit flashes, meters, view switches |
| Kit switching | Load a different kit every 15 minutes, including a kit with missing samples |
| Devices | Real WASAPI device; also run one shorter (1 h) fake-backend pass in CI nightly |

## 2. Procedure

1. Record the build hash, artifact SHA-256, OS build, CPU, RAM, audio device, driver version, buffer, and sample rate.
2. Start with a warm machine: no other audio apps, no pending Windows updates.
3. Run:

   ```powershell
   cargo run --release -p drumpad-tools --bin soak -- --hours 8 --out docs/qa/soak-results
   ```

   The runner drives the scripted load above and samples every 10 seconds.
4. Do not interact with the machine beyond the scripted hotplug. If you must, note the time and reason in the log.
5. At the end, collect: runner exit code, CSV samples, app log, crash reports (should be none), and a screenshot of diagnostics.
6. Write the summary table into `docs/qa/soak-results/<date>-<build>.md` using §4.

## 3. Metrics and pass criteria

| Metric | Sampling | Pass |
| --- | --- | --- |
| Crashes / panic dialogs | continuous | 0 |
| Process working set (RSS) | 10 s | ≤ 5 % drift after the 15-minute warm-up, no monotonic climb |
| Handle count | 10 s | flat; no per-hotplug growth after the first reconnect |
| Thread count | 10 s | flat |
| Xrun count | 10 s | no increase in the last hour |
| Callback load p99 | 10 s | < 70 % of the block period |
| Active voices | 10 s | within configured cap; 0 after panic and after each hotplug settle |
| Stuck voices | after each hotplug and at end | 0 (diagnostics voice count returns to 0 when input stops) |
| Audio glitches audible | manual spot check hourly | none |

Any single crash fails the run. One unexplained RSS or handle climb fails the run; a known, documented Windows behavior may be waived by the release owner with evidence.

## 4. Results template

```markdown
# Soak result — <date> — <build>

- Artifact SHA-256:
- Commit:
- OS: Windows 11 <build> x64
- CPU / RAM:
- Audio: <device>, <rate> Hz, <buffer> frames, <mode>
- MIDI: <device>, Local Mode <on/off>
- Duration: 8 h
- Runner: `cargo run --release -p drumpad-tools --bin soak -- --hours 8`

| Metric | Start | 1 h | 4 h | 8 h | Verdict |
| --- | --- | --- | --- | --- | --- |
| RSS (MB) | | | | | |
| Handles | | | | | |
| Threads | | | | | |
| Xruns | | | | | |
| Callback p99 (%) | | | | | |
| Peak voices | | | | | |

Crashes: 0
Stuck voices: 0
Known limitations observed:
Artifacts: <path to csv> <path to log>
Operator:
```

## 5. Failure handling

- Keep the raw CSV and log; do not rerun before archiving.
- File a bead for every failing metric with the sampled evidence and the time window.
- A fix requires a full 8-hour rerun; a 1-hour rerun only proves the fix locally.
- If the failure is in a dependency or Windows audio stack, record the version and the workaround, and mark the row waived with the release owner's name.
