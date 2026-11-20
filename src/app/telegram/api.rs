//! The token is part of Telegram's URLs. Never surface raw HTTP errors, URLs or bodies.
use super::http::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fmt,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use zeroize::Zeroizing;

pub(super) const MAX_BYTES: u64 = 20 * 1024 * 1024;
const ORIGIN: &str = "https://api.telegram.org";

#[derive(Debug)]
pub(super) enum Error {
    Network,
    Timeout,
    Connection(&'static str),
    InvalidResponse,
    Rejected(u16),
    RateLimited(u64),
    TooLarge,
    Cancelled,
    Storage,
    FileUnavailable,
    ChatUnavailable,
}

impl Error {
    pub(super) fn fatal(&self) -> bool {
        matches!(self, Self::Rejected(401 | 403 | 404 | 409))
    }

    pub(super) fn retryable_download(&self) -> bool {
        matches!(
            self,
            Self::Network
                | Self::Timeout
                | Self::Connection(_)
                | Self::RateLimited(_)
                | Self::Rejected(408 | 429 | 500..=599)
        )
    }

    pub(super) fn definitely_rejected(&self) -> bool {
        matches!(self, Self::ChatUnavailable | Self::Rejected(400..=407 | 409..=499))
    }

    pub(super) fn retry_delay(&self) -> Duration {
        match self {
            Self::RateLimited(seconds) => Duration::from_secs((*seconds).max(1)),
            _ => Duration::from_secs(5),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Connection(reason) => reason,
            Self::Timeout => "Telegram не ответил вовремя.",
            Self::Network => "Нет связи с Telegram.",
            Self::InvalidResponse => "Не удалось прочитать ответ Telegram.",
            Self::Rejected(401 | 404) => "Telegram отклонил запрос. Проверьте токен бота.",
            Self::Rejected(403) => "Telegram запретил доступ.",
            Self::Rejected(409) => "У бота включён webhook или он уже запущен в другом приложении.",
            Self::RateLimited(_) => "Слишком много запросов к Telegram.",
            Self::Rejected(code @ 500..=599) => {
                return write!(f, "Сбой на стороне Telegram (HTTP {code}).");
            }
            Self::Rejected(_) => "Telegram отклонил запрос.",
            Self::TooLarge => {
                "Бот может скачать файл размером до 20 МБ. Большие файлы откройте в Voai на Mac."
            }
            Self::Cancelled => "Загрузка отменена.",
            Self::FileUnavailable => "Файл недоступен в Telegram. Перешлите его ещё раз.",
            Self::ChatUnavailable => "Бот не может отправить сообщение в этот чат.",
            Self::Storage => "Не удалось сохранить временный файл на Mac.",
        };
        f.write_str(text)
    }
}

pub(super) struct Update {
    pub(super) id: i64,
    pub(super) message: Option<Message>,
}

pub(super) struct Message {
    pub(super) chat: i64,
    pub(super) id: i64,
    pub(super) text: Option<String>,
    pub(super) media: Option<Media>,
}

pub(super) struct Media {
    pub(super) id: String,
    pub(super) extension: String,
    pub(super) size: Option<u64>,
}

pub(super) struct Api {
    agent: Client,
    origin: String,
    token: Zeroizing<String>,
}

impl Api {
    pub(super) fn validate_token(token: &str) -> Result<(), String> {
        let token = token.trim();
        if !token.split_once(':').is_some_and(|(id, secret)| {
            !id.is_empty()
                && id.len() <= 20
                && id.bytes().all(|c| c.is_ascii_digit())
                && (16..=128).contains(&secret.len())
                && secret
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        }) {
            return Err("Проверьте токен из BotFather.".into());
        }
        Ok(())
    }

    pub(super) fn new(token: &str, agent: Client) -> Result<Self, String> {
        Self::validate_token(token)?;
        Ok(Self {
            agent,
            origin: ORIGIN.to_owned(),
            token: Zeroizing::new(token.trim().to_owned()),
        })
    }

    #[cfg(test)]
    pub(super) fn for_test(origin: String) -> Self {
        let port = origin
            .strip_prefix("http://127.0.0.1:")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        assert!(port > 0);
        let agent = Client::new(None);
        Self {
            agent,
            origin,
            token: Zeroizing::new("123456:abcdefghijklmnop".into()),
        }
    }

