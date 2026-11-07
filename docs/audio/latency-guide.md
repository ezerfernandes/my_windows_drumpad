# Latency guide and diagnostics (E02-T07)

This guide explains where latency comes from, which knobs matter, what the app
actually negotiates, and how to read the diagnostics view. It complements the
measured numbers in [backend-spike.md](backend-spike.md).

## The budget

The v1 target is **under 10 ms end to end** from a pad hit to analog output:

| Stage | Target | Notes |
| --- | --- | --- |
| Input (keyboard/MIDI) to trigger queue | < 2 ms | Keyboard/MIDI spike; not this crate |
| Trigger queue to audio block start | 0-1 block | Sample-accurate scheduling inside the block |
| Audio block period | 2.67 ms at 128 frames, 5.33 ms at 256 frames | Device-dependent |
| Device buffer / WASAPI stream latency | one or more periods | Reported by `GetStreamLatency` where available |
| DAC, 3.5 mm output, Nitro Max Aux In | ~1-2 ms | Requires a loopback measurement |

The engine can only control the middle three rows. It cannot make a driver
accept a period the endpoint does not support.

## Shared vs exclusive

| | WASAPI shared | WASAPI exclusive |
| --- | --- | --- |
| Mixes with other apps | Yes | No (endpoint is locked) |
| Period control | Windows audio engine chooses; `IAudioClient3` can request a period inside `min..max` | Driver chooses the minimum; request is aligned up |
| Typical period | 10 ms (480/512 frames) on onboard Realtek | 3-5.3 ms on onboard Realtek |
| Format | Float mix format; autoconvert can accept other rates | Integer formats on most drivers (16/24-bit) |
| Failure mode | Almost always opens | Can fail with unsupported format, device-in-use, or exclusive-not-allowed |

The app requests **exclusive first** (because the spike showed shared mode
misses the target on the reference machine), then **shared low-latency**, then
**shared default**. The negotiated result is shown in diagnostics; a fallback
is never silent.

Exclusive mode is only offered when it is implemented and the device supports
it. ASIO remains a post-v1 option (see E12).

## How to read the diagnostics view

The audio layer exposes a `StreamSnapshot` with:

- **State** — Running, Stopped, DeviceLost, Error. A DeviceLost state should
  show a reconnect action.
- **Buffer frames / callback period frames** — what the device actually
  accepted, not what was requested.
- **Sample rate and sample format** — shared uses f32; exclusive may use i16 or
  i24.
- **Estimated latency** — `max(buffer, callback period, reported stream
  latency)`. This is software-side only.
- **Xruns / late callbacks / timeouts** — a non-zero steady-state xrun count
  means the machine or the callback cannot keep up.
- **Callback load** — averaged and peak fraction of real time spent in the
  callback. Keep the average well under 30 %; a drum sampler should sit under
  10 % on the reference machine with 64 voices.
- **Active voices / voice limit** — from the engine meters.
- **Dropped triggers / commands** — queue-full events. Any growth means the
  audio thread is starved or the producer is flooding it.

`GetStreamLatency` returns 0 on the Realtek shared path, so the app falls back
to the buffer size. Do not display a zero as “zero latency”.

## Device selection

- Prefer the motherboard/onboard endpoint driven by the Realtek-class driver.
  On the reference machine it accepts 256-frame exclusive periods.
- Avoid software/APO endpoints such as “AI Noise-cancelling Output” for
  performance audio. They advertise small exclusive buffers but deliver events
  around 15 ms (measured); see the spike notes.
- A USB audio interface is the fallback when the onboard jack is noisy. Test
  its exclusive period before assuming it is better.
- The Nitro Max USB port sends **MIDI only**. The monitoring path is PC 3.5 mm
  output → Nitro Max Aux In. There is no USB audio to enumerate.

## Nitro Max gain staging

- Set **Local Mode OFF** so the module does not add its own drum sounds.
- Set the Windows output volume high (close to unity) and the module master
  volume to a comfortable level.
- The Aux In has no separate gain control. If the module input clips, lower the
  Windows output or pad gain, not the module master (which also affects the
  module's own monitoring).
- Keep the master limiter enabled. It is zero-latency and transparent below
  -1 dBFS; it prevents a 64-voice hit from clipping the module input.

## Measuring a real round trip

The software numbers do not include the DAC, cable, or module input. To measure
the analog path:

1. Connect the PC 3.5 mm output to a loopback input (or to the Nitro Max Aux In
   and record the module output).
2. Play a single-sample click or a short impulse through one pad.
3. Record the input with the same interface, at a known sample rate.
4. Measure the distance between the first non-zero output sample and the first
   non-zero recorded sample in frames.
5. Repeat 100 times; report p50, p95, p99, and max.

Document the device, driver version, buffer, sample rate, and whether the
Nitro Max was in the path. The QA epic (E11) owns the formal protocol and the
release gate.

## Common problems

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Period is 10.7 ms despite asking for 2.67 ms | Shared mode engine period fixed by the driver | Use exclusive mode, or accept the device minimum |
| Exclusive open fails with `0x88890019` | Buffer size not aligned | Backend retries with the driver-reported aligned period |
| Exclusive open fails with unsupported format | Driver does not accept float or the selected bit depth | Backend tries 16/24-bit, then falls back to shared |
| Xruns while compiling/copying files | Host scheduling, no MMCSS priority | Set the audio thread to “Pro Audio” MMCSS; close CPU-heavy apps |
| Output clips the module | Too much level into Aux In | Lower pad/master gain; keep the limiter on |
| Crackle only on one device | APO/virtual endpoint | Select the physical endpoint |
