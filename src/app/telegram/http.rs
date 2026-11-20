//! Streaming URLSession adapter. A cloned handle cancels the session from the UI
//! while the bot worker is waiting for DNS, a response, or a media download.
use super::api::Error;
use std::{
    ffi::{CString, c_char, c_void},
    ptr::{self, NonNull},
    sync::Arc,
    time::Duration,
};

#[repr(C)]
struct NativeResult {
    error: i32,
    status: i32,
    retry_after: u64,
}

unsafe extern "C" {
    fn voai_http_create(proxy_port: u16) -> *mut c_void;
    fn voai_http_stop(session: *mut c_void);
    fn voai_http_release(session: *mut c_void);
    fn voai_http_request(
        session: *mut c_void,
        url: *const c_char,
        body: *const u8,
        body_len: usize,
        timeout: f64,
        limit: u64,
        write: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool,
        context: *mut c_void,
    ) -> NativeResult;
}

struct Session {
    handle: NonNull<c_void>,
}
// URLSession is thread-safe. The bridge serializes task creation and shutdown.
unsafe impl Send for Session {}
unsafe impl Sync for Session {}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { voai_http_release(self.handle.as_ptr()) };
    }
}

#[derive(Clone)]
pub(super) struct Client(Arc<Session>);

pub(super) struct Response {
    pub status: u16,
    pub retry_after: u64,
}

impl Client {
    pub fn new(proxy_port: Option<std::num::NonZeroU16>) -> Self {
        let session = NonNull::new(unsafe { voai_http_create(proxy_port.map_or(0, |p| p.get())) })
            .expect("URLSession initialization");
        Self(Arc::new(Session { handle: session }))
    }
    pub fn shutdown(&self) {
        unsafe { voai_http_stop(self.0.handle.as_ptr()) };
    }

    pub fn request<F>(
        &self,
        url: &str,
        body: Option<&[u8]>,
        timeout: Duration,
        limit: u64,
        write: F,
    ) -> Result<Response, Error>
    where
        F: FnMut(&[u8]) -> Result<(), Error> + Send,
    {
        let url = CString::new(url).map_err(|_| Error::InvalidResponse)?;
        let mut sink = Sink { write, error: None };
        let (bytes, len) = body.map_or((ptr::null(), 0), |b| (b.as_ptr(), b.len()));
        // The bridge blocks until the serial delegate has finished all callbacks.
        let response = unsafe {
            voai_http_request(
                self.0.handle.as_ptr(),
                url.as_ptr(),
                bytes,
                len,
                timeout.as_secs_f64(),
                limit,
                receive::<F>,
                ptr::from_mut(&mut sink).cast(),
            )
        };
        if let Some(e) = sink.error {
            return Err(e);
        }
        if response.error != 0 {
            return Err(network_error(response.error));
        }
        Ok(Response {
            status: u16::try_from(response.status).map_err(|_| Error::InvalidResponse)?,
            retry_after: response.retry_after,
        })
    }
}

struct Sink<F> {
    write: F,
    error: Option<Error>,
}
unsafe extern "C" fn receive<F: FnMut(&[u8]) -> Result<(), Error>>(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
) -> bool {
    // The bridge supplies a nonempty NSData buffer valid for this callback only.
    let sink = unsafe { &mut *ctx.cast::<Sink<F>>() };
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    match (sink.write)(bytes) {
        Ok(()) => true,
        Err(error) => {
            sink.error = Some(error);
            false
        }
    }
}

// NSURLErrorDomain values. Never propagate NSError descriptions or failing URLs.
fn network_error(code: i32) -> Error {
    match code {
        1 => Error::TooLarge,
        2 => Error::InvalidResponse,
        -999 => Error::Cancelled,
        -1001 => Error::Timeout,
        -1003 | -1006 => Error::Connection("app.telegram.dns"),
        -1004 => Error::Connection("app.telegram.connect_failed"),
        -1005 => Error::Connection("app.telegram.disconnected"),
        -1009 => Error::Connection("app.telegram.offline"),
        -1206..=-1200 => Error::Connection("app.telegram.tls"),
        _ => Error::Network,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener, thread};

    fn fixture(response: String) -> (String, thread::JoinHandle<()>) {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/synthetic-secret", server.local_addr().unwrap());
        let task = thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            super::super::tests::request(&stream);
            stream.write_all(response.as_bytes()).unwrap();
        });
        (url, task)
    }

    #[test]
    fn rejects_large_streams_with_or_without_content_length() {
        for headers in ["Content-Length: 16\r\n", ""] {
            let (url, server) = fixture(format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\n{headers}\r\nabcdefghijklmnop"
            ));
            let mut received = 0;
            let result =
                Client::new(None).request(&url, None, Duration::from_secs(2), 8, |chunk| {
                    received += chunk.len();
                    Ok(())
                });
            assert!(matches!(result, Err(Error::TooLarge)));
            assert!(received <= 8);
            server.join().unwrap();
        }
    }

    #[test]
    fn explicit_route_negotiates_socks5_with_the_loopback_proxy() {
        use std::io::Read;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = std::num::NonZeroU16::new(listener.local_addr().unwrap().port()).unwrap();
        let worker = thread::spawn(move || {
            Client::new(Some(port)).request(
                "https://voai-test.invalid/",
                None,
                Duration::from_secs(3),
                32,
                |_| Ok(()),
            )
        });
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        let mut peer = loop {
            match listener.accept() {
                Ok((peer, _)) => break peer,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "missing SOCKS5 connection"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("loopback proxy: {e}"),
            }
        };
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut header = [0; 2];
        peer.read_exact(&mut header).unwrap();
        assert_eq!(header[0], 5);
        let mut methods = vec![0; header[1] as usize];
        peer.read_exact(&mut methods).unwrap();
        assert!(methods.contains(&0));
        peer.write_all(&[5, 0]).unwrap();
        let mut connect = [0; 4];
        peer.read_exact(&mut connect).unwrap();
        assert_eq!(&connect[..3], &[5, 1, 0]);
        peer.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        drop(peer);
        drop(listener);
        assert!(worker.join().unwrap().is_err());
    }

    #[test]
    fn refuses_cross_origin_redirects() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        let (url, server) = fixture(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/capture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            target.local_addr().unwrap()
        ));
        let response = Client::new(None)
            .request(&url, Some(b"{}"), Duration::from_secs(2), 2048, |_| Ok(()))
            .unwrap();
        assert_eq!(response.status, 307);
        target.set_nonblocking(true).unwrap();
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        server.join().unwrap();
    }

    #[test]
    fn status_distinguishes_dns_tls_and_timeout_without_request_details() {
        let messages: std::collections::HashSet<_> = [-1003, -1001, -1202, -1004]
            .into_iter()
            .map(|c| {
                let error = network_error(c);
                assert!(error.retryable_download());
                error.to_string()
            })
            .collect();
        assert_eq!(messages.len(), 4);
    }
}
