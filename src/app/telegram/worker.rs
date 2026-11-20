use super::{
    DeliveryDecision, Event, Signals,
    api::{Api, Error, MAX_BYTES, Media, Message},
    state::{self, PendingReply, State},
};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
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
                let retry_in = delay.max(error.retry_delay());
                signals.status(format!("{error} Повторю через {} с.", retry_in.as_secs()))?;
                wait(signals, retry_in)?;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
    }
}

pub(super) fn serve(
    signals: &Signals,
    api: &Api,
    mut state: State,
    state_path: &Path,
    transcribe: impl Fn(PathBuf) -> Result<String, String>,
) -> Result<(), String> {
    let (bot, username) = connected(signals, READY, || api.me())?;
    if state.bot.is_some_and(|known| known != bot) {
        return Err("Токен принадлежит другому боту. Подключите его заново.".into());
    }
    if state.bot.is_none() {
        state.bot = Some(bot);
        state.save(state_path)?;
    }
    let secret = Uuid::new_v4().simple().to_string();
    let link = state
        .owner
        .is_none()
        .then(|| format!("https://t.me/{username}?start={secret}"));
    signals.event(Event::Identity {
        username,
        owner: state.owner,
        link,
    })?;
    signals.status(if state.owner.is_some() { READY } else { PAIR })?;
    deliver_pending(signals, api, &mut state, state_path)?;
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
            let was_unpaired = state.owner.is_none();
            let reply = match update.message {
                Some(message) if state.owner == Some(message.chat) => {
                    let text = response(signals, api, &message, &transcribe)?;
                    Some((message.id, text))
                }
                Some(message)
                    if state.owner.is_none() && message.text.as_deref() == Some(&pair_command) =>
                {
                    state.owner = Some(message.chat);
                    signals.status(READY)?;
                    Some((message.id, HELP.into()))
                }
                _ => None,
            };
            state.advance(update.id, state::now());
            if let Some((message, text)) = reply {
                state.pending = Some(PendingReply {
                    message,
                    remaining: chunks(&text).map(str::to_owned).collect(),
                    confirmed: 0,
                    in_flight: false,
                });
                // Commit the next update and its complete reply together, before sending.
                state.save(state_path)?;
                if was_unpaired && let Some(owner) = state.owner {
                    signals.event(Event::Paired(owner))?;
                }
                deliver_pending(signals, api, &mut state, state_path)?;
            }
        }
    }
}

fn deliver_pending(
    signals: &Signals,
    api: &Api,
    state: &mut State,
    path: &Path,
) -> Result<(), String> {
    while let Some(pending) = state.pending.as_mut() {
        let chat = state
            .owner
            .ok_or("Не удалось определить получателя ответа.")?;
        let Some(text) = pending.remaining.front() else {
            state.pending = None;
            state.save(path)?;
            signals.status(READY)?;
            break;
        };
        if !signals.active() {
            return Err("Остановлено.".into());
        }

        if pending.in_flight {
            let decision = signals.request(|decision| Event::Unconfirmed {
                part: pending.confirmed + 1,
                total: pending.confirmed + pending.remaining.len(),
                decision,
            })?;
            if matches!(decision, DeliveryDecision::Confirm) {
                pending.remaining.pop_front();
                pending.confirmed += 1;
            }
            pending.in_flight = false;
            state.save(path)?;
            continue;
        }
        let message = pending.message;
        let part = text.clone();
        pending.in_flight = true;
        state.save(path)?;
        // Once an attempt begins, only a definitive Telegram response (or the owner)
        // can clear this flag. A lost HTTP response cannot prove non-delivery.
        if !signals.active() {
            // We have not called Telegram yet, so the next launch can send safely.
            if let Some(pending) = &mut state.pending {
                pending.in_flight = false;
            }
            state.save(path)?;
            return Err("Остановлено.".into());
        }
        let result = api.send(chat, message, &part);
        let pending = state.pending.as_mut().expect("current delivery");
        match result {
            Ok(()) => {
                pending.remaining.pop_front();
                pending.confirmed += 1;
                pending.in_flight = false;
                let more = !pending.remaining.is_empty();
                if !more {
                    state.pending = None;
                }
                state.save(path)?;
                if more {
                    wait(signals, Duration::from_secs(1))?;
                }
            }
            Err(error @ Error::RateLimited(_)) => {
                pending.in_flight = false;
                state.save(path)?;
                wait(signals, error.retry_delay())?;
            }
            Err(error) if error.definitely_rejected() => {
                pending.in_flight = false;
                state.save(path)?;
                return Err(error.to_string());
            }
            Err(_) => continue, // Ask the owner before resending an ambiguous part.
        }
    }
    Ok(())
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
    thread::scope(|scope| {
        let (done, receiver) = mpsc::channel::<()>();
        scope.spawn(move || {
            let mut delay = Duration::from_millis(500);
            while receiver.recv_timeout(delay) == Err(mpsc::RecvTimeoutError::Timeout)
                && signals.active()
            {
                delay = match api.typing(message) {
                    Ok(()) => Duration::from_secs(4),
                    Err(Error::RateLimited(seconds)) => Duration::from_secs(seconds.max(5)),
                    Err(_) => return,
                };
            }
        });
        let result = transcribe_media(signals, api, media, transcribe);
        drop(done);
        result
    })
}

fn transcribe_media(
    signals: &Signals,
    api: &Api,
    media: &Media,
    transcribe: &impl Fn(PathBuf) -> Result<String, String>,
) -> Result<String, String> {
    signals.status("Загружаю запись…")?;
    let dir = tempfile::tempdir().map_err(|_| "Не удалось создать временную папку.")?;
    let file = dir.path().join(format!("media.{}", media.extension));
    let mut retries = 0;
    while let Err(error) = api.download(&media.id, &file, &signals.cancelled) {
        if retries < 4 && error.retryable_download() {
            signals.status("Загрузка прервалась. Повторяю…")?;
            wait(
                signals,
                Duration::from_secs(1 << retries).max(error.retry_delay()),
            )?;
            retries += 1;
            continue;
        }
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
