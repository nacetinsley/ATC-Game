"""Render the game's voices to clips, on any platform.

Two sets, both rendered once with Piper (https://github.com/OHF-Voice/piper1-gpl)
and committed, so the game itself needs no speech engine and plays them the
same on Windows, macOS and Linux:

- The radio voices: a clip for every word and stock phrase in
  assets/sounds/voice/words.txt, in each voice, packed into one file per voice
  (assets/sounds/voice/<voice>.pack). The game joins them into transmissions:
  "Nova one zero one, turn left heading two four zero".
- The ASDE-X alert words (--asdex): WAV clips joined into alerts such as
  "Warning, runway two seven left, arrival, runway occupied".

Every voice is trained from scratch on public-domain recordings, so the clips
carry no licence terms:
- en_US-norman-medium (the controller) and en_US-john-medium: LibriVox;
- en_US-kristin-medium: LibriVox;
- en_US-ljspeech-high: the LJ Speech dataset (also the ASDE-X voice);
- en_GB-cori-medium: LibriVox.
Piper is GPL; the audio it produces isn't covered by that licence.

The vocabulary comes from the game: write it with
    cargo test --release --test voice -- --ignored write_vocabulary
Words the voice would misread (fix names, mostly) are respelled in
assets/sounds/voice/say.txt, one "word = how to say it" a line.

Usage, from the repository root:

    python -m pip install piper-tts onnx
    python tools/make_voice.py                # the radio voice packs
    python tools/make_voice.py --voice cori   # one voice's pack
    python tools/make_voice.py --asdex        # the ASDE-X clips

Voice models are downloaded to tools/.cache/piper the first time.
"""

import argparse
import array
import math
import struct
import subprocess
import sys
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "tools" / ".cache" / "piper"
VOICE_DIR = ROOT / "assets" / "sounds" / "voice"

# --- ASDE-X ---------------------------------------------------------------