    fn call<T: serde::de::DeserializeOwned>(&self, method: &str, body: Value) -> Result<T, Error> {
        let url = format!("{}/bot{}/{}", self.origin, self.token.as_str(), method);
        let payload = serde_json::to_vec(&body).map_err(|_| Error::InvalidResponse)?;
        let mut bytes = Vec::new();
        let timeout = Duration::from_secs(if method == "getMe" { 100 } else { 15 });
        let response = self
            .agent
            .request(&url, Some(&payload), timeout, 1024 * 1024, |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .map_err(|error| match error {
                Error::Timeout if method == "getMe" => {
                    Error::Connection("Telegram не подтвердил бота за отведённое время.")
                }
                Error::TooLarge => Error::InvalidResponse,
                other => other,
            })?;
        if (500..600).contains(&response.status) {
            return Err(Error::Rejected(response.status));
        }
        let success = (200..300).contains(&response.status);
        let mut body: Value = serde_json::from_slice(&bytes).map_err(|_| {
            if success {
                Error::InvalidResponse
            } else {
                Error::Rejected(response.status)
            }
        })?;
        if !success || body["ok"] != true {
            let code = body["error_code"]
                .as_u64()
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(response.status);
            if code == 429 {
                return Err(Error::RateLimited(
                    body.pointer("/parameters/retry_after")
                        .and_then(Value::as_u64)
                        .unwrap_or(5),
                ));
            }
            if code == 400
                && method == "getFile"
                && body["description"]
                    .as_str()
                    .is_some_and(|s| s.contains("file is too big"))
            {
                return Err(Error::TooLarge);
            }
            if code == 403 && method == "sendMessage" {
                return Err(Error::ChatUnavailable);
            }
            return Err(Error::Rejected(code));
        }
        serde_json::from_value(body.get_mut("result").ok_or(Error::InvalidResponse)?.take())
            .map_err(|_| Error::InvalidResponse)
    }

    pub(super) fn me(&self) -> Result<(i64, String), Error> {
        let me: Value = self.call("getMe", json!({}))?;
        let id = me["id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or(Error::InvalidResponse)?;
        let name = me["username"]
            .as_str()
            .filter(|s| {
                (1..=32).contains(&s.len())
                    && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            })
            .ok_or(Error::InvalidResponse)?;
        if me["is_bot"] != true {
            return Err(Error::InvalidResponse);
        }
        Ok((id, name.to_owned()))
    }

    pub(super) fn updates(&self, offset: i64) -> Result<Vec<Update>, Error> {
        #[derive(Deserialize)]
        struct Raw {
            update_id: i64,
            message: Option<Value>,
        }
        let updates: Vec<Raw> = self.call(
            "getUpdates",
            json!({
                "offset": offset, "limit": 4, "timeout": 5, "allowed_updates": ["message"]
            }),
        )?;
        Ok(updates
            .into_iter()
            .map(|update| Update {
                id: update.update_id,
                message: update.message.as_ref().and_then(private_message),
            })
            .collect())
    }

    pub(super) fn send(&self, chat: i64, reply: i64, text: &str) -> Result<(), Error> {
        let _: serde::de::IgnoredAny = self.call(
            "sendMessage",
            json!({
                "chat_id": chat, "text": text, "link_preview_options": {"is_disabled": true},
                "reply_parameters": {"message_id": reply, "allow_sending_without_reply": true}
            }),
        )?;
        Ok(())
    }

    pub(super) fn download(
        &self,
        id: &str,
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), Error> {
        #[derive(Deserialize)]
        struct FileInfo {
            file_path: String,
            file_size: Option<u64>,
        }

        check_cancelled(cancelled)?;
        let info: FileInfo = self.call("getFile", json!({"file_id": id}))?;
        if info.file_size.is_some_and(|n| n > MAX_BYTES) {
            return Err(Error::TooLarge);
        }
        if !valid_file_path(&info.file_path) {
            return Err(Error::InvalidResponse);
        }
        check_cancelled(cancelled)?;
        let directory = path.parent().ok_or(Error::Storage)?;
        let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|_| Error::Storage)?;
        let mut count = 0u64;
        let url = format!(
            "{}/file/bot{}/{}",
            self.origin,
            self.token.as_str(),
            info.file_path
        );
        let response =
            self.agent
                .request(&url, None, Duration::from_secs(180), MAX_BYTES, |chunk| {
                    check_cancelled(cancelled)?;
                    temp.write_all(chunk).map_err(|_| Error::Storage)?;
                    count += chunk.len() as u64;
                    Ok(())
                })?;
        match response.status {
            200..=299 => {}
            404 => return Err(Error::FileUnavailable),
            429 => {
                return Err(Error::RateLimited(if response.retry_after == 0 {
                    5
                } else {
                    response.retry_after
                }));
            }
            code => return Err(Error::Rejected(code)),
        }
        if info.file_size.is_some_and(|size| count < size) {
            return Err(Error::Network);
        }
        if count == 0 || info.file_size.is_some_and(|size| count != size) {
            return Err(Error::InvalidResponse);
        }
        check_cancelled(cancelled)?;
        temp.persist(path).map_err(|_| Error::Storage)?;
        Ok(())
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn valid_file_path(path: &str) -> bool {
    path.len() <= 1024
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        })
}

fn private_message(raw: &Value) -> Option<Message> {
    let chat = raw.pointer("/chat/id")?.as_i64()?;
    let id = raw["message_id"].as_i64()?;
    if id <= 0
        || chat <= 0
        || raw.pointer("/chat/type")?.as_str()? != "private"
        || raw.pointer("/from/id")?.as_i64()? != chat
        || raw.pointer("/from/is_bot")?.as_bool()?
        || !raw["sender_chat"].is_null()
        || !raw["business_connection_id"].is_null()
    {
        return None;
    }
    // Pairing commands must be sent by the user. Forwarded voice and video still qualify.
    let text = if raw["forward_origin"].is_null() && raw["via_bot"].is_null() {
        raw["text"].as_str().map(str::to_owned)
    } else {
        None
    };
    let media = [
        ("voice", Some("ogg")),
        ("video_note", Some("mp4")),
        ("video", Some("mp4")),
        ("audio", Some("mp3")),
        ("document", None),
    ]
    .into_iter()
    .find_map(|(kind, fallback)| media(&raw[kind], fallback));
    Some(Message {
        id,
        chat,
        text,
        media,
    })
}

fn media(file: &Value, fallback: Option<&str>) -> Option<Media> {
    let id = file["file_id"].as_str().filter(|id| !id.is_empty())?;
    let named = file["file_name"]
        .as_str()
        .and_then(|name| Path::new(name).extension()?.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|ext| {
            matches!(
                ext.as_str(),
                "aac"
                    | "aif"
                    | "aiff"
                    | "caf"
                    | "flac"
                    | "m4a"
                    | "m4b"
                    | "mp3"
                    | "oga"
                    | "ogg"
                    | "opus"
                    | "wav"
                    | "mp4"
                    | "m4v"
                    | "mov"
                    | "mpg"
                    | "mpeg"
                    | "webm"
                    | "mkv"
                    | "avi"
                    | "3gp"
                    | "3gpp"
            )
        });
    let ext = match file["mime_type"]
        .as_str()
        .unwrap_or("")
        .split(';')
        .next()?
        .trim()
    {
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/opus" => Some("opus"),
        "audio/mp4" | "audio/x-m4a" => Some("m4a"),
        "audio/mpeg" => Some("mp3"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/aac" => Some("aac"),
        "audio/webm" | "video/webm" => Some("webm"),
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        "video/x-matroska" => Some("mkv"),
        _ => None,
    };
    Some(Media {
        id: id.to_owned(),
        extension: ext.or(named.as_deref()).or(fallback)?.to_owned(),
        size: file["file_size"].as_u64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, Read},
        net::TcpListener,
        thread,
    };

    #[test]
    fn identity_accepts_a_delayed_reply_and_shutdown_interrupts_an_active_request() {
        use std::{net::TcpStream, sync::mpsc, time::Instant};
        for cancel in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let api = Api::for_test(format!("http://{}", listener.local_addr().unwrap()));
            let handle = api.agent.clone();
            let (ready, waiting) = mpsc::channel();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                // Drain the POST body too so closing the fixture sends FIN, not RST.
                super::super::tests::request(&stream);
                ready.send(()).unwrap();
                thread::sleep(Duration::from_millis(200));
                if !cancel {
                    let body =
                        r#"{"ok":true,"result":{"id":42,"username":"test_bot","is_bot":true}}"#;
                    write!(
                        &mut stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                } else {
                    // Keep the socket open until the client itself shuts it down.
                    TcpStream::set_read_timeout(&stream, Some(Duration::from_secs(2))).unwrap();
                    let mut rest = Vec::new();
                    stream.read_to_end(&mut rest).unwrap();
                }
            });
            let client = thread::spawn(move || api.me());
            waiting.recv_timeout(Duration::from_secs(3)).unwrap();
            if cancel {
                let start = Instant::now();
                handle.shutdown();
                assert!(client.join().unwrap().is_err());
                assert!(start.elapsed() < Duration::from_secs(2));
            } else {
                assert_eq!(client.join().unwrap().unwrap(), (42, "test_bot".into()));
            }
            server.join().unwrap();
        }
    }

