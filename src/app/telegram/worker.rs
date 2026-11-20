use super::{
    Event, Signals,
    api::{Api, Error, MAX_BYTES, Message},
    state::{self, State},
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const READY: &str = "Жду запись.";
const PAIR: &str = "Откройте бота и нажмите «Старт».";
const HELP: &str = "Пришлите голосовое, аудио, видео или видеосообщение. Я отвечу расшифровкой.";
const TOO_BIG: &str =
    "Файл больше 20 МБ. Сохраните его на Mac и откройте в разделе «Расшифровка» в Voai.";
const UNREADABLE: &str = "Не удалось расшифровать запись. Проверьте модель и формат файла в Voai.";

fn wait(signals: &Signals, delay: Duration) -> Result<(), String> {
    let started = Instant::now();
    while signals.active() {
        let Some(left) = delay.checked_sub(started.elapsed()) else {
            return Ok(());
        };
        thread::park_timeout(left);
    }
    Err("Остановлено.".into())
}

fn connected<T>(
    signals: &Signals,
    ready: &str,
    mut operation: impl FnMut() -> Result<T, Error>,
) -> Result<T, String> {
    let mut delay = Duration::from_secs(1);
    loop {
        if !signals.active() {
            return Err("Остановлено.".into());
        }
        match operation() {
            Ok(value) => {
                if delay > Duration::from_secs(1) {
                    signals.status(ready)?;
                }
                return Ok(value);
            }
            Err(error) if error.fatal() => return Err(error.to_string()),
            Err(error) => {
                signals.status(format!("{error} Повторю подключение."))?;
                wait(signals, delay.max(error.retry_delay()))?;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
    }
}

pub(super) fn serve(
    signals: &Signals,
    api: &Api,
    mut state: State,
    transcribe: impl Fn(PathBuf) -> Result<String, String>,
) -> Result<(), String> {
    let (bot, username) = connected(signals, READY, || api.me())?;
    if state.bot.is_some_and(|known| known != bot) {
        return Err("Токен принадлежит другому боту. Подключите его заново.".into());
    }
    state.bot = Some(bot);
    signals.request(|tx| Event::Checkpoint(state.clone(), tx))?;
    let secret = Uuid::new_v4().simple().to_string();
    let link = state
        .owner
        .is_none()
        .then(|| format!("https://t.me/{username}?start={secret}"));
    signals.event(Event::Identity { username, link })?;
    signals.status(if state.owner.is_some() { READY } else { PAIR })?;
    let pair_command = format!("/start {secret}");
    loop {
        let offset = state.offset(state::now());
        let ready = if state.owner.is_some() { READY } else { PAIR };
        let updates = connected(signals, ready, || api.updates(offset))?;
        for update in updates {
            if !signals.active() {
                return Ok(());
            }
            if update.id < state.offset(state::now()) {
                continue;
            }
            let reply = match update.message {
                Some(message) if state.owner == Some(message.chat) => {
                    let text = response(signals, api, &message, &transcribe)?;
                    Some((message.chat, message.id, text))
                }
                Some(message)
                    if state.owner.is_none() && message.text.as_deref() == Some(&pair_command) =>
                {
                    state.owner = Some(message.chat);
                    signals.status(READY)?;
                    Some((message.chat, message.id, HELP.into()))
                }
                _ => None,
            };
            // Checkpoint after inference but before sending. If Telegram accepts a reply
            // and loses the connection, restarting must not send the same text again.
            state.advance(update.id, state::now());
            if let Some((chat, id, text)) = reply {
                signals.request(|tx| Event::Checkpoint(state.clone(), tx))?;
                'parts: for (index, part) in chunks(&text).enumerate() {
                    if index > 0 {
                        wait(signals, Duration::from_secs(1))?;
                    }
                    loop {
                        if !signals.active() {
                            return Ok(());
                        }
                        match api.send(chat, id, part) {
                            Ok(()) => break,
                            Err(error @ Error::RateLimited(_)) => {
                                wait(signals, error.retry_delay())?
                            }
                            Err(error) if error.fatal() => return Err(error.to_string()),
                            Err(_) => {
                                signals.status(
                                    "Не удалось доставить ответ. Перешлите запись ещё раз.",
                                )?;
                                break 'parts;
                            }
                        }
                    }
                }
            }
        }
    }
}

fn response(
    signals: &Signals,
    api: &Api,
    message: &Message,
    transcribe: &impl Fn(PathBuf) -> Result<String, String>,
) -> Result<String, String> {
    let Some(media) = &message.media else {
        return Ok(HELP.into());
    };
    if media.size.is_some_and(|size| size > MAX_BYTES) {
        return Ok(TOO_BIG.into());
    }
    signals.status("Загружаю запись…")?;
    let dir = tempfile::tempdir().map_err(|_| "Не удалось создать временную папку.")?;
    let file = dir.path().join(format!("media.{}", media.extension));
    if let Err(error) = api.download(&media.id, &file, &signals.cancelled) {
        if error.fatal() {
            return Err(error.to_string());
        }
        signals.status(error.to_string())?;
        return Ok(error.to_string());
    }
    signals.status("Распознаю речь на Mac…")?;
    match transcribe(file) {
        Ok(text) => {
            signals.status(READY)?;
            Ok(if text.trim().is_empty() {
                "В записи не удалось разобрать речь.".into()
            } else {
                text
            })
        }
        Err(error) => {
            signals.status(error)?;
            Ok(UNREADABLE.into())
        }
    }
}

/// Fit Telegram's message limit even for surrogate pairs. Prefer paragraph/word
/// boundaries and preserve the full transcript when multiple messages are needed.
pub(super) fn chunks(mut rest: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let mut units = 0;
        let mut whitespace = None;
        let mut end = rest.len();
        for (index, ch) in rest.char_indices() {
            units += ch.len_utf16();
            if units > 4096 {
                end = whitespace.unwrap_or(index);
                break;
            }
            if ch.is_whitespace() {
                whitespace = Some(index);
            }
        }
        let (part, tail) = rest.split_at(end);
        rest = tail;
        Some(part.trim_end())
    })
}