ASDEX_VOICE = "en_US-ljspeech-high"
ASDEX_RATE = 22050
ASDEX_WORDS = {
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

# --- Radio voices -----------------------------------------------------------

# Pack name -> Piper voice. The game picks the roles (see src/ui/voice.rs).
RADIO_VOICES = {
    "norman": "en_US-norman-medium",
    "john": "en_US-john-medium",
    "kristin": "en_US-kristin-medium",
    "ljspeech": "en_US-ljspeech-high",
    "cori": "en_GB-cori-medium",
}
# VHF voice is band-limited to about 300-3,000 Hz: 8 kHz holds all of it.
RADIO_RATE = 8000
# Radio talk is brisk.
LENGTH_SCALE = 0.88
# How a clip is said, where its spelling alone would mislead the voice.
SAY_AS = {
    "niner": "niner",
    "x-ray": "x-ray",
    "vfr": "V F R",
    "ifr": "I F R",
    "ils": "I L S",
    "rnav": "R nav",
    "efc": "E F C",
    "atis": "ATIS",
    "mva": "M V A",
    "lvp": "L V P",
    "i": "I",
    "i'll": "I'll",
    "i'll call your base": "I'll call your base",
    "i say again": "I say again",
    "iii": "three",
    "fedex": "FedEx",
    "skywest": "SkyWest",
    "o'hare": "O'Hare",
    "alfa": "alpha",
    "juliett": "juliet",
}
# A word said on its own trips Piper up: it drags short words out to
# seconds. Each clip is said inside this sentence instead and cut out of it
# by the phoneme alignments. With no comma after it, the word is said as in
# the middle of a sentence, at running pace: a word before a pause is drawn
# out, a quarter longer.
CARRIER = "Then {} then."
# A clip key ending " ." is said at the end of a sentence, with its fall in
# pitch; one ending " ," before a comma, held up to go on. They come from
# carriers that end there.
AT_END = " ."
BEFORE_COMMA = " ,"
END_CARRIER = "Then then {}."
COMMA_CARRIER = "Then then {}, then."
# Kept either side of the cut. Small where a word runs straight on, so as
# not to take in a piece of "then"; more where a pause follows.
LEAD_S = 0.004
TAIL_S = 0.006
PAUSE_TAIL_S = 0.04
# Each clip's loudness is set halfway between its own and its sentence's, so
# a stressed word stays louder than the little ones round it.
LEVEL = 0.18
PACK_MAGIC = b"ATCV"
PACK_VERSION = 1


def load_voice(name, alignments=False):
    try:
        from piper import PiperVoice
    except ImportError:
        sys.exit("Piper isn't installed: python -m pip install piper-tts")
    CACHE.mkdir(parents=True, exist_ok=True)
    model = CACHE / f"{name}.onnx"
    if not model.exists():
        subprocess.run(
            [sys.executable, "-m", "piper.download_voices", "--download-dir", str(CACHE), name],
            check=True,
        )
    return PiperVoice.load(str(model), include_alignments=alignments)


def synthesize(voice, text, path, config=None):
    """16-bit mono samples of `text`, and their rate."""
    with wave.open(str(path), "wb") as f:
        voice.synthesize_wav(text, f, syn_config=config)
    with wave.open(str(path), "rb") as f:
        if f.getnchannels() != 1 or f.getsampwidth() != 2:
            sys.exit(f"{text}: expected 16-bit mono from the voice")
        rate = f.getframerate()
        samples = array.array("h", f.readframes(f.getnframes()))
    path.unlink()
    return samples, rate


def resample_linear(samples, rate, to):
    """Linear resampling, for the ASDE-X clips."""
    if rate == to:
        return samples
    count = int(len(samples) * to / rate)
    out = array.array("h")
    for i in range(count):
        pos = i * rate / to
        j = int(pos)
        frac = pos - j
        a = samples[j]
        b = samples[min(j + 1, len(samples) - 1)]
        out.append(int(a + (b - a) * frac))
    return out


def asdex():
    voice = load_voice(ASDEX_VOICE)
    VOICE_DIR.mkdir(parents=True, exist_ok=True)
    for name, text in ASDEX_WORDS.items():
        samples, rate = synthesize(voice, text, VOICE_DIR / f"{name}.tmp.wav")
        samples = resample_linear(samples, rate, ASDEX_RATE)
        path = VOICE_DIR / f"{name}.wav"
        with wave.open(str(path), "wb") as f:
            f.setnchannels(1)
            f.setsampwidth(2)
            f.setframerate(ASDEX_RATE)
            f.writeframes(samples.tobytes())
        print(f"wrote {path.relative_to(ROOT)}")


def pronunciations():
    """Word -> how to say it, from assets/sounds/voice/say.txt: fix names and
    the like whose spelling misleads the voice."""
    path = VOICE_DIR / "say.txt"
    say = dict(SAY_AS)
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.split("#", 1)[0].strip()
            if "=" in line:
                word, spoken = (part.strip() for part in line.split("=", 1))
                say[word] = spoken
    return say


def spoken_text(key, say):
    """What the voice is given for a clip key: its words, each as said."""
    words = key.removesuffix(AT_END).removesuffix(BEFORE_COMMA)
    if words in say:
        return say[words]
    return " ".join(say.get(w, w) for w in words.split(" "))


def vocabulary():
    path = VOICE_DIR / "words.txt"
    if not path.exists():
        sys.exit("No words.txt: cargo test --release --test voice -- --ignored write_vocabulary")
    lines = [l.strip() for l in path.read_text(encoding="utf-8").splitlines()]
    return [l for l in lines if l and not l.startswith("#")]


def rms(np, x):
    """Loudness of the sounding part of `x`."""
    x = np.asarray(x, dtype=np.float64)
    if len(x) == 0:
        return 0.0
    voiced = x[np.abs(x) > 0.02 * max(float(np.max(np.abs(x))), 1e-9)]
    return math.sqrt(float(np.mean(voiced * voiced))) if len(voiced) else 0.0


def carried(np, voice, key, text, config):
    """16-bit samples of `text` cut from its carrier sentence, their rate,
    and how loud the clip is next to the whole sentence."""
    if key.endswith(AT_END):
        carrier, skip, stop, tail = END_CARRIER, 2, ".", PAUSE_TAIL_S
    elif key.endswith(BEFORE_COMMA):
        carrier, skip, stop, tail = COMMA_CARRIER, 2, ",", PAUSE_TAIL_S
    else:
        carrier, skip, stop, tail = CARRIER, 1, None, TAIL_S
    chunks = list(voice.synthesize(carrier.format(text), config, include_alignments=True))
    if len(chunks) != 1 or not chunks[0].phoneme_alignments:
        sys.exit(f"{text}: no phoneme alignments (is onnx installed?)")
    chunk = chunks[0]
    phonemes = [a.phoneme for a in chunk.phoneme_alignments]
    ends = np.cumsum([a.num_samples for a in chunk.phoneme_alignments])
    starts = ends - [a.num_samples for a in chunk.phoneme_alignments]
    # The clip starts after the space following the carrier's "Then"s, and
    # runs to the space before the last "then", or to the comma or full stop.
    first = 0
    for _ in range(skip):
        first = phonemes.index(" ", first) + 1
    if stop is None:
        last = len(phonemes) - 1 - phonemes[::-1].index(" ")
    else:
        last = len(phonemes) - 1 - phonemes[::-1].index(stop)
    rate = chunk.sample_rate
    audio = chunk.audio_int16_array
    start = max(int(starts[first]) - int(LEAD_S * rate), 0)
    end = min(int(starts[last]) + int(tail * rate), len(audio))
    clip = audio[start:end]
    whole = rms(np, audio)
    relative = rms(np, clip) / whole if whole > 0 else 1.0
    return clip, rate, relative


def radio_clip(np, samples, rate, relative=1.0):
    """A rendered word as radio audio: trimmed, band-limited to 3.4 kHz,
    resampled to 8 kHz, levelled, and µ-law encoded."""
    x = np.asarray(samples, dtype=np.float64) / 32768.0
    if len(x) == 0:
        return b""
    # Trim the silence the voice leaves around a word.
    loud = np.nonzero(np.abs(x) > 0.02 * max(np.max(np.abs(x)), 1e-9))[0]
    if len(loud):
        margin = int(0.015 * rate)
        x = x[max(loud[0] - margin, 0) : min(loud[-1] + margin, len(x))]
    # Low-pass (windowed sinc) before taking it down to 8 kHz.
    cutoff = 3400.0 / rate
    taps = np.arange(-48, 49)
    kernel = np.sinc(2 * cutoff * taps) * np.hamming(len(taps))
    kernel /= kernel.sum()
    x = np.convolve(x, kernel, mode="same")
    count = int(len(x) * RADIO_RATE / rate)
    x = np.interp(np.arange(count) * rate / RADIO_RATE, np.arange(len(x)), x)
    # Clips at about the same loudness, so joined words don't jump about,
    # but a word louder in its sentence a little louder here too.
    level = math.sqrt(float(np.mean(x * x))) if len(x) else 0.0
    if level > 1e-6:
        x *= LEVEL * math.sqrt(max(relative, 0.25)) / level
    x = np.clip(x, -1.0, 1.0)
    # µ-law, the continuous form: the game decodes it the same way.
    mu = 255.0
    y = np.sign(x) * np.log1p(mu * np.abs(x)) / math.log1p(mu)
    return np.round((y + 1.0) * 127.5).astype(np.uint8).tobytes()


def radio(only=None):
    try:
        import numpy as np
        from piper import SynthesisConfig
    except ImportError:
        sys.exit("Piper isn't installed: python -m pip install piper-tts")
    words = vocabulary()
    say = pronunciations()
    config = SynthesisConfig(length_scale=LENGTH_SCALE)
    for pack, model in RADIO_VOICES.items():
        if only and pack != only:
            continue
        voice = load_voice(model, alignments=True)
        entries = []
        data = bytearray()
        for n, word in enumerate(words):
            samples, rate, relative = carried(np, voice, word, spoken_text(word, say), config)
            clip = radio_clip(np, samples, rate, relative)
            entries.append((word, len(data), len(clip)))
            data += clip
            if n % 100 == 0:
                print(f"{pack}: {n}/{len(words)}")
        header = bytearray(PACK_MAGIC)
        header += struct.pack("<III", PACK_VERSION, RADIO_RATE, len(entries))
        for word, offset, length in entries:
            key = word.encode("utf-8")
            header += struct.pack("<H", len(key)) + key + struct.pack("<II", offset, length)
        path = VOICE_DIR / f"{pack}.pack"
        path.write_bytes(bytes(header) + bytes(data))
        print(f"wrote {path.relative_to(ROOT)}: {len(entries)} clips, {path.stat().st_size // 1024} KB")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--asdex", action="store_true", help="render the ASDE-X clips instead")
    parser.add_argument("--voice", choices=sorted(RADIO_VOICES), help="render only this voice's pack")
    args = parser.parse_args()
    if args.asdex:
        asdex()
    else:
        radio(args.voice)


if __name__ == "__main__":
    main()