    #[test]
    #[ignore = "requires a connection to the public Telegram Bot API"]
    fn public_api_rejects_synthetic_token() {
        let api = Api::new("123456:abcdefghijklmnop", Client::new(None)).unwrap();
        let result = api.me();
        assert!(
            matches!(result, Err(Error::Rejected(401 | 404))),
            "{result:?}"
        );
    }

    #[test]
    #[ignore = "requires a local SOCKS5 proxy with access to Telegram"]
    fn public_api_through_configured_loopback_proxy() {
        let port = std::env::var("VOAI_TELEGRAM_QA_PROXY_PORT")
            .unwrap()
            .parse()
            .unwrap();
        let api = Api::new("123456:abcdefghijklmnop", Client::new(Some(port))).unwrap();
        let result = api.me();
        assert!(
            matches!(result, Err(Error::Rejected(401 | 404))),
            "{result:?}"
        );
    }

    fn message() -> Value {
        json!({"message_id": 1, "chat": {"id": 19, "type": "private"}, "from": {"id": 19, "is_bot": false}})
    }

    #[test]
    fn accepts_forwarded_voice_video_and_media_documents_only_for_private_human_sender() {
        for (field, media, ext) in [
            ("voice", json!({"file_id": "a", "file_size": 721}), "ogg"),
            ("video_note", json!({"file_id": "b"}), "mp4"),
            (
                "video",
                json!({"file_id": "c", "mime_type": "video/quicktime"}),
                "mov",
            ),
            (
                "audio",
                json!({"file_id": "d", "mime_type": "audio/mpeg"}),
                "mp3",
            ),
            (
                "document",
                json!({"file_id": "e", "file_name": "../example.WAV"}),
                "wav",
            ),
            (
                "document",
                json!({"file_id": "f", "mime_type":"video/mp4"}),
                "mp4",
            ),
        ] {
            let mut raw = message();
            raw[field] = media;
            raw["forward_origin"] = json!({"type":"channel"});
            raw["text"] = json!("/start dont-pair");
            let message = private_message(&raw).unwrap();
            assert!(message.text.is_none());
            assert_eq!(message.media.unwrap().extension, ext);
        }

        for (pointer, value) in [
            ("/chat/type", json!("group")),
            ("/from/is_bot", json!(true)),
            ("/from/id", json!(21)),
        ] {
            let mut raw = message();
            *raw.pointer_mut(pointer).unwrap() = value;
            assert!(private_message(&raw).is_none());
        }
        let mut raw = message();
        raw["document"] =
            json!({"file_id":"e", "file_name":"voice.ogg.sh","mime_type":"application/pdf"});
        assert!(private_message(&raw).unwrap().media.is_none());
    }

