"""Render the ASDE-X voice alert words to WAV clips, on any platform.

The game joins the clips into alerts such as "Warning, runway two seven left,
arrival, runway occupied". They're rendered once with Piper
(https://github.com/OHF-Voice/piper1-gpl) and committed, so the game itself
needs no speech engine and plays them the same on Windows, macOS and Linux.

The voice is Piper's en_US-ljspeech-high, trained on the LJ Speech dataset,
which is in the public domain. Piper is GPL; the audio it produces isn't
covered by that licence.

Usage, from the repository root:

    python -m pip install piper-tts
    python tools/make_voice.py

The voice model is downloaded to tools/.cache/piper the first time.
"""

import argparse
import array
import subprocess
import sys
import wave
from pathlib import Path

VOICE = "en_US-ljspeech-high"
SAMPLE_RATE = 22050
WORDS = {
    "warning": "Warning.",
    "runway": "runway",
    "zero": "zero",
    "one": "one",
    "two": "two",
    "three": "three",
    "four": "four",
    "five": "five",
    "six": "six",
    "seven": "seven",
    "eight": "eight",
    "niner": "niner",
    "left": "left",
    "right": "right",
    "center": "center",
    "arrival": "arrival",
    "departure": "departure",
    "occupied": "occupied.",
}

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "tools" / ".cache" / "piper"


def resample(samples, rate):
    """Linear resampling to the game's rate, if the voice uses another."""
    if rate == SAMPLE_RATE:
        return samples
    count = int(len(samples) * SAMPLE_RATE / rate)
    out = array.array("h")
    for i in range(count):
        pos = i * rate / SAMPLE_RATE
        j = int(pos)
        frac = pos - j
        a = samples[j]
        b = samples[min(j + 1, len(samples) - 1)]
        out.append(int(a + (b - a) * frac))
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", default=str(ROOT / "assets" / "sounds" / "voice"))
    args = parser.parse_args()

    try:
        from piper import PiperVoice
    except ImportError:
        sys.exit("Piper isn't installed: python -m pip install piper-tts")

    CACHE.mkdir(parents=True, exist_ok=True)
    model = CACHE / f"{VOICE}.onnx"
    if not model.exists():
        subprocess.run(
            [sys.executable, "-m", "piper.download_voices", "--download-dir", str(CACHE), VOICE],
            check=True,
        )
    voice = PiperVoice.load(str(model))

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    for name, text in WORDS.items():
        rendered = out_dir / f"{name}.tmp.wav"
        with wave.open(str(rendered), "wb") as f:
            voice.synthesize_wav(text, f)
        with wave.open(str(rendered), "rb") as f:
            if f.getnchannels() != 1 or f.getsampwidth() != 2:
                sys.exit(f"{name}: expected 16-bit mono from the voice")
            rate = f.getframerate()
            samples = array.array("h", f.readframes(f.getnframes()))
        rendered.unlink()
        samples = resample(samples, rate)
        path = out_dir / f"{name}.wav"
        with wave.open(str(path), "wb") as f:
            f.setnchannels(1)
            f.setsampwidth(2)
            f.setframerate(SAMPLE_RATE)
            f.writeframes(samples.tobytes())
        print(f"wrote {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
