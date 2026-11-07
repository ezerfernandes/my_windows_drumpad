# Windows developer setup

Drumpad targets Windows 11 x64 only. This guide gets a new contributor from a
clean machine to a running build, a passing test suite, and a playable MIDI path.

## 1. Install the toolchain

### Visual Studio C++ build tools

Install **Visual Studio 2022 Build Tools** and select the **Desktop development
with C++** workload. That provides:

- MSVC v143 (x64/x86) compiler and linker (`cl.exe`, `link.exe`)
- Windows 11 SDK (headers, libraries, and `rc.exe` for the resource compiler)

If `link.exe` is not found later, this workload is almost always the missing
piece.

### Rust

Install `rustup` from <https://rustup.rs/> or with:

```powershell
winget install --id Rustlang.Rustup -e
```

The repository's `rust-toolchain.toml` pins the stable channel and installs
`rustfmt` and `clippy`, so no manual toolchain override is needed.

### Optional tools

- **PowerShell 7** (`pwsh`) for the packaging scripts.
- **Git** with a credential helper for the repository.
- **loopMIDI** and **MIDI-OX** for MIDI testing without hardware.
- **7-Zip** or Explorer for inspecting the portable ZIP.

Verify the basics:

```powershell
rustc --version
cargo --version
where.exe link
```

## 2. Build, run, and test

From the repository root:

```powershell
cargo build --workspace
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Run the app in a debug build (console visible, which is useful for startup
errors):

```powershell
cargo run -p drumpad-app
```

On first launch the app creates its directory tree and writes default settings.
The foundation build currently validates startup and exits; the winit/egui shell
is added by the UI pair.

### Release build

```powershell
cargo build --release -p drumpad-app
```

The release profile uses fat LTO, one codegen unit, and `strip = "debuginfo"`.
The executable is `target\release\drumpad-app.exe` and uses the Windows GUI
subsystem, so no console window appears. The debug build keeps the console for
diagnostics.

### Portable ZIP

```powershell
pwsh ./scripts/package-portable.ps1
```

The script builds the release executable, stages the payload, generates
third-party notices, and writes
`dist\Drumpad-<version>-windows-x64-portable.zip`. The ZIP contains
`Drumpad.exe`, `README.md`, `THIRD_PARTY_NOTICES.md`, and a `drumpad.portable`
marker. Unzip it anywhere and run `Drumpad.exe`; data stays in `DrumpadData/`
beside the executable.

## 3. Continuous integration

`.github/workflows/ci.yml` runs on every pull request and on `main` on
`windows-latest`:

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo test --workspace --all-targets`
4. `cargo build --workspace --release --locked`
5. Upload the release artifact

Clippy warnings fail the build. Fix the warning or add a narrow, justified
`#[allow(...)]` with a comment.

## 4. Where data lives

The layout comes from `directories` and the `drumpad.portable` marker.

### Installed build

| Purpose | Location |
| --- | --- |
| Config | `%APPDATA%\Drumpad\Drumpad\config` |
| Data (kits, samples) | `%LOCALAPPDATA%\Drumpad\Drumpad\data` |
| Cache | `%LOCALAPPDATA%\Drumpad\Drumpad\cache` |
| Logs | `%LOCALAPPDATA%\Drumpad\Drumpad\logs` |
| Crash reports | `%LOCALAPPDATA%\Drumpad\Drumpad\crashes` |

### Portable build

Create an empty file named `drumpad.portable` beside `Drumpad.exe`. Everything
then lives under:

```text
DrumpadData\
  cache\
  config\settings.json
  crashes\
  data\
  logs\drumpad.log.yyyy-mm-dd
```

Delete the marker to return to per-user directories.

### Settings and logs

`settings.json` is versioned JSON with `schema_version`, `logging`, and `ui`
sections. It is written atomically and rejects unknown fields or invalid values.
To reset settings, delete the file; the app writes defaults on the next launch.
If the file is corrupt, the app reports an error instead of overwriting it:
rename it to `settings.json.bak` and restart.

Logs rotate daily and keep 14 days by default. Set `RUST_LOG=debug` in the
environment for a more verbose session without editing settings. Crash reports
are local files; they are never uploaded. They include the app version, OS,
architecture, thread, panic message, location, and backtrace, with the user
profile path redacted.

## 5. MIDI loopback testing

Without hardware:

1. Install loopMIDI and create a virtual port named `Drumpad Test`.
2. Open MIDI-OX and select the same port as output.
3. Send Note On/Off messages and watch the app's MIDI monitor (once the input
   pair lands): note 36 is kick, 38 snare, 42 closed hi-hat, 46 open hi-hat.

With the Alesis Nitro Max:

1. Connect the module over USB. It is class-compliant; no vendor driver is
   required. Prefer a short cable and a direct port if it is not detected.
2. Set **Local Mode to OFF** so the module sends MIDI without playing its
   internal sounds.
3. Connect the PC's onboard 3.5 mm output to the Nitro Max **Aux In**.
4. Set the PC output level and module master volume so the Aux In does not clip.
5. Verify the note map: kick 36, snare 38, snare rim 40, toms 48/45/43, ride 51,
   crash 49, hi-hat open 46, half-open 23, closed 42, pedal 44, splash 21.

If the module still makes its own sound, Local Mode is still on. If the app
audio is late or flams, check the Aux In chain and the buffer size.

## 6. Audio testing

- Start with WASAPI **shared mode** and the Windows default output device.
- Test at 44.1, 48, and 96 kHz with 64, 128, and 256-frame buffers.
- Use the offline renderer and latency harness in `drumpad-tools` for
  deterministic checks without an audio device.
- Add exclusive mode only after the audio spike (E02-T01) shows shared mode
  misses the latency target.

## 7. Troubleshooting

| Symptom | Fix |
| --- | --- |
| `link.exe` not found | Install the VS 2022 Build Tools C++ workload |
| `rc.exe` not found or resource build fails | Install the Windows 11 SDK and reopen the shell |
| `cargo fmt --check` fails in another crate | That pair owns the file; do not format their reserved paths |
| No MIDI device listed | Check cable/port, close other apps holding the port, then rescan |
| No sound | Check the Windows default device, app volume, Local Mode, and Aux In |
| SmartScreen blocks the ZIP | Unsigned local builds trigger it; use "More info" or sign release builds |
| App exits immediately in release | Expected for the foundation build; the UI shell is not wired yet |
| Startup fails | Read the local log and crash folder paths above |

## 8. Project workflow

- Issue tracker: `br` in `.beads/`; triage with `bv --robot-triage`.
- Claim work with `br update <id> --claim --json`, close with
  `br close <id> --reason "..." --json`, then `br sync --flush-only`.
- Crate boundaries and the shared interface vocabulary:
  `docs/architecture/interfaces.md`.
- Architecture decisions: `docs/adr/README.md`.
- Design and work breakdown:
  `docs/superpowers/specs/2026-11-07-windows-drum-sampler-design.md`.
