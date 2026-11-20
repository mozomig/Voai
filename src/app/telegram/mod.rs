//! One background worker owns Telegram state, credentials and delivery. Workers
//! are serialized across reconnects and each has a private UI event channel.
mod api;
mod http;
mod state;
#[cfg(test)]
mod tests;
mod worker;

use super::files::BotPipeline;
use api::Api;
use state::State;
use std::{
    fs,
    num::NonZeroU16,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
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
    pub owner: Option<i64>,
    pub awaiting_confirmation: bool,
    pub status: String,
    pub proxy_port: Option<NonZeroU16>,
    pub route_ready: bool,
}

pub enum TelegramCommand {
    SetRoute(Option<NonZeroU16>),
    Connect(String),
    Retry,
    ResendPart,
    ConfirmDelivery,
    Disconnect,
}

enum Event {
    Route(Option<NonZeroU16>),
    Identity {
        username: String,
        owner: Option<i64>,
        link: Option<String>,
    },
    Status(String),
    Paired(i64),
    Unconfirmed {
        part: usize,
        total: usize,
        decision: mpsc::Sender<Result<DeliveryDecision, String>>,
    },
    Pipeline(mpsc::Sender<Result<BotPipeline, String>>),
    End {
        configured: bool,
        result: Result<(), String>,
    },
}

enum DeliveryDecision {
    Resend,
    Confirm,
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
    decision: Option<mpsc::Sender<Result<DeliveryDecision, String>>>,
    disconnecting: bool,
    wake: async_channel::Sender<()>,
    events: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    http: Arc<OnceLock<http::Client>>,
}

enum Operation {
    Restore,
    Connect(Zeroizing<String>, Option<NonZeroU16>),
    SetRoute(Option<NonZeroU16>),
    Disconnect,
}

impl Telegram {
    pub fn new(dir: &Path, wake: async_channel::Sender<()>) -> Self {
        let path = dir.join("telegram.json");
        let mut this = Self {
            view: TelegramView::default(),
            path,
            decision: None,
            disconnecting: false,
            wake,
            events: None,
            cancel: Arc::default(),
            worker: None,
            http: Arc::default(),
        };
        match this.path.try_exists() {
            Ok(true) => this.start(Operation::Restore),
            Ok(false) => {}
            Err(_) => this.view.status = "Не удалось открыть настройки Telegram.".into(),
        }
        this
    }

    pub fn dispatch(&mut self, cmd: TelegramCommand) {
        if self.disconnecting {
            return;
        }
        match cmd {
            TelegramCommand::Connect(token) if !self.view.enabled => self.start(
                Operation::Connect(Zeroizing::new(token), self.view.proxy_port),
            ),
            TelegramCommand::SetRoute(route) if !self.view.enabled => self.view.proxy_port = route,
            TelegramCommand::SetRoute(route)
                if self.view.route_ready && route != self.view.proxy_port =>
            {
                self.start(Operation::SetRoute(route))
            }
            TelegramCommand::Disconnect if self.view.enabled => self.start(Operation::Disconnect),
            TelegramCommand::Retry if self.view.enabled && !self.view.running => {
                self.start(Operation::Restore)
            }
            TelegramCommand::ResendPart => self.resolve(DeliveryDecision::Resend),
            TelegramCommand::ConfirmDelivery => self.resolve(DeliveryDecision::Confirm),
            _ => {}
        }
    }

    fn resolve(&mut self, choice: DeliveryDecision) {
        if let Some(tx) = self.decision.take() {
            let _ = tx.send(Ok(choice));
            self.view.awaiting_confirmation = false;
            self.view.status = "Бот на связи.".into();
        }
    }

    fn start(&mut self, op: Operation) {
        self.stop();
        let previous = self.worker.take();
        self.cancel = Arc::default();
        let (tx, rx) = mpsc::channel();
        let signals = Signals {
            tx,
            wake: self.wake.clone(),
            cancelled: self.cancel.clone(),
        };
        self.events = Some(rx);
        self.disconnecting = matches!(op, Operation::Disconnect);
        self.view = TelegramView {
            enabled: true,
            running: true,
            status: if self.disconnecting {
                "Отключаю бота…"
            } else {
                "Подключение к Telegram…"
            }
            .into(),
            ..Default::default()
        };
        let path = self.path.clone();
        self.http = Arc::default();
        let http = self.http.clone();
        self.worker = Some(thread::spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            // Disconnect must finish even if the user quits while the old worker is unwinding.
            let result = match op {
                Operation::Disconnect => forget_connection(&path),
                _ if !signals.active() => return,
                _ => run_bot(op, &path, &signals, http),
            };
            let _ = signals.event(Event::End {
                configured: path.try_exists().unwrap_or(true),
                result,
            });
        }));
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(http) = self.http.get() {
            http.shutdown();
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
        self.events = None;
        self.decision = None;
    }

    pub fn poll(&mut self, pipeline: impl Fn() -> Result<BotPipeline, String>) {
        let Some(events) = &self.events else { return };
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Identity {
                    username,
                    owner,
                    link,
                } => {
                    self.view.username = Some(username);
                    self.view.owner = owner;
                    self.view.pairing_url = link;
                }
                Event::Paired(owner) => {
                    self.view.owner = Some(owner);
                    self.view.pairing_url = None;
                }
                Event::Route(port) => {
                    self.view.proxy_port = port;
                    self.view.route_ready = true;
                }
                Event::Status(text) => self.view.status = text,
                Event::Pipeline(tx) => {
                    let _ = tx.send(pipeline());
                }
                Event::Unconfirmed {
                    part,
                    total,
                    decision,
                } => {
                    self.view.status = format!(
                        "Telegram не подтвердил доставку. Проверьте в чате часть {part} из {total}."
                    );
                    self.view.awaiting_confirmation = true;
                    self.decision = Some(decision);
                }
                Event::End { configured, result } => {
                    self.disconnecting = false;
                    if !configured {
                        self.view = TelegramView::default();
                    }
                    self.view.running = false;
                    self.view.pairing_url = None;
                    self.view.status = result.err().unwrap_or_default();
                }
            }
        }
    }
}

fn run_bot(
    op: Operation,
    path: &Path,
    signals: &Signals,
    handle: Arc<OnceLock<http::Client>>,
) -> Result<(), String> {
    let (state, secret) = match op {
        Operation::Connect(token, port) => {
            Api::validate_token(&token)?;
            let mut state = State::new();
            state.proxy_port = port;
            state.set_token(token.trim())?;
            if let Err(e) = state.save(path) {
                state.forget()?;
                return Err(e);
            }
            signals.event(Event::Route(state.proxy_port))?;
            (state, token)
        }
        Operation::Restore | Operation::SetRoute(_) => {
            let mut state = State::load(path)?.ok_or("Подключите бота заново.")?;
            if let Operation::SetRoute(port) = op {
                state.proxy_port = port;
                state.save(path)?;
            }
            signals.event(Event::Route(state.proxy_port))?;
            let token = state.token()?;
            (state, token)
        }
        Operation::Disconnect => unreachable!(),
    };
    let http = handle.get_or_init(|| http::Client::new(state.proxy_port));
    if !signals.active() {
        return Ok(());
    }
    let api = Api::new(&secret, http.clone())?;
    worker::serve(signals, &api, state, path, |source| {
        let pipeline = signals.request(Event::Pipeline)?;
        pipeline.transcribe(source, signals.cancelled.clone())
    })
}

fn forget_connection(path: &Path) -> Result<(), String> {
    if let Some(state) = State::load(path)? {
        state.forget()?;
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Не удалось удалить настройки Telegram.".into()),
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
