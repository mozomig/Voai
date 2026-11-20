//! Serializes local-server discovery and publishes only the most recent request.
use std::{sync::mpsc, thread};
use voai_l10n::tr;

type DiscoveryResult = (Option<bool>, Result<Vec<String>, String>);

#[derive(Default)]
pub(super) struct ServerDiscovery {
    pub models: Vec<String>,
    pub error: String,
    pub has_key: bool,
    endpoint: Option<String>,
    worker: Option<mpsc::Receiver<DiscoveryResult>>,
    next: Option<String>,
}

impl ServerDiscovery {
    pub fn loading(&self) -> bool {
        self.worker.is_some()
    }

    pub fn request(&mut self, endpoint: String, wake: &async_channel::Sender<()>) {
        if self.endpoint.as_ref() != Some(&endpoint) {
            self.endpoint = Some(endpoint.clone());
            self.models.clear();
            self.has_key = false;
        }
        self.error.clear();
        if self.worker.is_some() {
            self.next = Some(endpoint);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let wake = wake.clone();
        self.worker = Some(rx);
        thread::spawn(move || {
            let result = match voai_models::has_server_key(&endpoint) {
                Ok(has_key) => (
                    Some(has_key),
                    voai_models::server_models(&endpoint).map_err(|e| e.to_string()),
                ),
                Err(err) => (None, Err(err.to_string())),
            };
            let _ = tx.send(result);
            let _ = wake.try_send(());
        });
    }

    pub fn poll(&mut self, wake: &async_channel::Sender<()>) {
        let Some(worker) = &self.worker else { return };
        let (has_key, result) = match worker.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                (None, Err(tr("app.models.discovery_interrupted").into()))
            }
        };
        self.worker = None;
        if let Some(next) = self.next.take() {
            self.request(next, wake);
            return;
        }
        if let Some(has_key) = has_key {
            self.has_key = has_key;
        }
        match result {
            Ok(list) if list.is_empty() => {
                self.models.clear();
                self.error = tr("app.models.server_empty").into()
            }
            Ok(list) => self.models = list,
            Err(text) => self.error = text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_discovery_failure_preserves_previous_models_and_key_for_the_same_endpoint() {
        let (wake, _) = async_channel::bounded(1);
        let (tx, rx) = mpsc::channel();
        let mut server = ServerDiscovery {
            endpoint: Some("http://127.0.0.1:1/v1".into()),
            models: vec!["model".into()],
            has_key: true,
            worker: Some(rx),
            ..Default::default()
        };
        tx.send((None, Err("Сервер недоступен".into()))).unwrap();
        server.poll(&wake);
        assert_eq!(server.models, ["model"]);
        assert!(server.has_key);
        assert_eq!(server.error, "Сервер недоступен");

        let (_tx, rx) = mpsc::channel();
        server.worker = Some(rx);
        server.request("http://127.0.0.1:2/v1".into(), &wake);
        assert!(server.models.is_empty());
        assert!(!server.has_key);
    }
}
