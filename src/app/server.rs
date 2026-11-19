//! Serializes local-server discovery and publishes only the most recent request.
use std::{sync::mpsc, thread};

type DiscoveryResult = (bool, Result<Vec<String>, String>);

#[derive(Default)]
pub(super) struct ServerDiscovery {
    pub models: Vec<String>,
    pub error: String,
    pub has_key: bool,
    worker: Option<mpsc::Receiver<DiscoveryResult>>,
    next: Option<String>,
}

impl ServerDiscovery {
    pub fn loading(&self) -> bool {
        self.worker.is_some()
    }

    pub fn request(&mut self, endpoint: String, wake: &async_channel::Sender<()>) {
        self.models.clear();
        self.error.clear();
        self.has_key = false;
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
                    has_key,
                    voai_models::server_models(&endpoint).map_err(|e| e.to_string()),
                ),
                Err(err) => (false, Err(err.to_string())),
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
            Err(mpsc::TryRecvError::Disconnected) => (false, Err("Поиск моделей прерван".into())),
        };
        self.worker = None;
        if let Some(next) = self.next.take() {
            self.request(next, wake);
            return;
        }
        self.has_key = has_key;
        match result {
            Ok(list) if list.is_empty() => {
                self.error = "Сервер доступен. Добавьте в него модель.".into()
            }
            Ok(list) => self.models = list,
            Err(text) => self.error = text,
        }
    }
}
