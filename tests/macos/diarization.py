#!/usr/bin/env python3
"""Evaluate Voai's two local helpers on pinned Veche Bench meeting mixes (stdlib)."""
import argparse
import csv
import itertools
import json
import re
import subprocess
import tempfile
import wave
from collections import Counter
from difflib import SequenceMatcher
from pathlib import Path

CASES = [("simple", "2-one-to-one"), ("simple", "3-auth-incident"), ("medium", "3-auth-incident")]
REV = "a0ab08d6887b64995d076b475a87310953a51138"
DURATION = 75


def reference(path, offset):
    turns = []
    with path.open() as stream:
        for row in csv.DictReader(stream):
            start = float(row["start"]) - offset
            end = float(row["end"]) - offset
            if start < DURATION and end > 0:
                turns.append((start, end, row["speaker_id"], row["text"]))
    return turns


def diarization_error(reference, predicted):
    # Keep the overlap in both timelines. Maximize agreement using a one-to-one
    # assignment. At most three reference speakers participate in these fixtures.
    agree = Counter()
    total = errors_before_assignment = 0
    for tick in range(DURATION * 100):
        time = (tick + 0.5) / 100
        actual = {speaker for start, end, speaker, _ in reference if start <= time < end}
        estimated = {speaker for start, end, speaker in predicted if start <= time < end}
        total += len(actual)
        errors_before_assignment += max(len(actual), len(estimated))
        agree.update(itertools.product(actual, estimated))
    ids = sorted({r[2] for r in reference})
    labels = sorted({r[2] for r in predicted})
    options = labels + [None] * max(0, len(ids) - len(labels))
    pairing = max(itertools.permutations(options, len(ids)),
                  key=lambda perm: sum(agree[pair] for pair in zip(ids, perm)))
    matrix = list(zip(ids, pairing))
    errors = errors_before_assignment - sum(agree[pair] for pair in matrix)
    return round(100 * errors / total, 2), {p: r for r, p in matrix if p is not None}


def tokens(text):
    return re.findall(r"[\w]+", text.lower().replace("ё", "е"))


def word_errors(turns, predicted, words, mapping):
    ref = [(token, start, end, speaker) for start, end, speaker, text in turns
           for token in tokens(text)]
    recognized = [(token, w["start_ms"] / 1000, w["end_ms"] / 1000)
                  for w in words for token in tokens(w["text"])]
    alignment = SequenceMatcher(None, [r[0] for r in ref], [w[0] for w in recognized], autojunk=False)
    matched = assigned_elsewhere = unassigned = 0
    for block in alignment.get_matching_blocks():
        for j in range(block.size):
            _, r_start, r_end, expected = ref[block.a + j]
            _, start, end = recognized[block.b + j]
            # Reject coincidental text matches in a different part of the recording.
            if end <= r_start - 1.5 or start >= r_end + 1.5:
                continue
            matched += 1
            overlap = Counter()
            for left, right, speaker in predicted:
                duration = min(end, right) - max(start, left)
                if duration > 0:
                    overlap[speaker] += duration
            if not overlap:
                unassigned += 1
            elif mapping.get(max(overlap, key=lambda s: (overlap[s], s))) != expected:
                assigned_elsewhere += 1
    return matched, assigned_elsewhere, unassigned


def report(ref, voice, words):
    turns = [(t["start_ms"] / 1000, t["end_ms"] / 1000, t["speaker"]) for t in voice]
    der, mapping = diarization_error(ref, turns)
    matched, wrong, missing = word_errors(ref, turns, words, mapping)
    return dict(der=der, speakers=len({t[2] for t in turns}),
                matched_words=matched, wrong_speaker=wrong, no_speaker=missing)


def run_helper(command):
    result = subprocess.run(["/usr/bin/time", "-l", *command], capture_output=True, check=True)
    stats = result.stderr.decode(errors="replace")
    rss = re.search(r"(\d+)\s+maximum resident set size", stats)
    wall = re.search(r"(\d+\.\d+) real\s+", stats)
    return ([json.loads(line) for line in result.stdout.splitlines()],
            dict(seconds=float(wall[1]), peak_mib=round(int(rss[1]) / 2**20, 1)))


def identified(turns, start_ms, end_ms):
    support = Counter()
    for t in turns:
        overlap = min(end_ms, t["end_ms"]) - max(start_ms, t["start_ms"])
        if overlap > 0:
            support[t["speaker"]] += overlap
    return support.most_common(1)[0][0] if support else None


