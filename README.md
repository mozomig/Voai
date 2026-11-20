# Voai

Hold **⌥ Space** and speak. Release the keys and Voai types the words at your cursor, leaving your clipboard alone. Voai runs on **macOS 14 or later on Apple Silicon**.

![Voai on Mac, with the compact dictation widget and a transcript showing two speakers and timestamps](docs/images/voai.png)

## What you can do

- **Dictate into a text field.** Watch the words appear in a floating panel, or switch to a small waveform above the field. You can assign the shortcut to a single key, including right Command. History keeps the audio and text from your last 20 dictations.
- **Transcribe an interview or meeting.** Open an audio or video file to get a readable transcript. Turn on speaker identification for a timestamped conversation. Click a timestamp to hear the original, correct a speaker's name, then export plain text or SRT subtitles.
- **Read a Telegram voice message.** Forward a voice note, audio file or video to your own bot. While Voai is running on your Mac, the bot replies with the transcript in your private chat.

## Where your speech goes

Whisper transcribes on your Mac using Metal. Download a model in Voai or point it at one you already have. Once the models are on disk, dictation and file transcription work offline.

Text editing is optional. For dictation, use a GGUF model, a local Ollama or LM Studio server, or your own OpenAI or Anthropic API key. In cloud mode, the selected provider receives your instructions and transcript. File and Telegram transcripts use local models for editing.

## Try it

Download the DMG from [GitHub Releases](https://github.com/mozomig/Voai/releases), open it and drag Voai to Applications. Version 1.6.0 is distributed without Apple notarization. If macOS blocks the first launch, you can review Voai in **System Settings → Privacy & Security** after trying to open it. Voai checks for signed updates through Sparkle.

To build from source, you'll need Xcode with the Metal toolchain, CMake and a stable Rust toolchain that supports the 2024 edition.

```sh
git clone https://github.com/mozomig/Voai.git
cd Voai
./scripts/package-macos
open target/dist/release/Voai.app
```

The quick start walks you through microphone and Accessibility permissions, then opens the speech model picker. Try a phrase in the test field before switching to your editor. Voai follows your Mac's preferred language and lets you select English or Russian in settings. You can choose the language you dictate in separately.

[User guide](docs/usage.md) · [Build and release](docs/development.md) · [Architecture](ARCHITECTURE.md) · [Releases](https://github.com/mozomig/Voai/releases)
