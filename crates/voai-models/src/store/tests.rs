use super::*;
use crate::CATALOG;
use std::sync::atomic::Ordering;

fn fixture() -> ModelFile {
    ModelFile {
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ..CATALOG[0].file
    }
}
#[test]
fn completes_interrupted_download_after_digest_check() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(tmp.path().into());
    let model = fixture();
    fs::create_dir_all(store.dir(&model)).unwrap();
    fs::write(store.dir(&model).join("download.part"), b"abc").unwrap();
    assert!(!store.installed(&model));
    store
        .download(&model, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(
        fs::read(store.installed_path(&model).unwrap()).unwrap(),
        b"abc"
    );
    assert_eq!(store.partial_bytes(&model), 0);
}
#[test]
fn refuses_a_modified_installed_file_with_the_same_length() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(tmp.path().into());
    let model = fixture();
    fs::create_dir_all(store.dir(&model)).unwrap();
    fs::write(store.dir(&model).join("download.part"), b"abc").unwrap();
    let path = store
        .download(&model, &AtomicBool::new(false), |_| {})
        .unwrap();
    let mut out = File::create(path).unwrap();
    out.write_all(b"bad").unwrap();
    out.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    assert!(!store.installed(&model));
}

#[test]
fn rejects_bad_digest_and_clears_bad_partial() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(tmp.path().into());
    let model = fixture();
    fs::create_dir_all(store.dir(&model)).unwrap();
    fs::write(store.dir(&model).join("download.part"), b"bad").unwrap();
    assert!(
        store
            .download(&model, &AtomicBool::new(false), |_| {})
            .is_err()
    );
    assert!(!store.installed(&model));
    assert_eq!(store.partial_bytes(&model), 0);
}
#[test]
fn interrupted_or_excess_bytes_never_report_complete() {
    let stop = AtomicBool::new(false);
    let mut out = Vec::new();
    assert!(transfer(&b"bc"[..], &mut out, 0, 1, 5, &stop, &mut |_| {}).is_err());
    assert_eq!(out, b"bc");
    out.clear();
    assert!(transfer(&b"bc"[..], &mut out, 0, 2, 3, &stop, &mut |_| {}).is_err());
    assert!(out.is_empty());
    stop.store(true, Ordering::Release);
    assert!(transfer(&b"abc"[..], &mut out, 0, 0, 3, &stop, &mut |_| {}).is_err());
    assert!(out.is_empty());
}

#[test]
fn restored_weights_repair_the_receipt_offline_and_detect_same_mtime_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let store = ModelStore::new(dir.path().into());
    let model = fixture();
    let weights_dir = store.dir(&model);
    fs::create_dir_all(&weights_dir).unwrap();
    let weights = weights_dir.join(model.filename);
    fs::write(&weights, b"abc").unwrap();
    let original = weights.metadata().unwrap();
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .build()
        .new_agent();
    store
        .download_with(
            &model,
            &AtomicBool::new(false),
            |_| {},
            &agent,
            "http://127.0.0.1:1/unused",
        )
        .unwrap();
    assert!(store.installed(&model));
    assert_eq!(original.ino(), weights.metadata().unwrap().ino());

    let replacement = weights_dir.join("replacement");
    let mut file = File::create(&replacement).unwrap();
    file.write_all(b"bad").unwrap();
    file.set_modified(original.modified().unwrap()).unwrap();
    drop(file);
    fs::rename(replacement, weights).unwrap();
    assert!(!store.installed(&model));
}

// A real loopback connection exercises range headers and short HTTP
// bodies through the same transport and file-publishing path as the app.
fn serve(
    responses: Vec<(Option<&'static str>, &'static str)>,
) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{BufRead, BufReader};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/weights", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for (expected_range, response) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut headers = BufReader::new(&socket);
            let mut range = None;
            loop {
                let mut line = String::new();
                assert!(headers.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':')
                    && k.eq_ignore_ascii_case("Range")
                {
                    range = Some(v.trim().to_owned());
                }
            }
            assert_eq!(range.as_deref(), expected_range);
            socket.write_all(response.as_bytes()).unwrap();
        }
    });
    (url, worker)
}

