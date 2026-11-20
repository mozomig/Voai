use super::*;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    time::Duration,
};

#[test]
fn long_replies_preserve_emoji_and_unbroken_words() {
    let text = format!("{} {}", "🙂".repeat(2050), "ю".repeat(7000));
    let parts: Vec<_> = worker::chunks(&text).collect();
    assert!(
        parts
            .iter()
            .all(|s| !s.is_empty() && s.encode_utf16().count() <= 4096)
    );
    assert_eq!(parts.concat().replace(' ', ""), text.replace(' ', ""));
    assert_eq!(worker::chunks("   ").count(), 0);
}

fn message(update: i64, chat: i64, fields: Value) -> Value {
    let mut msg = json!({"message_id":update*10, "chat":{"type":"private","id":chat},"from":{"id":chat,"is_bot":false}});
    msg.as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    json!({"update_id": update, "message": msg})
}

pub(super) fn request(socket: &std::net::TcpStream) -> (String, Value) {
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(socket);
    let mut request = String::new();
    reader.read_line(&mut request).unwrap();
    let mut length = 0;
    loop {
        let mut s = String::new();
        assert!(reader.read_line(&mut s).unwrap() > 0);
        if s.trim().is_empty() {
            break;
        }
        if let Some(v) = s.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let path = request.split_whitespace().nth(1).unwrap().to_owned();
    (path, body)
}

#[test]
fn pairs_once_and_replies_to_forwarded_video_only_for_the_owner() {
    let (link_tx, link_rx) = mpsc::channel::<String>();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = Api::for_test(format!("http://{}", listener.local_addr().unwrap()));
    let fake = thread::spawn(move || {
        let mut replies = Vec::new();
        let mut downloads = 0;
        let mut media_requests = 0;
        let mut typing = false;
        let mut identity_attempts = 0;
        for req in listener.incoming() {
            let mut socket = req.unwrap();
            let (path, body) = request(&socket);
            let mut status = 200;
            let data = match path.rsplit('/').next().unwrap() {
                "getMe" => {
                    identity_attempts += 1;
                    if identity_attempts == 1 {
                        write!(socket, "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").unwrap();
                        continue;
                    }
                    json!({"ok":true,"result":{"id":123456,"is_bot":true,"username":"sample_bot"}})
                        .to_string()
                }
                "getUpdates" if body["offset"] == 0 => {
                    let link = link_rx.recv_timeout(Duration::from_secs(4)).unwrap();
                    let secret = link.split_once("?start=").unwrap().1;
                    let video = json!({"message_thread_id":72,"forward_origin":{"type":"hidden_user"},"video":{"file_id":"forwarded","file_size":5}});
                    json!({"ok":true,"result":[
                        message(1, 21, json!({"text":"/start guess"})),
                        message(2, 88, json!({"text":format!("/start {secret}")})),
                        message(3, 21, video.clone()), message(4,88,video)
                    ]})
                    .to_string()
                }
                "getUpdates" => {
                    assert_eq!(body["offset"], 5);
                    status = 409;
                    json!({"ok":false,"error_code":409}).to_string()
                }
                "getFile" => {
                    assert_eq!(body["file_id"], "forwarded");
                    downloads += 1;
                    if downloads == 1 {
                        status = 429;
                        json!({"ok":false,"error_code":429,"parameters":{"retry_after":1}})
                            .to_string()
                    } else {
                        json!({"ok":true,"result":{"file_path":"videos/video.mp4","file_size":5}})
                            .to_string()
                    }
                }
                "video.mp4" => {
                    media_requests += 1;
                    if media_requests == 1 {
                        "me".into()
                    } else {
                        "media".into()
                    }
                }
                "sendChatAction" => {
                    assert_eq!(
                        body,
                        json!({"chat_id":88,"message_thread_id":72,"action":"typing"})
                    );
                    assert_eq!(replies.len(), 1);
                    typing = true;
                    // Cosmetic feedback must never prevent the actual response.
                    status = 429;
                    json!({"ok":false,"error_code":429,"parameters":{"retry_after":20}}).to_string()
                }
                "sendMessage" => {
                    assert_eq!(body["chat_id"], 88);
                    replies.push((
                        body["reply_parameters"]["message_id"].as_i64().unwrap(),
                        body["text"].as_str().unwrap().to_owned(),
                    ));
                    json!({"ok":true,"result":{"message_id":130}}).to_string()
                }
                other => panic!("Unexpected test endpoint: {other}"),
            };
            write!(socket,"HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{data}",data.len()).unwrap();
            if status == 409 {
                break;
            }
        }
        assert_eq!(identity_attempts, 2);
        assert!(typing, "no progress indication while the owner was waiting");
        assert_eq!(downloads, 3);
        assert_eq!(media_requests, 2);
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].0, 20);
        assert_eq!(replies[1], (40, "Тестовое видео".into()));
    });
    let (tx, rx) = mpsc::channel();
    let (wake, _wake_rx) = async_channel::bounded(1);
    let signals = Signals {
        tx,
        wake,
        cancelled: Arc::default(),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("telegram.json");
    let worker_path = path.clone();
    let worker = thread::spawn(move || {
        worker::serve(&signals, &api, State::new(), &worker_path, |path| {
            assert_eq!(path.extension().unwrap(), "mp4");
            assert_eq!(fs::read(path).unwrap(), b"media");
            Ok("Тестовое видео".into())
        })
    });

    let mut reported_outage = false;
    loop {
        let event = match rx.recv_timeout(Duration::from_secs(8)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("Bot timed out waiting for the mock Telegram server")
            }
        };
        match event {
            Event::Status(TelegramStatus::Retrying(api::Error::Rejected(502), _)) => {
                reported_outage = true
            }
            Event::Identity { link, .. } => {
                link_tx.send(link.unwrap()).unwrap();
            }
            _ => {}
        }
    }
    assert!(worker.join().unwrap().is_err());
    fake.join().unwrap();
    let state = State::load(&path).unwrap().unwrap();
    assert!(
        reported_outage,
        "missing Telegram server error during automatic recovery"
    );
    assert!(state.pending.is_none());
    assert_eq!(state.owner, Some(88));
    assert_eq!(state.offset(state::now()), 5);
}

