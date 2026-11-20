# Speaker identification on Russian speech

These measurements were taken on November 20, 2026 on an Apple M4 Pro with 24 GiB unified memory, running arm64 macOS 26.5.2. We used three mono mixes from Veche Bench, revision `a0ab08d6887b64995d076b475a87310953a51138`, licensed under CC BY 4.0. Volunteers recorded scripted lines in Russian. The corpus authors mixed them into conversations with different gaps and overlaps and provided reference times, text and speaker labels. The two three-person mixes use the same dialogue. Accuracy on spontaneous speech from a shared room still needs a separate evaluation against a hand-labeled recording. [Veche Bench dataset](https://huggingface.co/datasets/artem-tish/veche-bench)

## Installation and offline use

The recommended pair totals 31,137,485 bytes. It includes an INT8 conversion of pyannote Segmentation 3.0 (MIT, © CNRS, 2022) and the 3D-Speaker CAM++ VoxCeleb embedding model (Apache 2.0). `src/app/diarization.rs` pins each revision, size and SHA-256. The signed helper statically links sherpa-onnx 2.0.2 (Apache 2.0). Packaging checks the hash of its arm64 native archive. Users can download the pair or select a folder with compatible `segmentation.onnx` and `embedding.onnx` files. A helper runs both networks on generated input to validate the pair. [sherpa-onnx release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v2.0.2) · [Segmentation conversion](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0) · [Original model license](https://huggingface.co/pyannote/segmentation-3.0) · [CAM++](https://modelscope.cn/models/iic/speech_campplus_sv_en_voxceleb_16k)

In a fresh profile, Voai downloaded both files, verified their hashes and opened them. With all network access denied in the macOS sandbox, Voai found the local files and validated the pair again. Swapping the two files failed validation. Cancelling before download left the directory empty. Under the same network restrictions, the signed app emitted two batches of results from a 76-second file.

## Candidate comparison

The first comparison used the first 75 seconds of each mix. Diarization error rate (DER) includes missed speech, false alarms and time attributed to the wrong speaker. We scored 10 ms frames with optimal one-to-one assignment between predicted and reference speakers. Overlaps count and there is no boundary collar. Both sherpa configurations used one CPU thread, a segmentation window shift of 0.20 and clustering threshold of 0.50. Nemotron used the Transformers implementation with four CPU threads.

| Mix | Sherpa + CAM++ · DER / labels | Sherpa + WeSpeaker ResNet34-LM · DER / labels | NVIDIA Nemotron-3 · DER / labels |
| --- | ---: | ---: | ---: |
| Two speakers | 13.99% / 2 | 14.94% / 2 | 8.77% / 2 |
| Three speakers | 26.15% / 5 | 37.99% / 2 | 7.75% / 3 |
| Three with overlap | 14.86% / 4 | 37.92% / 2 | 7.21% / 3 |

Through Python, sherpa with CAM++ processed each 75-second excerpt in 2.77–2.86 s at 140–190 MiB peak RSS. WeSpeaker took 5.23–6.04 s at 137–179 MiB. In one process, Nemotron took 2.12 s on the first excerpt and 0.53–0.59 s on the next two. Its peak RSS was 691–733 MiB. Its weights occupy 378.6 MiB and use the OpenMDW 1.1 license. Thread counts and warm-up differed between runs. Nemotron scored the lowest DER on all three excerpts. Voai ships CAM++ for its small download and native macOS runtime. Access to the official pyannote Community-1 weights required Hugging Face authorization. Those weights use PyTorch, while the current helper takes ONNX models. [NVIDIA Nemotron-3-Diarization](https://huggingface.co/nvidia/Nemotron-3-Diarization/tree/52a432672732425e4c19765b9fec9b5810e6f4ba) · [Community-1 access and usage](https://huggingface.co/pyannote/speaker-diarization-community-1)

## Speaker accuracy on recognized words

We tuned the clustering threshold to 0.75 on the first three excerpts and held out the next 75 seconds of each recording. The native implementation processed both sets using Whisper Small with Russian word timestamps. The GGML file's SHA-256 was `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b`.

The test matches identical Whisper and reference words in sequence and drops matches more than 1.5 s away from the corresponding turn. Each word is assigned to the speaker with the largest time overlap. The last three columns count matched words, those with the wrong speaker and those without a speaker. Words missed by Whisper are outside these counts.

| Mix | Excerpt | Overlap | DER | Labels / expected | Matched words | Wrong speaker | Unassigned |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Two speakers | 0–75 s | 0.00 s | 14.86% | 3 / 2 | 152 | 14 | 4 |
| Three speakers | 0–75 s | 0.00 s | 9.41% | 3 / 3 | 116 | 12 | 6 |
| Three with overlap | 0–75 s | 0.77 s | 11.49% | 3 / 3 | 140 | 17 | 3 |
| Two speakers | 75–150 s | 0.00 s | 4.00% | 2 / 2 | 157 | 3 | 6 |
| Three speakers | 75–150 s | 0.00 s | 35.64% | 3 / 3 | 123 | 32 | 9 |
| Three with overlap | 75–150 s | 4.23 s | 29.06% | 3 / 3 | 137 | 35 | 4 |

The held-out three-person conversation had the correct number of speaker labels, but 32 of 123 matched words went to the wrong person. The transcript editor lets the reader listen to a turn and reassign it.

## Streaming and memory

The Rust helper reads 75-second PCM windows with up to 5 seconds of context on each side. It emits the central intervals as JSONL and releases the audio for that window. CAM++ extracts a normalized embedding from up to 12 seconds of speech per person that does not overlap with another voice. Matching across windows uses cosine similarity and the speech intervals across the most recent boundary. The helper retains that boundary and up to 256 voice profiles. Beyond that limit, it evicts the least recently heard speaker, who may receive a new label on return.

The test recording begins with speakers A and B. C enters at 75.0 s. After a stretch of silence, A returns with different words. Each run used a fresh native process and measured peak RSS via `/usr/bin/time -l`. All runs returned the ID sequence 0, 1, 2, 0.

| A returns after | Peak RSS | Elapsed time with concurrent builds |
| --- | ---: | ---: |
| 150 s | 125.8 MiB | 57.23 s |
| 30 min | 137.2 MiB | 79.85 s |
| 90 min | 137.4 MiB | 51.37 s |

The helper skips all-zero windows. These runs exercise label continuity, timestamps and helper memory as recordings get longer. The measurements exclude Whisper, AVFoundation, the UI and disk space used for decoded PCM.

A second test repeated the same three-speaker mix with overlap and no long pauses, exercising both networks and speaker matching in each window. Increasing the file from 3 min 45 s to 11 min 15 s added 2.5 MiB of peak RSS. All three runs kept the same three global labels.

| Active audio | Peak RSS | Processing time |
| --- | ---: | ---: |
| 75 s | 147.8 MiB | 13.13 s |
| 225 s | 166.5 MiB | 68.81 s |
| 675 s | 169.0 MiB | 175.14 s |

## Reproduce the measurements

From the repository root, supply the app, the folder containing the ONNX pair, the Whisper file and a directory for Veche Bench. On the first run, the download flag fetches the authors' public WAV and CSV files. Later runs can use the local copy offline. The script reports measurements without logging the conversations.

```sh
python3 tests/macos/diarization.py target/dist/release/Voai.app \
  /path/to/onnx-pair /path/to/ggml-small.bin target/qa/interviews \
  --download-fixtures --stream-check
```
