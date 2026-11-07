# Drumpad user manual (v1)

Drumpad is a low-latency drum sampler for Windows 11 x64. It turns a computer keyboard, a single-hand keypad, or an Alesis Nitro Max electronic drum kit into a 16-pad performance instrument. It is not a DAW: there is no timeline, no multitrack recording, and no mixer console.

> Status: written against the v1 design (`docs/superpowers/specs/2026-11-07-windows-drum-sampler-design.md`). Items marked **[verify]** must be checked against the shipping build before release.

## 1. Install

Two packages are provided:

- **Portable ZIP** — extract anywhere and run `drumpad.exe`. All data stays beside the executable when a `portable` marker file is present.
- **Installer** — per-user install with an uninstall entry in Windows Settings. No admin rights are required.

Windows 11 x64 is the only supported platform. Windows 10 and other architectures are not supported in v1.

### SmartScreen

The installer is not signed with an EV certificate in the first release. Windows may show "Windows protected your PC". Choose **More info → Run anyway**, or verify the SHA-256 published with the release notes before running.

## 2. First run

On first launch a short setup wizard runs. It asks for:

1. **Audio output device** — defaults to the Windows default output.
2. **MIDI input** — the Nitro Max if one is connected; can be skipped.
3. **Keyboard profile** — full keyboard or the Finding 35-key single-hand keypad.
4. **Sample folder** — where imported samples are stored.

Every step can be skipped and completed later from **Settings**. The wizard never blocks the app when hardware is missing; Drumpad opens with a silent default kit and is playable from the keyboard.

## 3. Audio setup

Open **Settings → Audio**.

| Setting | Default | Notes |
| --- | --- | --- |
| Device | Windows default output | Pick the onboard 3.5 mm jack, not a monitor or HDMI output |
| Sample rate | 48 000 Hz | 44 100 and 96 000 are also supported |
| Buffer | 128 frames | 64 for lower latency, 256 for a slower machine |
| Mode | WASAPI shared | Exclusive mode only appears if it is implemented and measured **[verify]** |

Changing device, sample rate, buffer, or mode restarts the audio stream. Playback stops for a moment; kit state is preserved. If the saved device disappears, Drumpad falls back to the Windows default and shows which device is active.

### Latency tuning

Start with 48 kHz and 128 frames. The target is under 10 ms end to end on the reference chain.

- Lower the buffer to 64 frames if the diagnostics show no xruns. Lower buffers make the keyboard feel tighter.
- Raise the buffer to 256 frames if you hear crackles, see xrun counts climb, or the machine is busy.
- WASAPI shared mode goes through the Windows mixer. Exclusive mode bypasses it, but only use it if the measured shared-mode latency misses the target.
- If the onboard 3.5 mm output is noisy, use a USB audio interface. This is a supported fallback; it does not change the app.

The diagnostics panel shows the active device, sample rate, buffer, estimated latency, callback load, voice count, and xrun count **[verify]**.

## 4. Playing with the keyboard

The 16 pads form a 4x4 grid mapped to this key block:

```text
1 2 3 4
Q W E R
A S D F
Z X C V
```

Every pad shows its key label and MIDI note. **F5** opens the keyboard map overlay, which highlights keys as you press them.

### Finding 35-key single-hand keypad

Connect the keypad over USB. Windows sees it as a normal keyboard; no driver is needed. Select the **Finding 35-key** profile in Settings, or the default map already carries over because the keypad's QWERTY block matches the grid above.

Transport row:

| Key | Action |
| --- | --- |
| F1 | Panic — stop all voices |
| F2 | Metronome toggle |
| F3 | Previous kit |
| F4 | Next kit |
| F5 | Keyboard map overlay |

`5`, `T`, `G`, `B`, and `Space` are deliberately unmapped so a one-handed player does not trigger a pad by accident.

Key events use physical key positions, so the mapping does not change with the Windows keyboard layout.

### Rebinding keys

