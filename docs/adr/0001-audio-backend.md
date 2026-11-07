# ADR 0001: Audio backend

- Date: 2026-11-07
- Status: Accepted

## Context

Drumpad must play 16 pads with at least 64 voices at 48 kHz with a 128-frame
buffer, targeting under 10 ms end to end on Windows 11 x64 reference hardware.
The backend must enumerate devices, open a stream, report errors, and leave room
for a fake backend that tests can drive without hardware. The design fixes
Windows 11 x64 as the only v1 platform.

## Decision

Use `cpal` with the WASAPI host in shared mode as the v1 backend. Keep the audio
crate's device and stream abstraction independent of cpal so a fake backend and,
later, an exclusive-mode path can sit behind the same interface. Measure shared
mode at 64, 128, and 256 frames during the audio spike (E02-T01). Add WASAPI
exclusive mode in v1 only if shared mode misses the latency target or shows
unacceptable xrun behavior. Keep ASIO as post-v1 work (E12-T02).

## Alternatives considered

- **`wasapi` crate only**: direct exclusive-mode control and low latency, but
  more Windows-specific code and no ready-made device abstraction. Rejected as
  the first path because shared mode is the default user expectation and the
  spike may show it is sufficient.
- **cubeb**: cross-platform and battle-tested, but a larger dependency and less
  direct control over WASAPI-specific stream settings and error reporting.
- **ASIO**: lowest latency on pro interfaces, but licensing, driver
  requirements, and build complexity are not justified for a first release.
- **Raw WinMM**: simple but high latency and no modern stream control.

## Consequences

- Shared mode mixes with other applications and follows the Windows default
  device, which is the safest first-run behavior.
- The sample rate may be dictated by the shared-mode mix format; the sample
  pipeline must resample offline to the device rate.
- The audio spike decides whether exclusive mode is a v1 requirement. That
  decision is recorded before the vertical slice is declared complete.
- Exclusive mode can be added behind the existing `AudioBackend` trait without
  changing the engine or UI.
