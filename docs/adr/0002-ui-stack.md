# ADR 0002: UI stack

- Date: 2026-11-07
- Status: Accepted

## Context

The app needs a dark, high-contrast 4x4 pad grid that is readable on stage, a
focused inspector, kit browsing, settings, and a keyboard map overlay. Keyboard
input must reach the trigger queue in under 2 ms, so the UI must not stand
between a key event and the audio path. The app is a native Windows desktop
program, not a browser shell.

## Decision

Build the UI with `winit` and `egui` integrated directly, without `eframe`.
Keyboard events are handled in the winit event loop and pushed to the audio
queue before the next frame is rendered. egui draws the pad grid, inspector,
browser, settings, and overlays.

## Alternatives considered

- **eframe**: the fastest way to an egui window, but it hides the event loop
  behind an app trait and delays input until the next frame. Rejected for v1
  because keyboard latency is a product premise.
- **Slint**: a strong declarative UI toolkit, but a larger runtime and a
  different styling model than the custom pad grid needs.
- **Tauri**: web UI inside a native shell. Rejected: browser rendering adds
  latency and a large dependency, and the app must stay native.
- **Custom immediate-mode renderer**: maximum control but months of work for
  widgets the inspector and settings need.

## Consequences

- The app crate owns event-loop integration and must forward triggers before
  repainting. This is explicitly in scope for E08-T01.
- UI thread work must stay off the audio thread: no locks, allocations, or file
  I/O in the callback.
- egui's immediate-mode model suits animated pads and meters, but accessibility
  support is limited; the keyboard map overlay and visible focus states cover the
  primary stage workflow.
- The UI can be smoke-tested with `egui_kittest` without a GPU once the shell
  exists (E11-T05).
