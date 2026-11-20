"""Exercise a packaged Voai offline on a sparse hour-long movie.

python3 tests/macos/file_transcription.py Voai.app Whisper-Small.bin
Requires ffmpeg (fixture generation only) and the macOS Milena voice.
"""
import argparse
import json
import re
import struct
import subprocess
import tempfile
import time
from pathlib import Path


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, timeout=180)


def transcript(worker, model, pcm):
    result = run(worker, "--file-worker", model, "ru", pcm)
    updates = [json.loads(line) for line in result.stdout.splitlines()]
    progress = [row["progress"] for row in updates if "progress" in row]
    assert progress == sorted(progress) and progress[-1] == 100
    return [word for row in updates for word in row.get("words", [])]


def normalized(word):
    return re.sub(r"[^\w]", "", word["text"].casefold()).replace("ё", "е")


def exercise(bundle, model, work):
    bin_dir = bundle / "Contents/MacOS"
    media, worker = bin_dir / "voai-media-helper", bin_dir / "voai"
    phrase = "Давай перенесём встречу на пятницу."
    run("/usr/bin/say", "-v", "Milena", "-o", work / "voice.aiff", phrase)
    run(media, "decode", work / "voice.aiff", work / "voice.pcm")
    audio = (work / "voice.pcm").read_bytes()
    reference = [normalized(w) for w in transcript(worker, model, work / "voice.pcm")]
    assert "пятницу" in reference

    # AAC has encoder priming. Its decoded timeline must still match the original.
    run("ffmpeg", "-v", "error", "-i", work / "voice.aiff", "-af", "adelay=1200:all=1",
        "-c:a", "aac", work / "voice.m4a")
    run(media, "decode", work / "voice.m4a", work / "aac.pcm")
    aac_words = transcript(worker, model, work / "aac.pcm")
    assert [normalized(w) for w in aac_words] == reference
    assert 700 <= aac_words[0]["start_ms"] <= 1700
    run(media, "play", work / "voice.m4a", "100", "300")

    # The video starts first. PTS determines the audio's offset within the movie.
    run("ffmpeg", "-v", "error", "-f", "s16le", "-ar", "16000", "-ac", "1",
        "-i", work / "voice.pcm", "-c:a", "pcm_s16le", work / "voice.wav")
    run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=s=32x32:r=1:d=8",
        "-itsoffset", "2", "-i", work / "voice.wav", "-c:v", "h264", "-c:a", "alac",
        work / "delayed.mp4")
    run(media, "decode", work / "delayed.mp4", work / "delayed.pcm")
    delayed = (work / "delayed.pcm").read_bytes()
    assert len(delayed) == 8 * 32000
    assert delayed[:2 * 32000] == bytes(2 * 32000)
    assert delayed[2 * 32000:2 * 32000 + len(audio)] == audio

    # The second utterance crosses the first Whisper chunk boundary at 24 s.
    # The voice at 3 s returns half an hour later. The last words test the tail.
    starts = [3, 23, 1805, 3596]
    size = 3600 * 32000
    with (work / "hour.wav").open("wb") as out:
        out.write(struct.pack("<4sI4s4sIHHIIHH4sI", b"RIFF", size + 36, b"WAVE", b"fmt ",
                              16, 1, 1, 16000, 32000, 2, 16, b"data", size))
        out.truncate(44 + size)
        for second in starts:
            out.seek(44 + second * 32000)
            out.write(audio)
    run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=s=32x32:r=1/300:d=3600",
        "-i", work / "hour.wav", "-c:v", "h264", "-c:a", "alac", work / "hour.mov")
    run(media, "decode", work / "hour.mov", work / "hour.pcm")
    assert (work / "hour.pcm").stat().st_size == size
    words = transcript(worker, model, work / "hour.pcm")
    assert [normalized(w) for w in words] == reference * len(starts), "Missing or repeated words"
    for index, second in enumerate(starts):
        group = words[index * len(reference):(index + 1) * len(reference)]
        assert second * 1000 - 500 <= group[0]["start_ms"] <= second * 1000 + 500
        assert group[-1]["end_ms"] <= second * 1000 + len(audio) // 32 + 500
    assert all(w["start_ms"] < w["end_ms"] for w in words)
    assert all(a["end_ms"] <= b["start_ms"] for a, b in zip(words, words[1:]))

    # Play a silent interval and verify the empty-track and playlist error paths.
    run(media, "play", work / "hour.mov", "500", "650")
    # Stop during a silent part of the source and immediately start another clip.
    with subprocess.Popen([media, "play", work / "hour.mov", "60000", "120000"],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) as player:
        time.sleep(0.3)
        assert player.poll() is None
        player.kill()
        assert player.wait(timeout=3) < 0
    run(media, "play", work / "hour.mov", "3599700", "3600000")
    run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "color=s=32x32:d=1",
        "-c:v", "h264", work / "silent.mp4")
    manifest = work / "renamed.mp4"
    manifest.write_text("#EXTM3U\n#EXTINF:10,\nclip.ts\n")
    for source, expected in [(work / "silent.mp4", "нет аудиодорожки"), (manifest, "плейлиста")]:
        attempt = subprocess.run([media, "decode", source, work / "invalid.pcm"],
                                 capture_output=True, text=True, timeout=15)
        assert attempt.returncode != 0 and expected in attempt.stderr
    print("Verified M4A, one-hour video, chunk seam, time offsets, playback and invalid inputs.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    parser.add_argument("model", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="voai-file-test-") as tmp:
        exercise(args.bundle.resolve(), args.model.resolve(), Path(tmp))
