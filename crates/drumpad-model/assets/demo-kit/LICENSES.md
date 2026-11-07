# Drumpad Demo Kit licenses

All audio files in `samples/` were synthesized from scratch for the Drumpad
project by the project authors. They contain no third-party recordings or
sample libraries.

Dedication: **CC0 1.0 Universal (Public Domain Dedication)**.
See https://creativecommons.org/publicdomain/zero/1.0/

Files and their sound sources:

| File | Source |
| --- | --- |
| `kick.wav` | original sine sweep + noise transient |
| `snare.wav` | original tone + white-noise burst |
| `rim.wav` | original short tone + click |
| `clap.wav` | original multi-burst noise |
| `floor_tom.wav` | original sine sweep |
| `tom_low.wav` | original sine sweep |
| `tom_mid.wav` | original sine sweep |
| `tom_high.wav` | original sine sweep |
| `hat_closed.wav` | original noise + metallic partials |
| `hat_pedal.wav` | original noise + metallic partials |
| `hat_open.wav` | original noise + metallic partials |
| `crash.wav` | original noise + metallic partials |
| `ride.wav` | original noise + metallic partials |
| `ride_bell.wav` | original metallic partials |
| `cowbell.wav` | original dual tone + noise |
| `tambourine.wav` | original noise + metallic partials |

Regenerate with `python crates/drumpad-model/examples/generate_demo_kit.py`.
