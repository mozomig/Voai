# Voai

Voai is a macOS dictation application. Use Rust edition 2024 and the workspace lockfile. The native settings and floating panel are implemented in GPUI. The 1.0 interface uses the GitHub Dark palette and compact controls. design/index.html records the original 0.1 prototype.

- Keep session transitions in src/app/session.rs. UI code dispatches commands and renders snapshots. Each session owns its worker responses. Cancelling a session must invalidate pending results.
- Keep macOS Accessibility writes in src/macos.rs. Recheck the field, window and selection before delivery. Automatic insertion must preserve the clipboard, never send Return, and never retry an ambiguous write. The Copy buttons in history and the dictation panel write to the clipboard only on an explicit user action.
- Speech runs locally. Only the configured postprocessor receives a dictation transcript. The opt-in Telegram bot may return transcripts of Telegram attachments only to the paired private chat. Keep its inference and optional editing local. Local HTTP servers must use loopback without redirects or a proxy. External providers require explicit cloud mode. Keep text, audio and credentials out of logs. Secret storage belongs in Keychain.
- Build and test on Apple Silicon with scripts/cargo-macos. Run workspace tests, Clippy with warnings denied and cargo fmt --all --check. Use scripts/package-macos to verify the executable and helper together as a signed app bundle.

## Commit messages

- Before committing from a new clone, run `./scripts/git-email setup YOUR_GITHUB_NOREPLY_ADDRESS`. Use a GitHub noreply identity for authors, committers and annotated tags.

- Follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/). Use `type: summary` or `type(scope): summary`. Include a scope when it helps identify the affected area, as in `fix(hotkeys): recognize right Command on release`.
- Use `feat` for new functionality, `fix` for bug fixes, and a descriptive type such as `docs`, `refactor`, `test`, `perf`, `build`, `ci`, or `chore` for other changes.
- Write a concise English summary in the imperative mood without a trailing period. If the reason for the change needs explanation, add a body after a blank line. Mark breaking changes with `!` before the colon or a `BREAKING CHANGE:` footer.
- Never credit an AI model or coding agent as a co-author. Omit model-generated attribution trailers such as `Co-authored-by: Codex <…>` and `Generated-by: …`.
