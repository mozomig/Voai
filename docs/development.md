# Building and releasing Voai

Voai uses Rust 2024 and GPUI. The main process owns the settings window, floating dictation panel and macOS integration. Speech recognition and local text editing run in bundled helpers. [Architecture](../ARCHITECTURE.md) · [Original interface prototype](../design/index.html)

Run the commands below from the repository root.

## Build and test

Use an Apple Silicon Mac with Xcode, the macOS SDK and Metal compiler, CMake, and a stable arm64 Rust toolchain with 2024 edition support. If rustup only has an Intel toolchain installed, run `rustup toolchain install stable-aarch64-apple-darwin --force-non-host`. The build wrapper selects the native compiler and Xcode's libclang.

```sh
./scripts/cargo-macos test --workspace --locked
./scripts/cargo-macos clippy --workspace --all-targets --locked -- -D warnings
./scripts/cargo-macos fmt --all --check
./scripts/package-macos
./scripts/verify-macos-release
open target/dist/release/Voai.app
```

For a quicker build with debug symbols, use `./scripts/package-macos debug`.

### Dictation and insertion

To try insertion by itself, run `target/dist/release/Voai.app/Contents/MacOS/voai probe-input` and select text in a compatible field during the countdown. The probe replaces the selection with a test phrase.

For an interactive AppKit and WebKit test, run the following command and keep its temporary test window active. It checks replacing a selection, changing the field or cursor position, rejecting secure fields and preserving the pasteboard change count.

```sh
swift tests/macos/input.swift "$PWD/target/dist/release/Voai.app/Contents/MacOS/voai"
```

The Chrome test launches the installed Google Chrome with a temporary profile and checks the DOM after insertion.

```sh
python3 tests/macos/chromium.py "$PWD/target/dist/release/Voai.app/Contents/MacOS/voai"
```

Use `crates/voai-speech/tests/worker_smoke.py` to check recognition on a recording. It takes the executable, Whisper weights, a mono 16 kHz PCM16 WAV, a language code and an expected phrase.

The dictation acceptance test uses a synthetic recording in place of a microphone. It checks live drafts, recognition on key release, translation with the supplied prompt, delivery to the test field, cancellation and recovery of captured audio when input stalls. The script includes commands for synthesizing the Russian fixture. [Dictation test](../tests/macos/dictation.py)

```sh
python3 tests/macos/dictation.py target/dist/release/Voai.app tiny.bin Qwen.gguf ru.wav
```

### File transcription

To test a synthetic hour-long video, run the following command with ffmpeg and the Milena system voice installed.

```sh
python3 tests/macos/file_transcription.py target/dist/release/Voai.app /path/to/Whisper-Small.bin
```

To test the complete pipeline on your own recording, set absolute paths in `VOAI_FILE_QA_APP`, `VOAI_FILE_QA_MEDIA` and `VOAI_FILE_QA_WHISPER`. Optionally set `VOAI_FILE_QA_VOICES` and `VOAI_FILE_QA_EDITOR`. The test writes to a temporary library and reports stages, elapsed time and counts. Speaker-identification results and memory measurements are in the [diarization evaluation](../tests/macos/diarization.md).

```sh
./scripts/cargo-macos test -p voai imports_media_with_packaged_workers -- --ignored --nocapture
```

### Signing local builds

Print the available identities with `security find-identity -v -p codesigning`. Pick an Apple Development certificate, then run `git config --local voai.signingIdentity FINGERPRINT`. Reusing the identity helps macOS recognize subsequent builds when checking permissions. Without an identity the script uses ad hoc signing, and a rebuild may need new permission grants.

When switching from ad hoc signing to a certificate, remove the old Voai entry from **System Settings → Privacy & Security → Accessibility**. Add the freshly signed app with **+**. macOS manages microphone access separately.

## Publish a release

On a new clone, copy your GitHub noreply address from **GitHub → Settings → Emails** and run `./scripts/git-email setup YOUR_NOREPLY_ADDRESS`. This sets the address for the current checkout and enables the commit and push hooks. The hooks check author, committer and annotated tagger addresses in the history being published.

The update feed uses the repository from the GitHub `origin` remote. For a separate release repository, set `VOAI_RELEASE_REPOSITORY=owner/repo`. A local build with no repository configured skips update checks.

1. Bump the workspace version, update `Cargo.lock`, set the same short version in `packaging/Info.plist`, and increment `CFBundleVersion`. Sparkle uses the build number to compare releases.
2. Set up a **Developer ID Application** certificate, a `notarytool` Keychain profile and the Sparkle signing key on the release Mac. The public key is in `packaging/sparkle-public-key.txt`. The private key is stored in Keychain under the Sparkle account `app.voai.desktop`. For a fork, generate a key with `"$(./scripts/sparkle-macos)/bin/generate_keys" --account YOUR_ACCOUNT` and set `VOAI_SPARKLE_ACCOUNT` and `VOAI_SPARKLE_PUBLIC_KEY`.
3. Run the release script with your signing identity and notary profile. Optionally set `VOAI_RELEASE_NOTES` to a Markdown file. The script submits the app and DMG for notarization, staples the tickets and verifies the packages.

```sh
VOAI_SIGN_IDENTITY='Developer ID Application: …' \
VOAI_NOTARY_PROFILE='voai-notary' \
./scripts/release-macos
```

4. Create a draft GitHub Release tagged `v<version>`. Attach `Voai-<version>-arm64.dmg`, `Voai.zip` and `appcast.xml` from `target/dist/v<version>/`. Publish it as **Latest**. Sparkle checks `releases/latest/download/appcast.xml`, then downloads the DMG from the versioned tag. If the DMG changes, regenerate the signatures and feed.

For a local rehearsal with a development certificate, run `./scripts/package-macos`, `./scripts/make-appcast` and `./scripts/verify-macos-release path/to/image.dmg`. The artifacts are written to `target/dist/release/`. [Publishing with Sparkle](https://sparkle-project.org/documentation/publishing/)
