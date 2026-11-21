# Voai for macOS

## Process boundaries

The main process owns the GPUI windows, macOS menus, global shortcut and microphone permissions. Speech recognition and local text editing run in child processes from the same signed bundle. The active utterance stays in memory. Settings, verified model files and completed history entries persist across launches. Missing fields in older settings files receive defaults.

| Location | Responsibility |
| --- | --- |
| `src/ui/` | Settings, test field, floating panel and menu bar item. The UI dispatches commands and renders snapshots. |
| `src/app/` | Hotkeys, settings, downloads and dictation sessions. Each session owns its response channel and cancellation flag. |
| `src/app/history.rs`, `src/app/history/store.rs` | A dedicated thread owns the SQLite history and AVAudioPlayer playback. UI snapshots carry text and metadata. Audio is loaded for playback. |
| `src/macos.rs`, `src/insertion.rs` | Permissions, capture of the target application, window, field and selection, and revalidation before a single insertion attempt. |
| `crates/voai-speech/` | Core Audio HAL input discovery, CPAL capture, bounded buffering and conversion to mono 16 kHz PCM. Optional FastEnhancer-B feeds cleaned frames into partial and final recognition. Releasing the shortcut flushes the filter tail. |
| `voai --speech-worker` | Whisper GGML runs in whisper.cpp. For speech GGUF, the process replaces itself with the sibling `voai-asr-helper` (transcribe.cpp with Metal), retaining the PID and cancellation semantics. Each engine links its own ggml. The `voai-asr-worker` crate owns the shared PCM protocol and timestamped file window stitching. Drafts use the most recent 12 seconds. Final recognition uses the full recording up to five minutes. The process can stay warm for up to 90 seconds. |
| `crates/voai-l10n/` | English and Russian catalogs compiled with rust-i18n. The System preference follows the first supported language in the macOS list. Each worker receives the resolved interface language at launch, separately from the language passed to Whisper. |
| `crates/voai-models/` | Model catalog, resumable downloads with SHA-256 validation, checks for local files, text editing adapters and Keychain access. |
| `crates/voai-text-helper/` | llama.cpp with Metal. The local editor receives text and returns results over pipes. |
| `scripts/package-macos` | Builds for arm64, checks dynamic dependencies, signs the app and its helpers, and optionally notarizes the package. |

Pressing the dictation shortcut captures the target and settings. Releasing it finishes recognition, applies the selected editing mode and rechecks focus. A changed target requires the user to select where to insert the phrase. An editor failure preserves the original transcript. An ambiguous write always requires user review.

AppKit fields use `AXSelectedText`. When the setter reports a generic failure or a messaging failure, Voai checks the original field for the exact replacement and a collapsed caret at its end. An unconfirmed edit stays available for review. For supported web fields with an accessible selection range, Voai sends targeted Unicode events through Core Graphics and checks the inserted range. It waits up to four seconds for the Accessibility tree to catch up, stopping as soon as the write is confirmed. The panel stays responsive. Chrome may report the updated range late. WKWebView can acknowledge an `AXSelectedText` write while its DOM retains the old value. The AppKit and WebKit regression test covers both insertion and pasteboard preservation. Secure fields, editors with no accessible caret and multiline text requiring keyboard events may need a different target. [Insertion test](tests/macos/input.swift)

### Interface language

The `voai-l10n` crate holds English and Russian message catalogs shared by the desktop app and its workers. `InterfaceLanguage` stores either an explicit language or a preference to follow macOS. In system mode, Voai uses the first supported language in the preferred language list, falling back to English. Each child process receives the language when it starts. The language used for speech recognition is a separate setting.

### Audio and video files

The transcription screen dispatches commands to `app::files::Files`. Starting an import snapshots the source path, model choices for each stage, speech language and editing instructions. The file pipeline's own settings and documents live in `Transcripts/library.sqlite3`. Each import has a response channel. Cancelling closes it and stops the child process. A completed document is published in a single SQLite write.

