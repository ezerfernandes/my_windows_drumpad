#!/usr/bin/env python3
"""Generate the embedded Drumpad demo kit assets.

The output is deterministic so the manifest hashes can be reproduced:

    python crates/drumpad-model/examples/generate_demo_kit.py

Every sound is synthesized from scratch (noises, sine sweeps, metallic
partials) and is dedicated to CC0 alongside the kit manifest.
"""

from __future__ import annotations

import hashlib
import json
import math
import random
import struct
import wave
from pathlib import Path

RATE = 48_000
ASSET_ROOT = Path(__file__).resolve().parents[1] / "assets" / "demo-kit"
SAMPLE_DIR = ASSET_ROOT / "samples"
KIT_ID = "d3a0f0b1-5b1e-4b7a-9b5d-0d2c4e6f8a10"
CREATED_MS = 1_762_531_200_000


def noise(count: int, seed: int) -> list[float]:
    rng = random.Random(seed)
    return [rng.uniform(-1.0, 1.0) for _ in range(count)]


def env_exp(count: int, decay: float, attack: float = 0.002) -> list[float]:
    values = []
    for index in range(count):
        t = index / RATE
        attack_gain = min(1.0, t / attack) if attack > 0 else 1.0
        values.append(attack_gain * math.exp(-t / decay))
    return values


def sine_sweep(count: int, start_hz: float, end_hz: float, decay: float) -> list[float]:
    values = []
    phase = 0.0
    for index in range(count):
        t = index / RATE
        frequency = end_hz + (start_hz - end_hz) * math.exp(-t / (decay * 0.35))
        phase += 2.0 * math.pi * frequency / RATE
        values.append(math.sin(phase) * math.exp(-t / decay))
    return values


def mix(*parts: list[float]) -> list[float]:
    length = max(len(part) for part in parts)
    out = [0.0] * length
    for part in parts:
        for index, value in enumerate(part):
            out[index] += value
    return out


def scale(values: list[float]) -> list[float]:
    peak = max(1e-9, max(abs(value) for value in values))
    gain = 0.92 / peak
    return [max(-1.0, min(1.0, value * gain)) for value in values]


def metallic(count: int, partials: list[float], decay: float, seed: int) -> list[float]:
    rng = random.Random(seed)
    phases = [rng.uniform(0.0, 2 * math.pi) for _ in partials]
    out = []
    for index in range(count):
        t = index / RATE
        value = 0.0
        for frequency, phase in zip(partials, phases):
            value += math.sin(2 * math.pi * frequency * t + phase)
        value /= max(1, len(partials))
        out.append(value * math.exp(-t / decay))
    return out


def make_sounds() -> dict[str, list[float]]:
    sounds: dict[str, list[float]] = {
        "kick.wav": scale(
            mix(
                sine_sweep(int(0.55 * RATE), 165, 42, 0.22),
                [v * 0.15 for v in noise(int(0.02 * RATE), 1)],
            )
        ),
        "snare.wav": scale(
            mix(
                sine_sweep(int(0.28 * RATE), 210, 160, 0.12),
                [v * 0.85 for v in noise(int(0.28 * RATE), 2)],
            )
        ),
        "rim.wav": scale(
            mix(
                sine_sweep(int(0.09 * RATE), 900, 700, 0.03),
                [v * 0.5 for v in noise(int(0.05 * RATE), 3)],
            )
        ),
        "clap.wav": scale(
            mix(
                noise(int(0.3 * RATE), 4),
                [0.0] * int(0.012 * RATE) + noise(int(0.28 * RATE), 5),
                [0.0] * int(0.024 * RATE) + noise(int(0.26 * RATE), 6),
            )
        ),
        "floor_tom.wav": scale(sine_sweep(int(0.75 * RATE), 130, 55, 0.35)),
        "tom_low.wav": scale(sine_sweep(int(0.6 * RATE), 175, 75, 0.3)),
        "tom_mid.wav": scale(sine_sweep(int(0.55 * RATE), 220, 95, 0.28)),
        "tom_high.wav": scale(sine_sweep(int(0.5 * RATE), 285, 120, 0.24)),
        "hat_closed.wav": scale(
            mix(
                [v * 0.9 for v in noise(int(0.08 * RATE), 7)],
                metallic(int(0.08 * RATE), [6200, 8100, 10400], 0.018, 8),
            )
        ),
        "hat_pedal.wav": scale(
            mix(
                [v * 0.8 for v in noise(int(0.2 * RATE), 9)],
                metallic(int(0.2 * RATE), [5200, 7400, 9300], 0.06, 10),
            )
        ),
        "hat_open.wav": scale(
            mix(
                [v * 0.8 for v in noise(int(1.1 * RATE), 11)],
                metallic(int(1.1 * RATE), [4800, 6900, 8800, 11200], 0.5, 12),
            )
        ),
        "crash.wav": scale(
            mix(
                [v * 0.55 for v in noise(int(1.8 * RATE), 13)],
                metallic(int(1.8 * RATE), [3200, 4700, 6100, 7900], 0.9, 14),
            )
        ),
        "ride.wav": scale(
            mix(
                [v * 0.25 for v in noise(int(1.4 * RATE), 15)],
                metallic(int(1.4 * RATE), [2800, 4300, 5900, 7200], 0.75, 16),
            )
        ),
        "ride_bell.wav": scale(
            mix(
                metallic(int(0.9 * RATE), [2200, 3300, 4450, 6100], 0.45, 17),
                [v * 0.18 for v in noise(int(0.9 * RATE), 18)],
            )
        ),
        "cowbell.wav": scale(
            mix(
                sine_sweep(int(0.45 * RATE), 540, 540, 0.22),
                sine_sweep(int(0.45 * RATE), 800, 800, 0.22),
                [v * 0.2 for v in noise(int(0.45 * RATE), 19)],
            )
        ),
        "tambourine.wav": scale(
            mix(
                [v * 0.8 for v in noise(int(0.6 * RATE), 20)],
                metallic(int(0.6 * RATE), [5600, 7300, 9100, 11500], 0.2, 21),
            )
        ),
    }
    return sounds