Open **Edit → Mapping**. Select a pad, choose **Learn**, and press the key you want. Drumpad rejects or explicitly confirms a key that is already assigned. **Reset** restores the default map. Profiles are saved per device and can travel with a kit **[verify]**.

## 5. Playing with the Alesis Nitro Max

### Wiring

```text
Nitro Max pads
      |
      v
USB-MIDI  ->  PC running Drumpad  ->  PC onboard 3.5 mm output
                                          |
                                          v
                              Nitro Max Aux In (3.5 mm stereo)
                                          |
                                          v
                              Nitro Max phones / audio outputs
```

1. Connect the Nitro Max to the PC with a USB cable. Use a short cable and a direct port; a hub can cause detection problems. The module is class-compliant, so no vendor driver is required.
2. Connect the PC's onboard 3.5 mm output to the Nitro Max **Aux In** with a 3.5 mm stereo cable.
3. Set the Nitro Max **Local Mode to OFF**. The module then sends MIDI to the PC and does not play its own sounds, so Drumpad is the only drum source and there is no flam.
4. In Drumpad, select the Nitro Max under **Settings → MIDI**.
5. Load the **Alesis Nitro Max** template kit.

The Nitro Max's USB port carries MIDI only. Audio always returns through the Aux In.

### Gain staging

The Aux In has no separate gain control. Set the PC volume to about 70–80 %, then raise the Nitro Max master volume until the kit is as loud as you need. If the module input clips, lower the PC output first. The app's master limiter keeps its own output below 0 dBFS; it cannot fix clipping at the module input.

### Note map

| Pad | Note | Pad | Note |
| --- | --- | --- | --- |
| Kick | 36 | Hi-hat open | 46 |
| Snare | 38 | Hi-hat half-open | 23 |
| Snare rim | 40 | Hi-hat closed | 42 |
| Tom 1 | 48 | Hi-hat pedal | 44 |
| Tom 1 rim | 50 | Hi-hat splash | 21 |
| Tom 2 | 45 | Ride | 51 |
| Tom 2 rim | 47 | Crash 1 | 49 |
| Tom 3 | 43 | Crash 2 (optional) | 57 |
| Tom 3 rim | 58 | Tom 4 (optional) | 41 |
| | | Tom 4 rim (optional) | 39 |

### Hi-hat calibration

The Nitro Max sends discrete hi-hat notes rather than a continuous position. Drumpad maps open, half-open, closed, pedal, and splash to separate layers.

1. Open **Edit → Hi-hat** **[verify]**.
2. Hit the pedal and each hi-hat zone in turn; the app shows which note and layer it received.
3. If open and closed feel swapped, check that the module is in Local Mode OFF and the template is loaded.
4. If the pedal chick does not trigger, verify note 44 reaches the MIDI monitor.

Modules that send continuous CC4 or CC64 position are also supported; the app uses the continuous value when it is present.

### Local Mode tradeoff

With Local Mode ON, the module's own sounds mix with the app's Aux In audio. This is useful for layering percussion onto the module's kits, but the app path can arrive a few milliseconds later and cause a flam. Use Local Mode OFF for a single, tight sound source.

## 6. Kits

Open the **Kit browser** from the top bar.

- Create, duplicate, rename, delete, and load kits.
- Loading happens off the UI thread; the window stays responsive and shows progress.
- If the current kit has unsaved changes, Drumpad asks before switching.
- Deleting the active kit prompts and selects a safe fallback.
- A silent default kit loads on first run. A small factory demo kit is included **[verify]**.

Kits are versioned JSON manifests with relative sample paths and SHA-256 hashes. Saving is atomic: a crash during save does not corrupt the last good kit. Rotating backups and debounced autosave protect recent edits.

### Portable kits

**Export kit** writes a folder or archive with the manifest, samples, and hashes. **Import kit** validates the bundle, resolves duplicates by content, and reports missing files. Moving the exported folder to another machine does not break it.