1. The signed `voai-media-helper` uses AVFoundation to decode a local audio track into a private temporary 16 kHz PCM file. It also plays a time interval in the original recording.
2. `voai --file-worker` reads overlapping windows of up to 30 seconds and returns words with absolute timestamps. A separate protocol lets file imports outlast short dictation sessions.
3. Optional `voai-diarize` uses sherpa-onnx to extract speaker intervals in 75-second windows with up to five seconds of context on each side. It keeps voice embeddings and the intervals at the most recent join to match speakers across windows. `files::document` assigns words to intervals and marks uncertain turns. User-provided weights are validated outside the UI process.
4. Optional `voai-text-helper --edit-lines-v1` loads a GGUF model once per file and edits one turn at a time. Changed turns keep their originals. File imports can instead use a loopback server or an explicitly selected cloud API or CLI. Each request contains one turn and the editing instructions. The Telegram pipeline accepts only the GGUF editor.

Each document owns its speaker IDs. Renaming a speaker updates the document's name map. Reassigning a turn affects that turn alone. Before playback, Voai checks that the source file still matches its recorded identity. TXT and SRT exports preserve original timestamps and use a temporary file followed by rename.

### Telegram

The optional adapter in `src/app/telegram` receives Bot API updates through HTTPS long polling. Bot API requests and media downloads use an ephemeral macOS URLSession with system VPN and proxy settings. If the user specifies SOCKS5, the session uses only the configured port on 127.0.0.1 and disables direct fallback. The delegate rejects redirects and limits response bytes before writing. Stopping the bot cancels active network tasks.

Voai checks the token with `getMe`. A one-time link pairs one private chat with that bot ID. The token has its own Keychain entry. `telegram.json` stores the Keychain account identifier, bot and owner IDs, proxy port if configured, and next update offset.

Incoming files are processed one at a time. The adapter enforces a streaming size limit, creates a temporary local copy and asks the backend for the current file transcription settings. It reuses the speech worker and optional local editor from file imports. It replies with paragraphs in the originating chat and removes the temporary directory. The background worker owns the state file and talks to Keychain. It retries a transient download failure up to four times, respecting Telegram's requested delay.

Before sending a reply, a single atomic write saves the update offset and remaining reply parts. Before each network call the worker records its delivery attempt. Confirmed parts are removed. After a restart Voai verifies the bot's identity and continues delivery in the paired chat. When Telegram may already have accepted a part, the user checks the chat and decides whether to confirm or resend it. Disconnecting cancels processing, waits for the old worker in the background and removes the connection and pending text.

### Models and local servers

Catalog weights download into Voai's own model directory. Models supplied by the user are referenced by canonical path. A separate process checks the container and tries loading the model with the corresponding engine. Voai records the type, file identity and modification time. Reconnecting validates the replacement. Removing a connection leaves the weights on disk.

An utterance uses the editing instructions captured when dictation starts. The local helper receives the instructions and transcript in separate protocol messages. HTTP adapters send separate system and user messages. A failed edit leaves the original available.

