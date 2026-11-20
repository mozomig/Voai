//! The main thread owns durable settings. Each polling worker has a private event channel:
//! disconnecting invalidates outstanding requests before another worker can use the bot.
mod api;
mod state;
#[cfg(test)]
mod tests;
mod worker;

use super::files::BotPipeline;
use api::Api;
use state::State;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};
use zeroize::Zeroizing;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct TelegramView {
    pub enabled: bool,
    pub running: bool,
    pub username: Option<String>,
    pub pairing_url: Option<String>,
    pub paired: bool,
    pub status: String,
}

pub enum TelegramCommand {
    Connect(String),
    Retry,
    Disconnect,
}

enum Event {
    Identity {
        username: String,
        link: Option<String>,
    },
    Status(String),
    Checkpoint(State, mpsc::Sender<Result<(), String>>),
    Pipeline(mpsc::Sender<Result<BotPipeline, String>>),
    End(Result<(), String>),
}

struct Signals {
    tx: mpsc::Sender<Event>,
    wake: async_channel::Sender<()>,
    cancelled: Arc<AtomicBool>,
}

impl Signals {
    fn active(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
    }
    fn event(&self, event: Event) -> Result<(), String> {
        if !self.active() {
            return Err("Остановлено.".into());
        }
        self.tx.send(event).map_err(|_| "Остановлено.")?;
        let _ = self.wake.try_send(());
        Ok(())
    }
    fn request<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, String>>) -> Event,
    ) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.event(make(tx))?;
        rx.recv().map_err(|_| "Остановлено.")?
    }
    fn status(&self, text: impl Into<String>) -> Result<(), String> {
        self.event(Event::Status(text.into()))
    }
}

pub(super) struct Telegram {
    pub view: TelegramView,
    path: PathBuf,
    state: Option<State>,
    wake: async_channel::Sender<()>,
    events: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Telegram {
    pub fn new(dir: &Path, wake: async_channel::Sender<()>) -> Self {
        let path = dir.join("telegram.json");
        let loaded = State::load(&path);
        let mut this = Self {
            view: TelegramView::default(),
            path,
            state: None,
            wake,
            events: None,
            cancel: Arc::default(),
            worker: None,
        };
        match loaded {
            Ok(Some(state)) => {
                this.state = Some(state);
                this.start();
            }
            Ok(None) => {}
            Err(error) => this.view.status = error,
        }
        this
    }

    pub fn dispatch(&mut self, cmd: TelegramCommand) {
        let result = match cmd {
            TelegramCommand::Connect(token) if self.state.is_none() => {
                self.connect(Zeroizing::new(token))
            }
            TelegramCommand::Disconnect => self.disconnect(),
            TelegramCommand::Retry if !self.view.running => {
                self.start();
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.view.status = error;
        }
    }

    fn connect(&mut self, token: Zeroizing<String>) -> Result<(), String> {
        let token = token.trim();
        Api::new(token)?;
        let state = State::new();
        state.set_token(token)?;
        if let Err(e) = state.save(&self.path) {
            state.forget()?;
            return Err(e);
        }
        self.state = Some(state);
        self.start();
        Ok(())
    }

    fn start(&mut self) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let previous = self.worker.take();
        self.cancel = Arc::default();
        let (tx, rx) = mpsc::channel();
        let signals = Signals {
            tx,
            wake: self.wake.clone(),
            cancelled: self.cancel.clone(),
        };
        self.events = Some(rx);
        self.view = TelegramView {
            enabled: true,
            running: true,
            paired: state.owner.is_some(),
            status: "Подключение к Telegram…".into(),
            ..Default::default()
        };
        self.worker = Some(thread::spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            if signals.active() {
                let result = state
                    .token()
                    .and_then(|secret| Api::new(&secret))
                    .and_then(|api| {
                        worker::serve(&signals, &api, state, |path| {
                            let pipeline = signals.request(Event::Pipeline)?;
                            pipeline.transcribe(path, signals.cancelled.clone())
                        })
                    });
                let _ = signals.event(Event::End(result));
            }
        }));
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
        // Drops queued request reply senders, releasing a worker waiting on the UI.
        self.events = None;
        self.view.running = false;
        self.view.pairing_url = None;
    }

    fn disconnect(&mut self) -> Result<(), String> {
        self.stop();
        if let Some(state) = &self.state {
            state.forget()?;
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("Не удалось удалить настройки Telegram.".into()),
            }
        }
        self.state = None;
        self.view = TelegramView::default();
        Ok(())
    }

    pub fn poll(&mut self, pipeline: impl Fn() -> Result<BotPipeline, String>) {
        let Some(events) = &self.events else { return };
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Identity { username, link } => {
                    self.view.username = Some(username);
                    self.view.pairing_url = link;
                }
                Event::Status(text) => self.view.status = text,
                Event::Checkpoint(state, tx) => {
                    let result = state.save(&self.path);
                    if result.is_ok() {
                        self.view.paired = state.owner.is_some();
                        if self.view.paired {
                            self.view.pairing_url = None;
                        }
                        self.state = Some(state);
                    }
                    let _ = tx.send(result);
                }
                Event::Pipeline(tx) => {
                    let _ = tx.send(pipeline());
                }
                Event::End(result) => {
                    self.view.running = false;
                    self.view.pairing_url = None;
                    self.view.status = result.err().unwrap_or_else(|| "Бот остановлен.".into());
                }
            }
        }
    }
}

impl Drop for Telegram {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