#[test]
fn range_ignored_then_disconnected_keeps_progress_and_resumes_after_reopening() {
    let tmp = tempfile::tempdir().unwrap();
    let model = fixture();
    let stop = AtomicBool::new(false);
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .build()
        .new_agent();
    let store = ModelStore::new(tmp.path().into());
    fs::create_dir_all(store.dir(&model)).unwrap();
    fs::write(store.dir(&model).join("download.part"), b"ab").unwrap();
    let (url, worker) = serve(vec![
        (
            Some("bytes=2-"),
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\na",
        ),
        (
            Some("bytes=2-"),
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 2-2/3\r\nConnection: close\r\n\r\nc",
        ),
    ]);
    assert!(
        store
            .download_with(&model, &stop, |_| {}, &agent, &url)
            .is_err()
    );
    assert_eq!(
        fs::read(store.dir(&model).join("download.part")).unwrap(),
        b"ab"
    );
    let reopened = ModelStore::new(tmp.path().into());
    let mut progress = vec![];
    let path = reopened
        .download_with(&model, &stop, |bytes| progress.push(bytes), &agent, &url)
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"abc");
    assert!(progress.windows(2).all(|p| p[0] <= p[1]));
    assert!(reopened.installed(&model));
    worker.join().unwrap();
}

#[test]
fn range_rejection_retries_once_with_the_full_response() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(tmp.path().into());
    let model = fixture();
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .build()
        .new_agent();
    fs::create_dir_all(store.dir(&model)).unwrap();
    fs::write(store.dir(&model).join("download.part"), b"a").unwrap();
    let (url, worker) = serve(vec![
        (
            Some("bytes=1-"),
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ),
        (
            None,
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
        ),
    ]);
    assert!(
        store
            .download_with(&model, &AtomicBool::new(false), |_| {}, &agent, &url)
            .is_ok()
    );
    assert!(store.installed(&model));
    worker.join().unwrap();
}

#[test]
fn cancelling_in_flight_keeps_the_partial_file_for_the_next_launch() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    let tmp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(tmp.path().into());
    let model = fixture();
    let cancel = AtomicBool::new(false);
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .build()
        .new_agent();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/weights", listener.local_addr().unwrap());
    let (received, wait_for_cancel) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut headers = BufReader::new(&socket);
            loop {
                let mut line = String::new();
                assert!(headers.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nab")
                .unwrap();
            wait_for_cancel
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        });
        let result = store.download_with(
            &model,
            &cancel,
            |bytes| {
                if bytes == 2 {
                    cancel.store(true, Ordering::Release);
                    received.send(()).unwrap();
                }
            },
            &agent,
            &url,
        );
        assert_eq!(result.unwrap_err().to_string(), "Операция отменена");
    });
    assert_eq!(store.partial_bytes(&model), 2);
    assert!(!store.installed(&model));
    let reopened = ModelStore::new(tmp.path().into());
    let (url, worker) = serve(vec![(
        Some("bytes=2-"),
        "HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 2-2/3\r\nConnection: close\r\n\r\nc",
    )]);
    reopened
        .download_with(&model, &AtomicBool::new(false), |_| {}, &agent, &url)
        .unwrap();
    worker.join().unwrap();
    assert!(reopened.installed(&model));
}

#[test]
fn cancelling_before_final_verification_keeps_the_complete_partial_for_offline_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(tmp.path().into());
    let model = fixture();
    fs::create_dir_all(store.dir(&model)).unwrap();
    fs::write(store.dir(&model).join("download.part"), b"abc").unwrap();
    let cancel = AtomicBool::new(false);
    let result = store.download(&model, &cancel, |bytes| {
        if bytes == model.bytes {
            cancel.store(true, Ordering::Release);
        }
    });
    assert_eq!(result.unwrap_err().to_string(), "Операция отменена");
    assert!(!store.installed(&model));
    assert_eq!(store.partial_bytes(&model), model.bytes);
    let reopened = ModelStore::new(tmp.path().into());
    reopened
        .download(&model, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(reopened.installed(&model));
}