#[test]
fn restart_recovers_only_unconfirmed_parts_after_a_lost_response() {
    // Persisted text must survive being offline. An already accepted part must
    // never be sent again unless the owner explicitly requests it.
    for resend in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telegram.json");
        let mut state = State::new();
        state.bot = Some(123456);
        state.owner = Some(88);
        state.advance(40, state::now());
        state.pending = Some(state::PendingReply {
            message: 77,
            remaining: ["один", "два", "три"].map(String::from).into(),
            confirmed: 0,
            in_flight: false,
        });
        state.save(&path).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut sent = vec![];
            for conn in listener.incoming() {
                let mut conn = conn.unwrap();
                let (path, body) = request(&conn);
                let (status, data) = match path.rsplit('/').next().unwrap() {
                    "getMe" => (
                        200,
                        json!({"ok":true,"result":{"id":123456,"is_bot":true,"username":"sample_bot"}}),
                    ),
                    "sendMessage" => {
                        assert_eq!(body["chat_id"], 88);
                        assert_eq!(body["reply_parameters"]["message_id"], 77);
                        sent.push(body["text"].as_str().unwrap().to_owned());
                        if sent.len() == 2 {
                            continue;
                        } // Lost ACK after accepting part two.
                        (200, json!({"ok":true,"result":{"message_id":100}}))
                    }
                    "getUpdates" => {
                        assert_eq!(body["offset"], 41);
                        (409, json!({"ok":false,"error_code":409}))
                    }
                    _ => panic!("Unexpected endpoint"),
                };
                let data = data.to_string();
                write!(conn,"HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{data}",data.len()).unwrap();
                if status == 409 {
                    break;
                }
            }
            sent
        });

        for restarting in [false, true] {
            let (tx, rx) = mpsc::channel();
            let (wake, _) = async_channel::bounded(1);
            let signals = Signals {
                tx,
                wake,
                cancelled: Arc::default(),
            };
            let state = State::load(&path).unwrap().unwrap();
            let file = path.clone();
            let api = Api::for_test(origin.clone());
            let worker = thread::spawn(move || {
                worker::serve(&signals, &api, state, &file, |_| {
                    panic!("Repeated inference")
                })
            });
            let mut asked = false;
            loop {
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Event::Unconfirmed {
                        part,
                        total,
                        decision,
                    }) => {
                        assert!(!asked);
                        asked = true;
                        assert_eq!((part, total), (2, 3));
                        let saved = State::load(&path).unwrap().unwrap().pending.unwrap();
                        assert_eq!(saved.confirmed, 1);
                        assert!(saved.in_flight);
                        if restarting {
                            decision
                                .send(Ok(if resend {
                                    DeliveryDecision::Resend
                                } else {
                                    DeliveryDecision::Confirm
                                }))
                                .unwrap();
                        }
                        // On first run drop the reply channel to simulate shutdown.
                    }
                    Ok(_) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(_) => panic!("Delivery timed out"),
                }
            }
            assert!(asked);
            assert!(worker.join().unwrap().is_err());
        }
        assert!(State::load(&path).unwrap().unwrap().pending.is_none());
        let expected = if resend {
            vec!["один", "два", "два", "три"]
        } else {
            vec!["один", "два", "три"]
        };
        assert_eq!(server.join().unwrap(), expected);
    }
}
