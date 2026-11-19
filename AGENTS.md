# Voai

Voai is a macOS dictation application. Use Rust edition 2024 and the workspace lockfile. The native settings and floating panel are implemented in GPUI. The 1.0 interface uses the GitHub Dark palette and compact controls. design/index.html records the original 0.1 prototype.

- Keep session transitions in src/app/session.rs. UI code dispatches commands and renders snapshots. Each session owns its worker responses. Cancelling a session must invalidate pending results.
- Keep macOS Accessibility writes in src/macos.rs. Recheck the field, window and selection before delivery. Never touch the clipboard, send Return, or retry an ambiguous write automatically.
- Speech runs locally. Only the configured postprocessor receives a transcript. Local HTTP servers must use loopback without redirects or a proxy. External providers require explicit cloud mode. Keep text, audio and credentials out of logs. Secret storage belongs in Keychain.
- Build and test on Apple Silicon with scripts/cargo-macos. Run workspace tests, Clippy with warnings denied and cargo fmt --all --check. Use scripts/package-macos to verify the executable and helper together as a signed app bundle.