def stream_regression(helper, models, fixtures):
    # A and B have multiple turns. C enters at 75 s. A returns with different
    # sentences, with the original speaker labels taken from the corpus CSV.
    def read_voice(case, speaker, skip):
        with (fixtures / f"{case}.csv").open() as rows:
            clips = [r for r in csv.DictReader(rows) if r["speaker_id"] == speaker]
        parts = []
        with wave.open(str(fixtures / f"{case}.wav")) as wav:
            for row in clips[skip:skip + 3]:
                start = round(float(row["start"]) * 16000)
                end = round(float(row["end"]) * 16000)
                wav.setpos(start)
                parts.append(wav.readframes(end - start))
        return bytes(6400).join(parts)

    a = read_voice("simple-2-one-to-one", "1", 0)
    b = read_voice("simple-2-one-to-one", "2", 0)
    c = read_voice("simple-3-auth-incident", "3", 0)
    later_a = read_voice("simple-2-one-to-one", "1", 4)
    for delay in (150, 1800, 5400):
        positions = [(10, a), (40, b), (75, c), (delay, later_a)]
        with tempfile.NamedTemporaryFile() as pcm:
            for pos, audio in positions:
                pcm.seek(pos * 32000)
                pcm.write(audio)
            pcm.flush()
            events, perf = run_helper([helper, pcm.name, models])
        turns = [turn for event in events for turn in event.get("turns", [])]
        labels = [identified(turns, pos*1000, pos*1000+len(audio)//32) for pos,audio in positions]
        assert all(label is not None for label in labels), labels
        assert len(set(labels[:3])) == 3 and labels[0] == labels[3], labels
        print(json.dumps(dict(return_at_s=delay, speaker_ids=labels, **perf)), flush=True)


    with wave.open(str(fixtures / "medium-3-auth-incident.wav")) as wav:
        clip = wav.readframes(75 * 16_000)
    for repetitions in (1, 3, 9):
        with tempfile.NamedTemporaryFile() as pcm:
            for _ in range(repetitions):
                pcm.write(clip)
            pcm.flush()
            events, perf = run_helper([helper, pcm.name, models])
        labels = {t["speaker"] for e in events for t in e.get("turns", [])}
        print(json.dumps(dict(active_seconds=75 * repetitions, speakers=len(labels), **perf)), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path, help="signed Voai.app")
    parser.add_argument("models", type=Path, help="diarization ONNX pair")
    parser.add_argument("whisper", type=Path, help="Whisper GGML file")
    parser.add_argument("fixtures", type=Path, help="local Veche Bench samples")
    parser.add_argument("--download-fixtures", action="store_true")
    parser.add_argument("--stream-check", action="store_true", help="add sparse recordings up to 90 minutes")
    args = parser.parse_args()
    bin_dir = args.app / "Contents/MacOS"
    args.fixtures.mkdir(parents=True, exist_ok=True)
    for kind, name in CASES:
        base = f"{kind}-{name}"
        wav = args.fixtures / f"{base}.wav"
        labels = args.fixtures / f"{base}.csv"
        for path in [wav, labels]:
            if args.download_fixtures and not path.exists():
                url = (f"https://huggingface.co/datasets/artem-tish/veche-bench/resolve/{REV}"
                       f"/data_{kind}/{name}/{name}{path.suffix}")
                with tempfile.TemporaryDirectory(dir=args.fixtures) as temp:
                    part = Path(temp) / "download"
                    subprocess.run(["curl", "-fLsS", "--retry", "2", "-o", part, url], check=True)
                    part.replace(path)
        for offset in [0, 75]:
            with tempfile.NamedTemporaryFile() as pcm, wave.open(str(wav)) as audio:
                assert (audio.getframerate(), audio.getsampwidth(), audio.getnchannels()) == (16_000, 2, 1)
                audio.setpos(offset * 16_000)
                pcm.write(audio.readframes(DURATION * 16_000))
                pcm.flush()
                voiced, perf = run_helper([bin_dir / "voai-diarize", pcm.name, args.models])
                recognized, _ = run_helper([bin_dir / "voai", "--file-worker", args.whisper, "ru", pcm.name])
            words = [word for event in recognized for word in event.get("words", [])]
            turns = [turn for e in voiced for turn in e.get("turns", [])]
            scores = report(reference(labels, offset), turns, words)
            print(json.dumps(dict(case=base, offset=offset, **scores, **perf)), flush=True)

    if args.stream_check:
        stream_regression(bin_dir / "voai-diarize", args.models, args.fixtures)


if __name__ == "__main__":
    main()