Local servers use an OpenAI-compatible API on loopback. The client discovers models with `GET /v1/models` and edits text through `POST /v1/chat/completions`. Requests run in the background with proxies and redirects disabled. Each Keychain token is scoped to a server address. The server controls its loaded models, resources and any subsequent forwarding of data. [Ollama API](https://docs.ollama.com/api/openai-compatibility) · [LM Studio API](https://lmstudio.ai/docs/developer/openai-compat)

### macOS distribution

Voai ships as a standalone macOS app. Writing into another app's focused field requires user-authorized Accessibility APIs. Apple lists this use of Accessibility as incompatible with the App Sandbox required for Mac App Store distribution. MacWhisper also documents a store limitation for system-wide dictation. Its help page does not specify which API it uses to insert text. [Apple's sandbox documentation](https://developer.apple.com/documentation/security/protecting-user-data-with-app-sandbox) · [MacWhisper dictation](https://docs.macwhisper.com/article/14-how-to-use-the-dictation-feature)

## Original design and acceptance criteria

These were the design goals before implementation. Hardware targets, future measurements and proposed recovery behavior below describe the original plan. [Original interface brief](./opendesign-prompt.txt)

### Ownership

Ship one app. The main process owns the UI, microphone, access to other applications and the dictation lifecycle. Isolate native model code in helpers so an editing engine crash leaves the transcript and recovery panel available.

| Module | Owns | Interface |
| --- | --- | --- |
| Session | Active utterance, frozen settings and valid state transitions | User commands and worker events in, UI snapshot out |
| macOS | Hotkeys, input permissions, target, panel window and menu | Start and stop capture, capture a target reference, attempt insertion |
| Speech | Audio preparation and local recognition | Drafts, final transcript and failures |
| Processing | Local or cloud editing, timeouts and cancellation | Edited text or a failure with the original preserved |
| Models | Catalog, downloads, file verification and installed versions | Download, select, release and follow progress |
| UI | GPUI panel, menu and settings | Render current state and dispatch available actions |

The proposed Cargo boundaries followed build requirements. Session logic could be tested without GPUI, AppKit or model weights. The application would compose the adapters. Each model helper would build its own native engine, allowing independent version and build settings. Ordinary Rust modules would handle internal logic. External operations such as writing to another app's field would provide a test seam.

### One utterance

A single ordered event handler owns a session. Audio capture and inference run separately. Starting records the session ID, target field, available selection range, speech model version and editing settings. Changes in Settings take effect on the next utterance.

| Event | Expected behavior |
| --- | --- |
| First key press | Start capture and show the panel over the originating app. Ignore key repeat. If a model or permission is missing, show the relevant setup action. |
| Incoming audio | Update the level meter and replace the draft as recognition results arrive. |
| Release | Close capture, drain samples and finish recognition. Silence or an empty result closes the panel without inserting text. |
| Transcript ready | Keep the original in memory. Deliver immediately with editing off, otherwise hand it to the selected editor. |
| Final text ready | Recheck the target and attempt insertion once. If focus changed, retain the text until the user selects a target. |
| Escape, sleep or lost microphone | Escape before insertion cancels and releases resources. Sleep ends capture even if the key-up was lost. A device failure lets the user finish with the captured audio or cancel. |

Each background response belongs to a session. A retry also receives a new operation identity. The handler accepts responses only for the expected operation and phase, including late cloud replies, drafts arriving after the final transcript and duplicate releases. Pressing the shortcut during completion or recovery shows the current phrase. There is no hidden queue of dictations.

### Capture and live recognition

The first engine candidate was whisper.cpp with Metal and multilingual weights. Core ML could be measured separately on the same recordings. The upstream streaming example periodically decodes a sliding window. [whisper.cpp](https://github.com/ggml-org/whisper.cpp) · [Streaming example](https://github.com/ggml-org/whisper.cpp/blob/master/examples/stream/README.md)

The microphone callback copies samples into a bounded, preallocated ring buffer. Another thread converts them to mono 16 kHz PCM, holds the current utterance in memory and feeds the speech worker. The callback must avoid waiting on the disk, engine or UI. Under load, drop stale draft updates first. If samples are lost, report an interrupted recording. Use measured memory to bound recording time and warn near the limit.

A draft has a revision and may revise the last words. The prototype repeatedly decodes overlapping audio. On release the final transcript uses all captured samples. Measure final decoding time on long phrases. If it becomes too slow, retain completed timestamped segments and finish only the tail. Test for duplicate or missing words at joins. The UI should receive the same event types in both cases.

Launch the helper from inside the bundle. Use bounded, versioned messages over inherited pipes for model loading, start, PCM, finish and cancellation. A warm process can serve successive dictations and exits when its pipes close. Bound waits for hung helpers. The design called for terminating a stuck helper and offering a retry with retained audio. Load the local text model only when editing is enabled. Measure the cost of keeping both models resident on lower-memory Macs.

### Text delivery

Capture the application, window, AX element and selection. Validate them again before delivery. Reject secure fields and suspend automatic delivery when the target is uncertain. The panel should let the user choose a compatible field, and clicking its cancel button should preserve the editor's focus. The first prototype was intended to test the nonactivating AppKit window with GPUI.

Prefer writable `AXSelectedText`. For validated fields where it is unavailable, consider Core Graphics Unicode events. Editors vary in their handling of keyboard input. Test Cyrillic, emoji, selection replacement and line breaks in chat clients and terminals. Both mechanisms preserve the clipboard. [Selection attribute](https://developer.apple.com/documentation/applicationservices/kaxselectedtextattribute) · [Writing an AX attribute](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue) · [Unicode keyboard events](https://developer.apple.com/documentation/coregraphics/cgevent/keyboardsetunicodestring%28stringlength%3Aunicodestring%3A%29)

Keep three outcomes distinct. A target can be rejected before a write, a write can be confirmed, or an attempted write can have an unknown result. In the last case, keep the phrase and ask the user to check the field. Never retry automatically or switch insertion methods, since some or all of the phrase may already have arrived. Focus can change between validation and delivery. Document compatibility per app and field, and verify the field contents after sending an event.

### Models, editing and storage

- Ship a catalog with each model's purpose, engine, version, URL, size, SHA-256 and license. Download into a temporary directory on the same volume and atomically rename it after verification. Keep installation and model selection separate.
- Run one download job per model version. Mark interrupted jobs after a restart. Resume only when the remote file identity and range support are confirmed. Exercise disk-full, checksum mismatch and exit-before-rename cases.
- Give each active session a reference to its installed version. The design deferred removal until the engine released that reference. On startup, verify selected files and offer reinstallation for missing weights.
- Use a bundled llama.cpp helper for GGUF editing. Cloud requests contain the final transcript and editing prompt only in explicit cloud mode. On timeout, empty output or a local engine crash, keep the raw text available for insertion. [llama.cpp](https://github.com/ggml-org/llama.cpp)
- Keep models and versioned settings in Application Support, write settings atomically and test migrations. Store API keys in Keychain. After recognition and optional editing, commit the audio, original and edit once to history. One SQLite transaction inserts the entry and trims old rows in `History/entries.sqlite3`. Cancellation discards drafts and recovery must not duplicate an entry.
- Keep diagnostics to versions, model identity, stage durations and error codes. Exclude text, audio, window names, keys and request bodies. Measure capture start, first draft, release to final text and editing separately.

### Validation and shipping

1. **Insertion compatibility.** Use a signed minimal app to capture press and release, show a panel and insert a fixed phrase. Test TextEdit, a browser, an Electron editor and a real day-to-day app. Switch fields within one window, try secure fields and full screen, click the panel and revoke permissions. Verify that the pasteboard change count stays constant and no message is sent. Record the app, field, insertion method and limits.
2. **Dictation latency and accuracy.** Compare models on identical short and long Russian and English samples on the least capable supported Mac. Measure word and name errors, p50/p95 time to first draft and time from release to insertion, peak memory and cold versus warm starts. The prototype goal for a warm model and a five-second utterance was one second to first draft and one second from release to insertion with editing off. Select minimum hardware, macOS and default model from the results.
3. **End-to-end behavior.** Verify cancellation and recovery for local dictation, then exercise model management and both editing paths. The plan called for CI session tests with fake clocks and adapters covering late answers and uncertain delivery. Exercise helper crashes and model installation in a temporary filesystem. Before release, repeat end-to-end tests in an interactive macOS user session.
4. **Reproducible packaging.** Pin GPUI, the workspace lockfile and native engines. Build arm64 from the tag, sign nested executables and the app with Developer ID, notarize and staple. Test the downloaded package and permission prompts on a clean Mac, offline use with installed weights and upgrading a previous version with its settings intact. [GPUI](https://gpui.rs/) · [Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
