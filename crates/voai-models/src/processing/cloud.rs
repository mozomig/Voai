use super::{Provider, credentials, edited, validate_input, validate_processing_prompt};
use crate::{check_cancelled, store::http_failure};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{sync::atomic::AtomicBool, time::Duration};

pub(super) fn extract_response(provider: Provider, data: &Value) -> Result<String> {
    match provider {
        Provider::OpenAI => {
            ensure!(
                data.pointer("/choices/0/finish_reason")
                    .and_then(Value::as_str)
                    == Some("stop"),
                "Провайдер не завершил редактирование"
            );
            edited(
                data.pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        }
        Provider::Anthropic => {
            ensure!(
                data["stop_reason"].as_str() == Some("end_turn"),
                "Провайдер не завершил редактирование"
            );
            let blocks = data["content"]
                .as_array()
                .context("Не удалось прочитать результат провайдера")?;
            let text = blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            edited(&text)
        }
    }
}

pub fn process_cloud(
    provider: Provider,
    model: &str,
    prompt: &str,
    transcript: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    validate_input(transcript)?;
    validate_processing_prompt(prompt)?;
    check_cancelled(cancel)?;
    ensure!(
        provider.models().contains(&model),
        "Неизвестная модель провайдера"
    );
    let secret = credentials::read(provider)?.context("Укажите API-ключ в настройках")?;
    let key = std::str::from_utf8(&secret).context("Некорректный ключ в связке ключей")?;
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    let endpoint = match provider {
        Provider::OpenAI => "https://api.openai.com/v1/chat/completions",
        Provider::Anthropic => "https://api.anthropic.com/v1/messages",
    };
    send_edit(
        agent.post(endpoint),
        provider,
        model,
        key,
        prompt,
        transcript,
        cancel,
    )
}

fn send_edit(
    request: ureq::RequestBuilder<ureq::typestate::WithBody>,
    provider: Provider,
    model: &str,
    key: &str,
    prompt: &str,
    transcript: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    let (request, body) = match provider {
        Provider::OpenAI => {
            let mut body = json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": prompt},
                    {"role": "user", "content": transcript}
                ],
                "max_completion_tokens": 4096,
                "store": false
            });
            if model == "gpt-6-luna" {
                body["reasoning_effort"] = json!("none");
            }
            (
                request.header("Authorization", format!("Bearer {key}")),
                body,
            )
        }
        Provider::Anthropic => (
            request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            json!({"model":model,"system":prompt,"messages":[{"role":"user","content":transcript}],"max_tokens":4096}),
        ),
    };
    check_cancelled(cancel)?;
    let mut response = request
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("{}: {}", provider.name(), http_failure(&e)))?;
    check_cancelled(cancel)?;
    let value: Value = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_json()
        .map_err(|_| anyhow::anyhow!("Некорректный ответ провайдера"))?;
    check_cancelled(cancel)?;
    extract_response(provider, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        thread,
    };
    #[test]
    fn refuses_truncated_cloud_completion() {
        assert!(
            extract_response(
                Provider::OpenAI,
                &json!({"choices":[{"finish_reason":"length","message":{"content":"incomplete"}}]})
            )
            .is_err()
        );
        assert_eq!(extract_response(Provider::Anthropic,&json!({"stop_reason":"end_turn","content":[{"type":"thinking","thinking":"private"},{"type":"text","text":" Готово. "}]})).unwrap(),"Готово.");
    }
    #[test]
    fn both_cloud_adapters_send_a_transcript_and_parse_their_own_wire_format() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let prompt = "Верни ответ в формате Markdown.";
        let transcript = "проверка, один два";
        let edited = "Проверка, один-два.";
        for provider in [Provider::OpenAI, Provider::Anthropic] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let url = format!("http://{}/edit", listener.local_addr().unwrap());
            let model = provider.models()[0];
            thread::scope(|scope| {
                scope.spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut request = BufReader::new(&mut stream);
                    let headers: Vec<String> = request.by_ref().lines().map(|line|line.unwrap()).take_while(|line|!line.is_empty()).map(|line|line.to_ascii_lowercase()).collect();
                    let len: usize = headers.iter().find_map(|h|h.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                    let mut data = vec![0;len];
                    request.read_exact(&mut data).unwrap();
                    let body: Value = serde_json::from_slice(&data).unwrap();
                    assert_eq!(body["model"],model);
                    assert!(!body.to_string().contains("sample-api-key"));
                    let response = match provider {
                        Provider::OpenAI => {
                            assert!(headers.iter().any(|h| h=="authorization: bearer sample-api-key"));
                            assert_eq!(body["messages"][0],json!({"role":"system","content":prompt}));
                            assert_eq!(body["messages"][1],json!({"role":"user","content":transcript}));
                            json!({"choices":[{"finish_reason":"stop","message":{"content":edited}}]})
                        }
                        Provider::Anthropic => {
                            assert!(headers.iter().any(|h|h=="x-api-key: sample-api-key"));
                            assert!(headers.iter().any(|h| h=="anthropic-version: 2023-06-01"));
                            assert_eq!(body["system"],prompt);
                            assert_eq!(body["messages"][0],json!({"role":"user","content":transcript}));
                            json!({"stop_reason":"end_turn","content":[{"type":"text","text":edited}]})
                        }
                    }.to_string();
                    write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
                });
                let agent = ureq::Agent::config_builder()
                    .proxy(None)
                    .timeout_global(Some(Duration::from_secs(5)))
                    .build()
                    .new_agent();
                assert_eq!(
                    send_edit(
                        agent.post(url),
                        provider,
                        model,
                        "sample-api-key",
                        prompt,
                        transcript,
                        &AtomicBool::new(false)
                    )
                    .unwrap(),
                    edited
                );
            });
        }
    }

    #[test]
    fn no_provider_contact_after_cancel() {
        assert!(
            process_cloud(
                Provider::OpenAI,
                "gpt-4o-mini",
                "Исправь фразу",
                "Привет",
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
