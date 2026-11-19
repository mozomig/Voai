#!/usr/bin/env python3
"""Run the offline dictation acceptance test on macOS.

Usage: python3 tests/macos/dictation.py Voai.app tiny.bin Qwen.gguf ru.wav

Use a mono 16-bit PCM WAV saying «Давай перенесём встречу на пятницу».
To recreate one with an installed Russian voice:
    say -v Milena -o /tmp/voai-friday.aiff 'Давай перенесём встречу на пятницу.'
    afconvert -f WAVE -d LEI16@16000 -c 1 /tmp/voai-friday.aiff /tmp/voai-friday.wav
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import wave


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("bundle", "whisper", "qwen", "wav"):
        parser.add_argument(arg, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    macos = args.bundle.resolve() / "Contents/MacOS"
    for path in [macos / "voai", macos / "voai-text-helper", args.whisper, args.qwen, args.wav]:
        if not path.is_file():
            parser.error(f"Missing file: {path}")
    with tempfile.TemporaryDirectory(prefix="voai-dictation-test-") as tmp:
        work = Path(tmp)
        with wave.open(str(args.wav), "rb") as wav:
            if (wav.getnchannels(), wav.getsampwidth(), wav.getcomptype()) != (1, 2, "NONE"):
                parser.error("Expected a mono, signed 16-bit PCM WAV")
            if not 1 <= wav.getnframes() / wav.getframerate() <= 30:
                parser.error("Expected a speech fixture between 1 and 30 seconds")
            rate = wav.getframerate()
            pcm = work / "utterance.pcm"
            pcm.write_bytes(wav.readframes(wav.getnframes()))
        build = subprocess.run(
            [root / "scripts/cargo-macos", "test", "--locked", "-p", "voai", "--bin", "voai",
             "--no-run", "--message-format=json-render-diagnostics"],
            stdout=subprocess.PIPE, text=True, check=True,
        )
        messages = (json.loads(line) for line in build.stdout.splitlines())
        executable = next(
            item["executable"] for item in messages if item.get("executable")
            and item["target"]["name"] == "voai" and item["profile"]["test"]
        )
        # The production local editor discovers its helper next to the executable.
        runner = work / "acceptance"
        try:
            os.link(executable, runner)
        except OSError:
            shutil.copy2(executable, runner)
        (work / "voai-text-helper").symlink_to(macos / "voai-text-helper")
        env = dict(os.environ, VOAI_QA_WORKER=str(macos / "voai"),
                   VOAI_QA_WHISPER=str(args.whisper.resolve()), VOAI_QA_QWEN=str(args.qwen.resolve()),
                   VOAI_QA_PCM=str(pcm), VOAI_QA_RATE=str(rate))
        result = subprocess.run(
            [runner, "--exact", "app::session::acceptance::recorded_speech_reaches_test_field",
             "--ignored", "--nocapture"], env=env, stdout=subprocess.PIPE, text=True,
            timeout=420,
        )
        print(result.stdout, end="")
        result.check_returncode()
        if "1 passed" not in result.stdout:
            raise SystemExit("The dictation acceptance test did not run")


if __name__ == "__main__":
    main()
