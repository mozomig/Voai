# Using Voai

## Your first dictation

1. At first launch, choose **Set up permissions**. Allow the microphone to record speech and Accessibility to insert text in other apps. You can skip setup to transcribe files, or revisit it under **Dictation → Permissions → Quick start**.
2. Pick a speech model. Download one from the catalog or select a compatible speech model on disk.
3. Click the test field, hold **⌥ Space**, speak, then release the keys. Try the same shortcut in your editor. Press **Esc** to cancel.

To use a different shortcut, click the current one and press the keys you want. For right Command alone, press and release that key. For a letter with modifiers, press the letter while still holding the modifiers.

Under **Dictation → Interface language**, choose **System language**, **English** or Russian. System language follows your Mac’s preferred languages and uses English when none are supported. **Spoken language** controls recognition separately.

The standard widget shows a live draft. Click the copy icon beside its status to copy the full text available at that moment, including any words outside the preview. Dictation continues. The compact widget shows a waveform near the cursor, or near the mouse pointer when the editor hides the cursor position. If Voai needs your input, it opens the full panel.

Turn on noise suppression to clean up background sound for the next dictation. FastEnhancer-B runs on the Mac before speech recognition. History keeps the original microphone recording. Noise suppression is off by default.

The model selectors at the bottom of the sidebar work on every tab. Choosing a text model there turns on local editing. Turn editing off to insert the raw transcript.

## Edit dictation with a CLI

In **Editing → Cloud → Provider**, choose **Codex CLI** or **Claude Code**. Sign in to that CLI on your Mac first. Voai starts a fresh CLI process for each dictation and sends the transcript and your editing instructions to the provider. File transcription has its own model picker. Telegram uses local editing only.

Leave the executable path empty for automatic detection or enter the full path returned by `command -v codex` or `command -v claude`. Select a model, enter its ID, or leave the field empty for the CLI default. Set **Reasoning effort** or leave it at **CLI default**, then click **Save**. Each CLI remembers its path, model and effort. Support for individual levels depends on the model. If editing fails, the dictation panel lets you insert the original transcript.

## Inserting text

Voai uses macOS Accessibility and Unicode events to insert text while preserving the clipboard. If the field or selection changes before insertion, the text stays in the panel. Password fields are excluded. Some editors expose less information about the cursor and selection than others.

