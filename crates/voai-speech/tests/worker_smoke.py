"""Verify a built Voai speech worker with a local GGML model and a 16 kHz mono WAV.

python3 crates/voai-speech/tests/worker_smoke.py /path/to/voai /path/to/model.bin \
    /path/to/jfk.wav en "your country"
"""
import argparse
import array
import io
import struct
import subprocess
import wave


def frame(kind, samples):
    return struct.pack("<BI", kind, len(samples)) + samples.tobytes()


def reply(stream):
    header = stream.read(5)
    if len(header) != 5:
        raise ValueError("Missing worker reply")
    status, size = struct.unpack("<BI", header)
    if size > 1 << 20:
        raise ValueError("Invalid worker reply length")
    data = stream.read(size)
    if len(data) != size:
        raise ValueError("Truncated worker reply")
    text = data.decode()
    if status:
        raise RuntimeError(text)
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "model", "wav", "language", "expected_fragment"):
        parser.add_argument(name)
    args = parser.parse_args()
    with wave.open(args.wav) as audio:
        if (audio.getnchannels(), audio.getframerate(), audio.getsampwidth()) != (1, 16000, 2):
            parser.error("Fixture must be a mono 16-bit PCM WAV at 16 kHz")
        integers = array.array("h", audio.readframes(audio.getnframes()))
    samples = array.array("f", (value / 32768.0 for value in integers))
    requests = [
        b"VOAIS001",
        frame(0, samples[: max(16000, len(samples) // 2)]),
        frame(1, samples),
        frame(1, array.array("f", [0]) * 16000),
    ]
    run = subprocess.run(
        [args.binary, "--speech-worker", args.model, args.language],
        input=b"".join(requests), stdout=subprocess.PIPE, check=True, timeout=180,
    )
    messages = io.BytesIO(run.stdout)
    if reply(messages) != "":
        raise AssertionError("Invalid model handshake")
    partial, final, silence = (reply(messages) for _ in range(3))
    if not partial or args.expected_fragment.casefold() not in final.casefold():
        raise AssertionError("Missing preview or expected phrase in final result")
    if silence or messages.read(1):
        raise AssertionError("Stale text or unexpected output after finishing")
    print("Model handshake, partial, final, silence, and orderly worker exit passed.")


if __name__ == "__main__":
    main()
