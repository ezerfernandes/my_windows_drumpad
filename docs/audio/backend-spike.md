# Audio backend spike and latency measurements (E02-T01)

- Date: 2026-11-07
- Platform: Windows 11 x64
- Engine target: 48 kHz, 2 channels, 128-frame target
- Prototype: `crates/drumpad-audio/src/bin/latency-spike.rs`
- Raw results: `docs/audio/measurements/`

## What was measured

The spike opens each render endpoint directly through WASAPI and measures:

- **Callback interval** — wall-clock time between render events. This is the
  period at which the engine can start a new block.
- **Buffer size** — frames the device queues.
- **`IAudioClient::GetStreamLatency`** — the driver's reported stream latency.
- **Callback CPU load** — time spent inside the render write divided by wall
  time. The spike writes a preallocated block of silence, so this measures the
  WASAPI path, not the Drumpad mixer.
- **Late callbacks / timeouts** — an interval above 1.5 x the expected period
  is counted as a late callback and an xrun.

Each configuration ran for 8 seconds on the default endpoint and 6 seconds on
the secondary endpoint; the soak ran 120 seconds. The machine was actively
compiling during the runs, which is intentionally a noisy workload.

Two priority modes were measured:

- **normal** — the prototype thread ran at default priority. This shows how
  sensitive a hand-rolled WASAPI loop is to host scheduling.
- **MMCSS “Pro Audio”** — the production path (`drumpad_audio::priority`) calls
  `AvSetMmThreadCharacteristics("Pro Audio")` and `AvSetMmThreadPriority(CRITICAL)`
  before rendering. The primary table below uses this mode.

Reference machine: AMD desktop with onboard **Realtek High Definition Audio**
(“Speakers (Realtek(R) Audio)”) as the default endpoint, plus an ASUS
“AI Noise-cancelling Output” software endpoint. The Nitro Max USB port carries
MIDI only; it is not an audio endpoint.

## Results — Realtek “Speakers”, 48 kHz stereo, MMCSS “Pro Audio”

| Mode | Requested frames | Actual buffer | Callback period | GetStreamLatency | p50 interval | p99 interval | Max | CPU load | Late |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Shared, default | 128 | 1126 | 512 (10.667 ms) | 0 ms (not reported) | 10.601 ms | 10.900 ms | 11.086 ms | 0.09 % | 0 |
| Shared low-latency (`IAudioClient3`) | 64 | 1126 | 512 (clamped) | 0 ms | 10.600 ms | 10.881 ms | 10.932 ms | 0.09 % | 0 |
| Shared low-latency | 128 | 1126 | 512 (clamped) | 0 ms | 10.603 ms | 10.907 ms | 11.119 ms | 0.09 % | 0 |
| Shared low-latency | 256 | 1126 | 512 (clamped) | 0 ms | 10.599 ms | 10.903 ms | 11.021 ms | 0.09 % | 0 |
| Exclusive, 16-bit | 64 | 256 | 256 (5.333 ms) | 5.333 ms | 5.295 ms | 5.431 ms | 5.451 ms | 1.15 % | 0 |
| Exclusive, 16-bit | 128 | 256 | 256 (clamped) | 5.333 ms | 5.297 ms | 5.432 ms | 5.468 ms | 1.13 % | 0 |
| Exclusive, 16-bit | 256 | 256 | 256 | 5.333 ms | 5.295 ms | 5.439 ms | 5.662 ms | 1.22 % | 0 |
| Exclusive, 24-bit | 128 | 256 | 256 (clamped) | 5.333 ms | 5.298 ms | 5.437 ms | 5.522 ms | 1.40 % | 0 |

### Effect of thread priority

The same exclusive configurations without MMCSS produced 11 late callbacks at
128 frames, 87 at 256 frames, and a 131 ms maximum interval while the machine
was compiling. With MMCSS, all four exclusive configurations produced zero late
callbacks and stayed under 5.7 ms worst case. Registering the render thread is
therefore not an optimization; it is required for exclusive-mode stability on a
busy desktop.

Key facts:

- The Realtek shared-mode engine period is **fixed at 512 frames (10.667 ms)**.
  `GetSharedModeEnginePeriod` reports `min = default = max = 512`, so no
  low-latency shared period exists on this endpoint and the IAudioClient3 path
  cannot go below the engine period. Shared-mode buffer depth is 1126 frames
  (**23.5 ms**).
- Exclusive mode accepts only integer formats on this driver (S16 and S24);
  float exclusive is rejected. The smallest period the driver accepts is
  **256 frames (5.333 ms)**, even when 64 or 128 frames are requested.
- The 8-second pre-MMCSS run showed that S24 is usable but no faster than S16;
  both cap at the driver's 256-frame minimum.
- The two-minute MMCSS soak on exclusive 256 frames and shared 128 frames ran
  with **zero late callbacks**: shared 11,252 callbacks (p50 10.603 ms, p99
  10.906 ms, max 11.128 ms, 0.09 % CPU); exclusive 22,503 callbacks (p50 5.296
  ms, p99 5.434 ms, max 5.803 ms, 1.18 % CPU). Raw JSON:
  `docs/audio/measurements/wasapi-spike-realtek-mussc-120s-soak.json`.