    #[test]
    fn rejects_unsafe_url_components() {
        for token in [
            "",
            "123:abcdefghijklmnop?x",
            "123:abcdefghijklmnop/x",
            "host:abcdefghijklmnop",
        ] {
            assert!(Api::new(token, Client::new(None)).is_err());
        }
        assert!(valid_file_path("voice/file_127.oga"));
        for path in [
            "/a",
            "../x",
            "a/../x",
            "voice//x",
            "https://example.org/x",
            "x?y",
            "x%2fy",
        ] {
            assert!(!valid_file_path(path));
        }
    }

    // Serve canned Bot API envelopes on loopback; no real token or Telegram traffic.
    fn server(status: u16, json: &Value) -> (Api, thread::JoinHandle<()>) {
        let socket = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let api = Api::for_test(format!("http://{}", socket.local_addr().unwrap()));
        let body = json.to_string();
        let handle = thread::spawn(move || {
            let mut connection = socket.accept().unwrap().0;
            let mut reader = std::io::BufReader::new(&connection);
            let mut len = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = value.trim().parse().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; len]).unwrap();
            write!(connection, "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        (api, handle)
    }

    #[test]
    fn handles_flood_waits_conflicts_and_bad_credentials_without_exposing_descriptions() {
        for (code, fatal, delay) in [(429, false, 17), (409, true, 5), (401, true, 5)] {
            let (api, handle) = server(
                code,
                &json!({"ok":false, "error_code":code, "description":"secret raw request", "parameters":{"retry_after":17}}),
            );
            let err = api.updates(1).err().unwrap();
            assert_eq!(err.fatal(), fatal);
            assert_eq!(err.retry_delay(), Duration::from_secs(delay));
            assert!(!err.to_string().contains("secret"));
            handle.join().unwrap();
        }
    }

    #[test]
    fn refuses_large_or_unsafe_file_before_downloading() {
        for info in [
            json!({"file_path":"voices/f.ogg","file_size":MAX_BYTES + 1}),
            json!({"file_path":"../bad"}),
        ] {
            let (api, handle) = server(200, &json!({"ok":true, "result":info}));
            let dir = tempfile::tempdir().unwrap();
            let err = api
                .download("id", &dir.path().join("audio.ogg"), &AtomicBool::new(false))
                .unwrap_err();
            assert!(matches!(err, Error::TooLarge | Error::InvalidResponse));
            assert_eq!(dir.path().read_dir().unwrap().count(), 0);
            handle.join().unwrap();
        }
    }
}