def write_wav(path: Path, samples: list[float]) -> None:
    frames = b"".join(struct.pack("<h", int(round(value * 32767))) for value in samples)
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(RATE)
        handle.writeframes(frames)


def read_wav(path: Path) -> list[float]:
    with wave.open(str(path), "rb") as handle:
        assert handle.getnchannels() == 1
        assert handle.getframerate() == RATE
        frames = handle.readframes(handle.getnframes())
    values = struct.unpack("<%dh" % (len(frames) // 2), frames)
    return [value / 32768.0 for value in values]


def content_hash(samples: list[float]) -> str:
    digest = hashlib.sha256()
    digest.update(b"drumpad-pcm-v1")
    digest.update(struct.pack("<I", RATE))
    digest.update(struct.pack("<H", 1))
    digest.update(struct.pack("<Q", len(samples)))
    digest.update(struct.pack("<%df" % len(samples), *samples))
    return digest.hexdigest()


PADS = [
    # file stem, pad name, MIDI note, gain dB, pan, choke group
    ("kick", "Kick", 36, 0.0, 0.0, None),
    ("snare", "Snare", 38, -1.0, 0.0, None),
    ("rim", "Rimshot", 40, -6.0, 0.0, None),
    ("clap", "Clap", 39, -4.0, 0.0, None),
    ("floor_tom", "Floor Tom", 41, -3.0, -0.25, None),
    ("tom_low", "Tom Low", 45, -3.0, -0.15, None),
    ("tom_mid", "Tom Mid", 48, -3.0, 0.15, None),
    ("tom_high", "Tom High", 43, -3.0, 0.25, None),
    ("hat_closed", "Hat Closed", 42, -10.0, 0.18, 1),
    ("hat_pedal", "Hat Pedal", 44, -12.0, 0.18, 1),
    ("hat_open", "Hat Open", 46, -11.0, 0.18, 1),
    ("crash", "Crash", 49, -8.0, -0.2, None),
    ("ride", "Ride", 51, -10.0, 0.2, None),
    ("ride_bell", "Ride Bell", 53, -10.0, 0.2, None),
    ("cowbell", "Cowbell", 56, -8.0, 0.0, None),
    ("tambourine", "Tambourine", 54, -10.0, 0.3, None),
]

SCANCODES = [0x02, 0x03, 0x04, 0x05, 0x10, 0x11, 0x12, 0x13,
             0x1E, 0x1F, 0x20, 0x21, 0x2C, 0x2D, 0x2E, 0x2F]


def build_kit(hashes: dict[str, str]) -> dict:
    pads = []
    for slot, (stem, name, note, gain, pan, choke) in enumerate(PADS):
        sample_path = f"samples/{stem}.wav"
        pad = {
            "id": slot,
            "name": name,
            "layers": [
                {
                    "id": 1,
                    "sample": {
                        "path": sample_path,
                        "hash": hashes[stem],
                        "original_name": f"{stem}.wav",
                    },
                    "velocity": {"min": 1, "max": 127},
                    "gain_db": 0.0,
                    "tune_cents": 0.0,
                }
            ],
            "playback": "one_shot",
            "gain_db": gain,
            "pan": pan,
            "tune_semitones": 0.0,
            "midi": {"note": note, "channel": 1},
            "key": {"key": SCANCODES[slot]},
        }
        if choke is not None:
            pad["choke_group"] = choke
        pads.append(pad)
    return {
        "format_version": 1,
        "id": KIT_ID,
        "metadata": {
            "name": "Drumpad Demo",
            "author": "Drumpad project",
            "description": "Factory demo kit made from original CC0 synthesized one-shots.",
            "tags": ["demo", "factory", "cc0"],
            "created_unix_ms": CREATED_MS,
            "modified_unix_ms": CREATED_MS,
        },
        "pads": pads,
    }


LICENSES = """# Drumpad Demo Kit licenses

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
"""


def main() -> None:
    SAMPLE_DIR.mkdir(parents=True, exist_ok=True)
    sounds = make_sounds()
    hashes: dict[str, str] = {}
    for stem, samples in sounds.items():
        path = SAMPLE_DIR / stem
        write_wav(path, samples)
        decoded = read_wav(path)
        hashes[stem.removesuffix(".wav")] = content_hash(decoded)
        print(f"{stem:20s} {len(decoded):7d} frames  {hashes[stem.removesuffix('.wav')][:12]}")
    kit = build_kit(hashes)
    (ASSET_ROOT / "kit.json").write_text(json.dumps(kit, indent=2) + "\n", encoding="utf-8")
    (ASSET_ROOT / "LICENSES.md").write_text(LICENSES, encoding="utf-8")
    print(f"wrote {ASSET_ROOT}")


if __name__ == "__main__":
    main()