## Results — ASUS “AI Noise-cancelling Output”, 48 kHz stereo

| Mode | Requested | Actual buffer | Callback period | p50 interval | p99 | CPU | Late |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Shared / low-latency | 64-256 | 1056 | 480 (10.0 ms) | 9.95-10.03 ms | 11.3-11.5 ms | 0.10-0.14 % | 0 |
| Exclusive S16 | 64 | 160 | 160 (3.333 ms) | 15.35 ms | 16.5 ms | 0.26 % | 390/392 |
| Exclusive S16 | 256 | 256 | 256 (5.333 ms) | 15.41 ms | 16.5 ms | 0.29 % | 387/389 |

The ASUS endpoint is an APO/virtual output. It advertises a small exclusive
buffer but delivers events at roughly 15 ms, so almost every callback is late
relative to the advertised period. **Do not select it for low-latency
performance.** It is a fine fallback for non-realtime playback.

## Recommendation

1. **Shared mode alone does not meet the latency target on the reference
   machine.** Its 512-frame engine period and 1126-frame buffer put the software
   path at ~23.5 ms, more than twice the 10 ms end-to-end budget before the
   onboard DAC, cable, and module Aux In are added.
2. **Exclusive mode is a v1 requirement for this class of hardware.** It runs at
   256 frames (5.33 ms) on the Realtek endpoint, giving a ~5.3 ms software path.
   With a typical 0.5-1.5 ms DAC/analog contribution the total stays under the
   10 ms target. This follows the design document's rule: “If shared-mode WASAPI
   misses the target, exclusive mode becomes a v1 requirement.”
3. **Keep shared mode as the safe fallback.** Exclusive mode is device-specific
   and blocks other applications. The backend requests exclusive first, falls
   back to low-latency shared, then to shared default, and reports the actual
   mode and buffer to the UI.
4. **Request 128 frames but report the truth.** None of the tested endpoints
   accepted 128 frames. The backend must surface the negotiated period instead
   of pretending the request succeeded.
5. **Register the render thread with MMCSS “Pro Audio” (implemented).** Without
   it, exclusive mode produced 87 late callbacks and a 131 ms outlier in an
   8-second compile-loaded run. With it, every exclusive configuration stayed
   under 5.7 ms worst case with zero late callbacks. The backend does this in
   `drumpad_audio::priority`.
6. **Prefer S16 in exclusive mode on this driver.** S24 works and is equally
   fast with MMCSS, but it uses more CPU and the extra bit depth is inaudible
   for drum samples that peak near 0 dBFS. Keep 24-bit selectable.

## Accepted target vs measured

| Metric | Target | Measured on Realtek exclusive 256 | Status |
| --- | --- | --- | --- |
| Callback period | 2.67 ms (128 frames) | 5.33 ms (256 frames) | Device minimum; miss on this endpoint |
| Software output latency | < 10 ms including block | ~5.3 ms | Pass |
| End-to-end with analog/module path | < 10 ms | Not yet measured (requires loopback cable) | Open |
| CPU load, silent render | “low” | 1.1 % of one core | Pass |
| Xruns, 2 min idle render | 0 | 0 on both modes with MMCSS (11,252 + 22,503 callbacks) | Pass with MMCSS |

The analog round-trip measurement (PC output → 3.5 mm cable → Nitro Max Aux In
→ module phones) is hardware-dependent and is specified by the QA latency
protocol in E11. The spike archives the software-side numbers it can prove;
it does not claim an unmeasured analog figure.

## Reproducing

```text
cargo run -p drumpad-audio --bin latency-spike -- --seconds 8 \
  --out docs/audio/measurements/wasapi-spike-<machine>.json

# One endpoint only
$env:WASAPI_SPIKE_DEVICE = "Realtek"
# One configuration only
$env:WASAPI_SPIKE_CONFIGS = "exclusive:256"
```

The JSON files under `docs/audio/measurements/` are the raw runs used above:

| File | What it is |
| --- | --- |
| `wasapi-spike-realtek-48k-8s.json` | Early normal-priority matrix on the Realtek endpoint |
| `wasapi-spike-asus-noise-cancelling-48k-6s.json` | ASUS APO endpoint, shared and exclusive |
| `wasapi-spike-realtek-60s-soak.json` | Early 60 s soak without MMCSS |
| `wasapi-spike-realtek-production-48k-8s.json` | Production binary, normal priority (shows scheduling outliers) |
| `wasapi-spike-realtek-mussc-48k-8s.json` | Production binary with MMCSS “Pro Audio”, 8 s/config |
| `wasapi-spike-realtek-mussc-120s-soak.json` | MMCSS 120 s soak at shared 128 and exclusive 256 |

The production runs include the endpoint id and friendly name in the JSON so
the raw evidence is self-describing.
