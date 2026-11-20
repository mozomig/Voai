# Сборка и выпуск Voai

Voai написана на Rust 2024. Окно настроек и виджет диктовки используют GPUI. Как связаны интерфейс, речевой процесс и обработчики файлов, описано в [архитектуре проекта](../ARCHITECTURE.md). В репозитории также сохранился [первый макет интерфейса](../design/index.html).

Выполняйте команды из корня репозитория.

## Сборка и проверка

Сборка рассчитана на Apple Silicon. Нужны полный Xcode с SDK macOS и компилятором Metal, CMake и нативный стабильный Rust с поддержкой edition 2024. На Mac, где установлен только Intel Rust через rustup, добавьте нативный toolchain командой `rustup toolchain install stable-aarch64-apple-darwin --force-non-host`. Сценарий сборки сам выбирает нативный компилятор и libclang из Xcode.

```sh
./scripts/cargo-macos test --workspace --locked
./scripts/cargo-macos clippy --workspace --all-targets --locked -- -D warnings
./scripts/cargo-macos fmt --all --check
./scripts/package-macos
./scripts/verify-macos-release
open target/dist/release/Voai.app
```

Файловый режим можно проверить на синтетическом часовом видео с помощью `python3 tests/macos/file_transcription.py target/dist/release/Voai.app /путь/к/Whisper-Small.bin`. Для этого теста нужны ffmpeg и системный голос Milena. Для проверки всего конвейера на собственной записи укажите абсолютные пути в `VOAI_FILE_QA_APP`, `VOAI_FILE_QA_MEDIA`, `VOAI_FILE_QA_WHISPER` и, по желанию, `VOAI_FILE_QA_VOICES` и `VOAI_FILE_QA_EDITOR`. Запустите `./scripts/cargo-macos test -p voai imports_media_with_packaged_workers -- --ignored --nocapture`. Тест сохраняет результат во временной библиотеке и выводит только этапы, длительность и счётчики. Замеры качества спикеров, памяти и условия сравнения приведены в [отчёте по диаризации](../tests/macos/diarization.md).

Для быстрой сборки с отладочными символами используйте `./scripts/package-macos debug`. Проверить только интеграцию с текстовым полем можно командой `Voai.app/Contents/MacOS/voai probe-input`. Она заменяет выделение тестовой фразой через шесть секунд после запуска, если выбранное поле поддерживает ввод.

В интерактивном сеансе macOS можно проверить ввод из собранного приложения в одноразовое окно AppKit и поле WebKit. Тест проверяет замену выделения, смену поля и позиции курсора, защищённый ввод и счётчик изменений буфера обмена. Во время проверки тестовое окно должно оставаться активным.

```sh
swift tests/macos/input.swift "$PWD/target/dist/release/Voai.app/Contents/MacOS/voai"
```

Отдельный тест запускает установленный Google Chrome с временным профилем и проверяет результат в DOM. Команда для него — `python3 tests/macos/chromium.py "$PWD/target/dist/release/Voai.app/Contents/MacOS/voai"`.

Для проверки распознавания на собственной записи используйте `crates/voai-speech/tests/worker_smoke.py`. Она принимает путь к исполняемому файлу, весам Whisper, WAV (16 кГц, моно, PCM16), код языка и ожидаемый фрагмент фразы.

Для сквозного теста без микрофона запустите `python3 tests/macos/dictation.py target/dist/release/Voai.app tiny.bin Qwen.gguf ru.wav`. Скрипт проверяет черновик до отпускания клавиш, финальную расшифровку, перевод по заданной инструкции, доставку в обработчик пробного поля, отмену второй фразы и сохранение принятой речи при остановке аудиопотока. В начале [скрипта](../tests/macos/dictation.py) есть команда для синтеза тестовой записи.

Для локальной разработки выберите свой сертификат Apple Development из вывода `security find-identity -v -p codesigning` и сохраните его отпечаток командой `git config --local voai.signingIdentity ОТПЕЧАТОК`. У следующих сборок сохранится идентичность приложения для разрешений macOS. Без выбранного сертификата сценарий использует ad hoc, и разрешение может устареть при пересборке.

При переходе с ad hoc на сертификат удалите прежнюю запись Voai в **Системные настройки → Конфиденциальность и безопасность → Универсальный доступ** и добавьте актуальную `Voai.app` кнопкой **+**. macOS отдельно управляет доступом к микрофону. Для распространения задайте переменную `VOAI_SIGN_IDENTITY` с именем сертификата Developer ID Application. Передача профиля Keychain в `VOAI_NOTARY_PROFILE` включает отправку в Apple, ожидание нотарификации и прикрепление билета. Архив и пакет появляются рядом в `target/dist/release/`.

## Подготовка публичного релиза

При первом клонировании скопируйте свой адрес `@users.noreply.github.com` из **GitHub → Settings → Emails** и выполните `./scripts/git-email setup ВАШ_АДРЕС`. Команда задаёт почту только для этого репозитория и подключает хуки. Перед коммитом Git проверит адреса автора и коммитера, перед пушем — историю отправляемых веток и авторство тегов.

Сборка берёт адрес репозитория из GitHub-remote `origin`. Для отдельного репозитория релизов задайте `VOAI_RELEASE_REPOSITORY=owner/repo`. Без адреса обычная локальная сборка запускается с отключённой проверкой обновлений.

1. Увеличьте версию workspace и обновите `Cargo.lock`. В `packaging/Info.plist` задайте ту же версию и увеличьте `CFBundleVersion`. Sparkle сравнивает номера сборок.
2. На Mac для выпуска нужен сертификат **Developer ID Application**, профиль `notarytool` и ключ Sparkle в Связке ключей. Официальный публичный ключ находится в `packaging/sparkle-public-key.txt`. Закрытый ключ хранится в аккаунте Sparkle `app.voai.desktop`. Для своего форка создайте отдельный ключ через `"$(./scripts/sparkle-macos)/bin/generate_keys" --account ИМЯ`, задайте `VOAI_SPARKLE_ACCOUNT` и `VOAI_SPARKLE_PUBLIC_KEY`.
3. Запустите сборку. Для описания изменений можно передать путь к Markdown-файлу в `VOAI_RELEASE_NOTES`.

```sh
VOAI_SIGN_IDENTITY='Developer ID Application: …' \
VOAI_NOTARY_PROFILE='voai-notary' \
./scripts/release-macos
```

4. Создайте черновик GitHub Release с тегом `v<версия>`. Из `target/dist/v<версия>/` приложите **Voai-<версия>-arm64.dmg**, **Voai.zip** и **appcast.xml**, затем опубликуйте как **Latest**. Sparkle загружает подписанную ленту по постоянному адресу `releases/latest/download/appcast.xml`, а DMG — по ссылке на конкретный тег. При замене DMG соберите подписи и ленту заново.

Для локальной репетиции с сертификатом разработки доступны `./scripts/package-macos`, `./scripts/make-appcast` и `./scripts/verify-macos-release путь/к/образу.dmg`. Скрипты подготовки оставляют файлы на Mac. [Документация Sparkle](https://sparkle-project.org/documentation/publishing/)