## 7. Samples

Drumpad decodes WAV, AIFF, FLAC, MP3, and OGG. Import runs on a worker with progress and cancellation.

- **Drag and drop** a file or folder from Explorer onto a pad. A preview confirms the assignment; unsupported files are summarized and do not change the pad.
- **File picker** from the pad inspector assigns to the selected or first empty pad.
- Imported samples are hashed with SHA-256, stored once in a content-addressed cache, and deduplicated.
- If a sample file goes missing, the kit opens with the affected pads flagged and a **Relink** action. Relink by folder or by content hash; playback is restored without editing the kit by hand.

### Editing

The waveform editor is non-destructive; the original file is never modified. Available operations: trim, fade in, fade out, normalize, gain, reverse, loop points, pitch/tune, and undo **[verify]**.

## 8. Pads and layers

The inspector edits one pad at a time:

- Sample slot and velocity layers with their ranges
- Playback mode: one-shot, gate, or loop
- Volume, pan, tune, choke group
- Keyboard key and MIDI note

Changes apply live. Invalid values (overlapping layer ranges, out-of-range tune, unknown choke reference) are blocked with the reason shown. **Reset** returns the pad to its default state.

- **Gate** mode releases when the key or note is released, with a short fade.
- **Loop** mode repeats between sample-accurate loop points, optionally crossfaded. v1 does not time-stretch loops; loop starts can be quantized to the global tempo.
- **Velocity layers** select by incoming velocity. **Round-robin** groups cycle or randomize layers to avoid machine-gun repetition.

## 9. Performance mode

**F5** in the normal view, or the Performance view in the top bar, enters a full-screen stage layout: large pads, hidden editing chrome, key labels, and the F1–F5 transport row. No accidental edits are possible in this mode. Leaving Performance restores the previous view and selection.

F1 panic stops all voices with a short fade and never leaves a stuck note.

## 10. Tempo and metronome

The global tempo ranges from 30 to 300 BPM. Tap tempo is available from the metronome control. **F2** toggles the metronome without leaving Performance mode. Metronome volume and routing are in Settings. Count-in is available for loop starts. v1 does not time-stretch audio to the tempo.

## 11. Troubleshooting

### No sound

1. Check **Settings → Audio** shows the device you expect and the meter moves when you hit a pad.
2. Confirm the Windows volume mixer is not muted for Drumpad.
3. If the Nitro Max is the monitor path, confirm the Aux In cable and the module master volume.
4. Try 256 frames to rule out xruns; check the diagnostics xrun count.

### Crackles or dropouts

Raise the buffer, close CPU-heavy background apps, and avoid USB hubs for the MIDI connection. If the xrun count rises only while the UI animates, report it with the diagnostics snapshot.

### Audio device lost

If the device is unplugged or the format changes, Drumpad stops the stream safely, shows a disconnected state, and reconnects when the device returns. Kit state is not lost. If reconnection does not happen within a few seconds, reselect the device in Settings.

### MIDI device not found

Unplug and replug the Nitro Max, try a different USB port, and use a short cable. Close other MIDI applications that may hold the port. Drumpad rescans periodically; no restart should be required.

### Stuck note

Press **F1** to panic. Stuck notes are a bug; report the input source (keyboard/MIDI), what was held, and whether focus changed. Drumpad stops all voices on focus loss by design.

### Missing samples

Open the kit; affected pads are flagged. Use **Relink** to point at the moved folder or match by hash. If the file is gone, reassign the pad.

### High latency

Check the buffer and sample rate first. Shared mode adds the Windows mixer path. If p99 latency misses the target with a 128-frame buffer at 48 kHz, capture a diagnostics snapshot and attach it to the report.

### Reporting a problem

Attach the log bundle from **Settings → Advanced → Open log folder** and the diagnostics snapshot. Logs are local only; Drumpad does not send them anywhere. Remove sample paths from a log before sharing if they are private.
