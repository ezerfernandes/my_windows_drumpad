Implement the keyboard, one-hand keypad, MIDI, and Nitro Max input beads for the Drumpad v1 WBS.

Beads: E06-T01 through E06-T08 (trigger abstraction, default map, latency spike, low-latency keyboard path, velocity, mapping editor, focus loss, Finding 35-key profile and rollover) and E07-T01 through E07-T10 (midir backend, hotplug, note mapping, MIDI learn, velocity curves, hi-hat engine, monitor, Nitro Max template, latency/fallback, Aux In/Local Mode integration).

Start with E06-T01 (source-agnostic trigger abstraction) and E06-T03 (keyboard latency spike), then work down the dependencies.

Reference hardware:
- Finding 35-key single-hand USB keypad, ASIN B09HF5WDR2.
- Pads: 1-4 / Q-R / A-F / Z-V.
- F1 panic, F2 metronome, F3 previous kit, F4 next kit, F5 map overlay.
- 5/T/G/B and Space stay unmapped.
- Alesis Nitro Max over USB-MIDI. Set Local Mode OFF; the app is the sound source. The PC onboard 3.5 mm output feeds the Nitro Max Aux In.
- Nitro Max hi-hat notes: 46 open, 23 half-open, 42 closed, 44 pedal, 21 splash. Also support CC4/CC64 for other modules.

Own and reserve `crates/drumpad-input/**`. The app, model, engine, and audio crates belong to other pairs; agree on public interfaces through your thread and the orchestrator. Do not edit their files.

For each bead: claim it, implement it, run the relevant checks, update the acceptance criteria, and close it only when the criteria are met. Run `br sync --flush-only` after tracker changes. Do not commit or push.
