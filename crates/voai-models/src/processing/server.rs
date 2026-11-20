//! OpenAI-compatible servers explicitly selected on this Mac.
use super::{
    Provider, cloud::extract_response, credentials, validate_input, validate_processing_prompt,
};
use crate::check_cancelled;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::atomic::AtomicBool, time::Duration};
use ureq::http::Response;
use url::{Host, Url};
use voai_l10n::{tf, tr};

pub fn normalize_server_url(input: &str) -> Result<String> {
    let mut url = Url::parse(input.trim()).context(tr("server.address"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        tr("server.url")
    );
    match url.host() {
        // Use an IP literal so the local connection does not rely on DNS.
        Some(Host::Domain("localhost")) => url.set_host(Some("127.0.0.1"))?,
        Some(Host::Ipv4(ip)) if ip.is_loopback() => (),
        Some(Host::Ipv6(ip)) if ip.is_loopback() => (),
        _ => bail!(tr("server.loopback")),
    }
    if url.path() == "/" {
        url.set_path("/v1");
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn has_server_key(endpoint: &str) -> Result<bool> {
    credentials::has_account(&normalize_server_url(endpoint)?)
}

pub fn set_server_key(endpoint: &str, key: &str) -> Result<()> {
    credentials::set_key(&normalize_server_url(endpoint)?, key)
}

struct Server {
    endpoint: String,
    client: ureq::Agent,
    token: Option<zeroize::Zeroizing<Vec<u8>>>,
}

impl Server {
    fn connect(endpoint: &str, timeout: Duration) -> Result<Self> {
        let endpoint = normalize_server_url(endpoint)?;
        let token = credentials::read_account(&endpoint)?;
        Ok(Self::new(endpoint, token, timeout))
    }

    fn new(
        endpoint: String,
        token: Option<zeroize::Zeroizing<Vec<u8>>>,
        timeout: Duration,
    ) -> Self {
        let client = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(timeout))
            .build()
            .new_agent();
        Self {
            endpoint,
            client,
            token,
        }
    }

    fn authorize<B>(&self, request: ureq::RequestBuilder<B>) -> Result<ureq::RequestBuilder<B>> {
        if let Some(token) = &self.token {
            let token = std::str::from_utf8(token).context(tr("server.key"))?;
            Ok(request.header("Authorization", format!("Bearer {token}")))
        } else {
            Ok(request)
        }
    }

    fn models(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Model {
            id: String,
        }
        #[derive(Deserialize)]
        struct Models {
            data: Vec<Model>,
        }
        let request = self.authorize(self.client.get(format!("{}/models", self.endpoint)))?;
        let models: Models = read_json(request.call())?;
        let mut ids: Vec<_> = models
            .data
            .into_iter()
            .map(|model| model.id)
            .filter(|id| valid_model(id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    fn edit(&self, model: &str, prompt: &str, text: &str) -> Result<String> {
        let request = self.authorize(
            self.client
                .post(format!("{}/chat/completions", self.endpoint)),
        )?;
        let response: Value = read_json(request.send_json(json!({
            "model": model,
            "messages": [{"role":"system","content":prompt}, {"role":"user","content":text}],
            "stream": false
        })))?;
        extract_response(Provider::OpenAI, &response)
    }
}

fn valid_model(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
}

fn read_json<T: serde::de::DeserializeOwned>(
    result: Result<Response<ureq::Body>, ureq::Error>,
) -> Result<T> {
    let mut response = result.map_err(|err| match err {
        ureq::Error::StatusCode(401 | 403) => {
            anyhow::anyhow!(tr("server.unauthorized"))
        }
        ureq::Error::StatusCode(status) => anyhow::anyhow!(tf!("server.http", status = status)),
        ureq::Error::Timeout(_) => anyhow::anyhow!(tr("server.timeout")),
        _ => anyhow::anyhow!(tr("server.unavailable")),
    })?;
    ensure!(
        response.status().is_success(),
        tf!("server.http", status = response.status().as_u16())
    );
    response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_json()
        .map_err(|_| anyhow::anyhow!(tr("server.protocol")))
}

pub fn server_models(endpoint: &str) -> Result<Vec<String>> {
    Server::connect(endpoint, Duration::from_secs(10))?.models()
}

pub fn process_server(
    endpoint: &str,
    model: &str,
    prompt: &str,
    text: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    validate_input(text)?;
    validate_processing_prompt(prompt)?;
    ensure!(valid_model(model), tr("server.no.model"));
    check_cancelled(cancel)?;
    let server = Server::connect(endpoint, Duration::from_secs(120))?;
    check_cancelled(cancel)?;
    let output = server.edit(model, prompt, text)?;
    check_cancelled(cancel)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn addresses_are_canonical_loopback_urls() {
        assert_eq!(
            normalize_server_url("http://LOCALHOST:1234/").unwrap(),
            "http://127.0.0.1:1234/v1"
        );
        assert_eq!(
            normalize_server_url(" http://[::1]:11434/v1/ ").unwrap(),
            "http://[::1]:11434/v1"
        );
        for url in [
            "http://example.com/v1",
            "http://127.0.0.1.evil.example",
            "http://0.0.0.0",
            "http://user:pass@localhost",
            "http://localhost/?token=x",
            "file:///tmp/model",
            "https://localhost/#x",
        ] {
            assert!(normalize_server_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn discovery_and_edit_use_the_same_endpoint_and_scoped_token() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = Server::new(
            format!("http://{}/v1", listener.local_addr().unwrap()),
            Some(zeroize::Zeroizing::new(b"test-token".to_vec())),
            Duration::from_secs(4),
        );
        thread::scope(|scope| {
            scope.spawn(|| {
                for edit in [false, true] {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                    let mut request = BufReader::new(&mut stream);
                    let mut line = String::new();
                    request.read_line(&mut line).unwrap();
                    assert!(line.starts_with(if edit { "POST /v1/chat/completions " } else { "GET /v1/models " }));
                    let headers: Vec<_> = request.by_ref().lines().map(Result::unwrap)
                        .take_while(|line| !line.is_empty()).map(|line| line.to_ascii_lowercase()).collect();
                    assert!(headers.iter().any(|line| line == "authorization: bearer test-token"));
                    let response = if edit {
                        let len = headers.iter().find_map(|line|line.strip_prefix("content-length: "))
                            .unwrap().parse::<usize>().unwrap();
                        let mut bytes=vec![0;len];
                        request.read_exact(&mut bytes).unwrap();
                        let data: Value=serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(data["model"], "own/qwen");
                        assert_eq!(data["messages"],json!([{"role":"system","content":"Переведи."},{"role":"user","content":"Привет!"}]));
                        json!({"choices":[{"message":{"content":"Hello!"},"finish_reason":"stop"}]})
                    } else {
                        json!({"data":[{"id":"own/qwen"},{"id":"own/qwen"}]})
                    }.to_string();
                    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
                }
            });
            assert_eq!(client.models().unwrap(), ["own/qwen"]);
            assert_eq!(
                client.edit("own/qwen", "Переведи.", "Привет!").unwrap(),
                "Hello!"
            );
        });
    }

    #[test]
    fn redirect_cannot_move_a_local_request_to_a_different_server() {
        let origin = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        destination.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", origin.local_addr().unwrap());
        thread::scope(|scope| {
            scope.spawn(|| {
                let (mut stream, _) = origin.accept().unwrap();
                let mut request = BufReader::new(&mut stream);
                for line in request.by_ref().lines() {
                    if line.unwrap().is_empty() { break; }
                }
                write!(stream, "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", destination.local_addr().unwrap()).unwrap();
            });
            let client = Server::new(endpoint, None, Duration::from_secs(2));
            assert!(client.edit("model", "Correct the text.", "Test.").is_err());
            assert_eq!(
                destination.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        });
    }

    #[test]
    #[ignore = "requires an explicitly configured local inference server and model"]
    fn real_server_lists_and_runs_the_chosen_model() {
        let url = std::env::var("VOAI_TEST_SERVER_URL").unwrap();
        let model = std::env::var("VOAI_TEST_SERVER_MODEL").unwrap();
        assert!(server_models(&url).unwrap().contains(&model));
        let text = process_server(
            &url,
            &model,
            "Translate the user's text into English. Return only the translation.",
            "Доброе утро!",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            text.to_lowercase().contains("morning"),
            "Unexpected edit of the test phrase: {text}"
        );
    }
}