If Voai cannot locate the cursor in VS Code, set **Editor: Accessibility Support** to `on`. To restore automatic detection, set it back to `auto`. [VS Code accessibility settings](https://code.visualstudio.com/docs/configure/accessibility/accessibility#_screen-reader-mode)

## History

Open **History** from the sidebar or the menu bar. Click the copy icon on an entry to copy its full text, including any edits. Click **Play** to listen to the recording. Expand the entry to read the text or copy the original wording separately when it differs. If editing changed the wording, you can compare both versions. The original transcript is available even if the editor failed.

The last 20 completed dictations are stored in `~/Library/Application Support/Voai/History`. Each new entry replaces the oldest once the history is full. Deleting an entry removes its audio and text after confirmation. Empty and cancelled dictations never enter the history.

## Audio and video files

Open **Transcription** and choose a recording. The file workflow has three settings.

| Stage | What to choose |
| --- | --- |
| Speech recognition | Select Whisper, Parakeet or GigaAM from your speech models. Voai reads the audio track from a video directly. |
| Speakers | Leave speaker identification off for plain text. To separate speakers, download the model pair or open a folder containing compatible `segmentation.onnx` and `embedding.onnx` files. |
| Text editing | Select a downloaded or linked model under **On this Mac**, or scroll to the local server or cloud provider you want. Choose **No editing** to keep the original. The editor applies your saved instructions to each turn. Changed turns retain their originals. |

Voai reads the audio in chunks. Allow about 115 MB of temporary disk space per hour of audio. Voai removes the temporary track when the job finishes or you cancel it. File settings are independent of dictation settings. Speech recognition and speaker identification run on the Mac. Choosing a cloud editor sends each turn and the editing instructions to the selected provider. API keys, CLI paths and reasoning effort come from **Editing**. If speaker identification or editing fails, you can still use the recognized text.

Plain transcripts open as paragraphs. Choose **Show timestamps** to inspect segments and play the audio. With speakers enabled, the result opens as a conversation. Click a timestamp to hear a turn, reassign a single turn, or rename someone throughout the transcript. Export to TXT or SRT. Keep the source file in place for playback. Voai stores transcripts in `~/Library/Application Support/Voai/Transcripts`.

## Telegram

1. Create a bot with `/newbot` in [@BotFather](https://t.me/BotFather). In Voai, open **Telegram**, enter the token and connect.
2. Choose **Link account**. Open the link with the Telegram account you want to use and tap **Start**. Voai shows its Telegram ID.
3. Forward a voice note, video note, audio file or video to the bot. It sends the transcript back to your private chat. The bot uses the speech model from **Transcription**. Choose a GGUF editor there to edit bot transcripts. With any other editor, the bot returns the original text.

Keep Voai running and your Mac online. Telegram handles the incoming media and reply, and Voai transcribes on the Mac. The temporary file is removed after processing. The bot token is stored in macOS Keychain. The Bot API download limit is 20 MB. For larger recordings, open the file directly in Voai. [Telegram Bot API](https://core.telegram.org/bots/api#getfile)

The connection uses your Mac's network settings, including system proxies and an active VPN. You can set a local SOCKS5 port in the Telegram network settings. Voai shows temporary server errors and retries. The bot shows a typing indicator while downloading and transcribing. If a transfer is interrupted, Voai retries the download.

Unsent reply parts stay on the Mac until delivery is confirmed. Voai resumes after a restart. If Telegram may already have accepted a reply, check the chat and tell Voai whether to mark it delivered or send it again.

## Models and editing

In **Models**, choose **Speech** for Whisper GGML files or transcribe.cpp GigaAM and Parakeet GGUFs with word timestamps. GigaAM v3 E2E transcribes Russian with punctuation. Parakeet TDT v3 handles 25 European languages in one model. Under **Text**, choose a single-file llama.cpp GGUF with a chat template. The catalog includes Gemma 4 E4B and the non-thinking Qwen3 4B Instruct 2507 for local editing. Voai checks each file in a helper process and saves a reference to its location. Reconnect a file if you move or replace it. Removing the reference leaves your file on disk.

Catalog downloads live in `~/Library/Application Support/Voai/Models`. You can follow progress, cancel or resume. Voai verifies SHA-256 before using the weights.

| Editing mode | Setup |
| --- | --- |
| Off | Insert the Whisper transcript as recognized. |
| Model file | Choose a downloaded text model or a GGUF file. llama.cpp runs it on your Mac. |
| Local server | Start Ollama or LM Studio, enter the base URL, fetch the model list and select a model. If the server requires a bearer token, save it for that address. |
| Cloud | Use an OpenAI or Anthropic API key, or select Codex CLI or Claude Code with an existing login. The provider receives the editing instructions and transcript. |

Typical local URLs are `http://127.0.0.1:11434/v1` for Ollama and `http://127.0.0.1:1234/v1` for LM Studio. Voai connects to loopback addresses only in local server mode. For offline use, select a downloaded model in the server itself. [Ollama](https://docs.ollama.com/api/openai-compatibility) · [LM Studio](https://lmstudio.ai/docs/developer/openai-compat)

Edit your instructions under **Editing** and press **⌘S**. For example, try “Translate into French. Return only the translation.” Changes apply to the next dictation. You can restore the default instructions. If editing fails, the panel keeps the original text.

Settings live in `~/Library/Application Support/Voai`. API keys and server tokens are stored in Keychain. Closing the settings window leaves Voai available in the menu bar.

## Updates

Open **Voai → Check for Updates…** to check manually. The automatic check setting is under **Dictation**. You choose when to install. Voai waits for active dictation and transcription work to finish before restarting.
