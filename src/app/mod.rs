mod connections;
mod diarization;
mod downloads;
mod editing;
pub use editing::{EditorSource, ServiceModel};
mod files;
pub use files::{ExportFormat, FileCommand, FileDocument, FileEditor, FileProgress, FileView};
use voai_l10n::{tf, tr};
mod history;
mod hotkeys;
pub use history::Outcome as HistoryOutcome;
mod server;
mod session;
mod settings;
mod telegram;
pub use telegram::{TelegramCommand, TelegramView};
mod types;

use crate::macos;
use connections::Connections;
use downloads::Downloads;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
pub(crate) use hotkeys::right_command;
use hotkeys::{NotifyKey, Shortcuts};
use session::{Destination, Session, provider};
use settings::Settings;
use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};
pub use types::*;

pub struct Backend {
    _instance: File,
    dir: PathBuf,
    settings: Settings,
    session: Session,
    history: history::History,
    files: files::Files,
    telegram: telegram::Telegram,
    downloads: Downloads,
    connections: Connections,
    shortcuts: Option<Shortcuts>,
    init_keys: bool,
    notify_key: NotifyKey,
    held: bool,
    key_events: std::sync::mpsc::Receiver<GlobalHotKeyEvent>,
    wake: async_channel::Receiver<()>,
    notify: async_channel::Sender<()>,
    use_original_pending: bool,
    test_focus: bool,
    devices: Vec<String>,
    device_list: Option<std::sync::mpsc::Receiver<Result<Vec<String>, String>>>,
    message: String,
    has_key: bool,
    server: server::ServerDiscovery,
    mic: bool,
    ax: bool,
    refreshed: Instant,
    last_tick: SystemTime,
}

impl Backend {
    pub fn new() -> Result<Self, String> {
        let dir = settings::data_dir().map_err(|e| e.to_string())?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("instance.lock"))
            .map_err(|e| e.to_string())?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| tr("app.app.already_running").to_string())?;
        let settings =
            Settings::load(&dir).map_err(|e| tf!("app.settings.open_failed", error = e))?;
        settings.interface_language.apply();
        let has_key = if settings.mode == Mode::Cloud {
            provider(&settings.provider)
                .and_then(|p| voai_models::has_api_key(p).map_err(|e| e.to_string()))
                .unwrap_or(false)
        } else {
            false
        };
        let downloads = Downloads::new(dir.join("Models"));

        let (key_sender, key_events) = std::sync::mpsc::channel();
        let (notify, wake) = async_channel::bounded(1);
        let mut connections = Connections::default();
        connections.refresh(&settings.local_models, notify.clone());
        let notify_hotkey = notify.clone();
        let notify_key: NotifyKey = std::sync::Arc::new(move |event| {
            if key_sender.send(event).is_ok() {
                let _ = notify_hotkey.try_send(());
            }
        });
        let carbon = notify_key.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event| carbon(event)));
        let mut server = server::ServerDiscovery::default();
        if settings.mode == Mode::Server {
            server.request(settings.server_url.clone(), &notify);
        }
        let history = history::History::new(dir.join("History"), notify.clone());
        let files = files::Files::new(&dir, notify.clone());
        let telegram = telegram::Telegram::new(&dir, notify.clone());
        let mut backend = Self {
            telegram,
            files,
            history,
            _instance: lock,
            dir,
            settings,
            session: Session::default(),
            downloads,
            connections,
            shortcuts: None,
            init_keys: true,
            notify_key,
            held: false,
            key_events,
            wake,
            notify,
            use_original_pending: false,
            test_focus: false,
            devices: vec![],
            device_list: None,
            message: String::new(),
            has_key,
            server,
            mic: macos::microphone_allowed(),
            ax: macos::trusted(),
            refreshed: Instant::now(),
            last_tick: SystemTime::now(),
        };
        backend.refresh_devices();
        Ok(backend)
    }

    pub fn wake_signal(&self) -> async_channel::Receiver<()> {
        self.wake.clone()
    }

    pub fn widget_anchor(&self) -> Option<objc2_core_foundation::CGRect> {
        self.session.widget_anchor()
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = &self.settings;
        let mut models = self.downloads.rows(s);
        models.extend(self.connections.rows(s));
        Snapshot {
            telegram: self.telegram.view.clone(),
            files: {
                let mut view = self.files.view.clone();
                if view.speech_model.is_empty() {
                    view.speech_model.clone_from(&s.speech);
                }
                view
            },
            history: self.history.view.clone(),
            phase: self.session.phase,
            transcript: self.session.text.clone(),
            elapsed_secs: self.session.elapsed(),
            level: self.session.level,
            message: if self.session.message.is_empty() {
                self.message.clone()
            } else {
                self.session.message.clone()
            },
            onboarding_complete: s.onboarding_complete,
            shortcut: s.shortcut.clone(),
            microphone: s.microphone.clone(),
            noise_suppression: s.noise_suppression,
            compact_widget: s.compact_widget,
            microphones: self.devices.clone(),
            interface_language: s.interface_language,
            language: s.language.clone(),
            mode: s.mode,
            processing_prompt: s.processing_prompt.clone(),
            provider: s.provider.clone(),
            cloud_model: s.cloud_model.clone(),
            agent_profiles: s.agent_profiles.clone(),
            agent_config: voai_models::AgentCli::from_name(&s.provider)
                .map(|agent| s.agent_profiles.get(agent).clone())
                .unwrap_or_default(),
            server_url: s.server_url.clone(),
            server_model: s.server_model.clone(),
            server_models: self.server.models.clone(),
            server_loading: self.server.loading(),
            server_error: self.server.error.clone(),
            server_has_key: self.server.has_key,
            has_key: self.has_key,
            access_allowed: self.ax,
            mic_allowed: self.mic,
            models,
            connecting_model: self.connections.busy(),
            test_result: self.session.result.clone(),
        }
    }

    pub fn dispatch(&mut self, command: Command) {
        self.try_dispatch(command);
    }

    pub fn try_dispatch(&mut self, command: Command) -> bool {
        let result = self.handle(command);
        let succeeded = result.is_ok();
        if let Err(message) = result {
            self.message = message;
        }
        let _ = self.notify.try_send(());
        succeeded
    }

    fn handle(&mut self, command: Command) -> Result<(), String> {
        let refresh_server = matches!(
            command,
            Command::DiscoverServer(_) | Command::SetMode(Mode::Server)
        );
        let mut next = self.settings.clone();
        match command {
            Command::Telegram(cmd) => {
                self.telegram.dispatch(cmd);
                return Ok(());
            }
            Command::File(cmd) => {
                if matches!(cmd, FileCommand::Play(_)) {
                    self.history.send(history::Request::Stop);
                }
                self.files.dispatch(
                    cmd,
                    &self.settings,
                    &self.downloads.store,
                    !matches!(self.session.phase, Phase::Listening | Phase::Finishing),
                );
                return Ok(());
            }
            Command::PlayEntry(id) => {
                self.files.stop();
                if matches!(self.session.phase, Phase::Listening | Phase::Finishing) {
                    return Err(tr("app.dictation.busy").into());
                }
                self.history.send(history::Request::Play(id));
                return Ok(());
            }
            Command::StopPlayback => {
                self.history.send(history::Request::Stop);
                return Ok(());
            }
            Command::DeleteEntry(id) => {
                self.history.send(history::Request::Delete(id));
                return Ok(());
            }
            Command::CaptureShortcut(start) => {
                let keys = self
                    .shortcuts
                    .as_mut()
                    .ok_or(tr("app.shortcuts.enable_failed"))?;
                return if start {
                    if self.session.phase != Phase::Idle {
                        return Err(tr("app.dictation.busy").into());
                    }
                    self.held = false;
                    keys.suspend()
                } else {
                    keys.set(&self.settings.shortcut)
                };
            }
            Command::SetShortcut(s) => {
                if self.session.phase != Phase::Idle {
                    return Err(tr("app.dictation.busy").into());
                }
                self.shortcuts
                    .as_mut()
                    .ok_or(tr("app.shortcuts.enable_failed"))?
                    .set(&s)?;
                next.shortcut = s;
            }
            Command::SetCompactWidget(enabled) => next.compact_widget = enabled,
            Command::SetNoiseSuppression(enabled) => next.noise_suppression = enabled,
            Command::SetMicrophone(device) => next.microphone = device,
            Command::SetInterfaceLanguage(language) => next.set_interface_language(language),
            Command::SetLanguage(lang) => {
                if !matches!(lang.as_str(), "ru" | "en" | "auto") {
                    return Err(tr("app.dictation.language_unavailable").into());
                }
                next.language = lang;
            }
            Command::SetMode(mode) => next.mode = mode,
            Command::DiscoverServer(url) => {
                let url = voai_models::normalize_server_url(&url).map_err(|e| e.to_string())?;
                if url != next.server_url {
                    next.server_model.clear();
                    next.server_url = url;
                }
            }
            Command::SetServerModel(model) => {
                if !self.server.models.contains(&model) {
                    return Err(tr("app.models.refresh_server").into());
                }
                next.server_model = model;
            }
            Command::SaveServerKey(key) => {
                voai_models::set_server_key(&self.settings.server_url, &key)
                    .map_err(|e| e.to_string())?;
                self.server
                    .request(self.settings.server_url.clone(), &self.notify);
                self.message.clear();
                return Ok(());
            }
            Command::SetProcessingPrompt(prompt) => next.set_processing_prompt(prompt)?,
            Command::ResetProcessingPrompt => next.reset_processing_prompt(),
            Command::SetProvider(name) => {
                if voai_models::AgentCli::from_name(&name).is_none() {
                    next.cloud_model = provider(&name)?.models()[0].to_string();
                }
                next.provider = name;
            }
            Command::SetCloudModel(name) => {
                if let Some(agent) = voai_models::AgentCli::from_name(&next.provider) {
                    let mut config = next.agent_profiles.get(agent).clone();
                    config.model = name;
                    config.validate(agent).map_err(|e| e.to_string())?;
                    next.agent_profiles.set(agent, config);
                } else {
                    if !provider(&next.provider)?.models().contains(&name.as_str()) {
                        return Err(tr("app.models.cloud_unknown").into());
                    }
                    next.cloud_model = name;
                }
            }
            Command::ConfigureAgent(agent, config) => {
                if next.mode != Mode::Cloud || next.provider != agent.name() {
                    return Err(tr("agent.provider_changed").into());
                }
                config.validate(agent).map_err(|e| e.to_string())?;
                next.agent_profiles.set(agent, config);
            }
            Command::SaveKey(key) => {
                let p = provider(&next.provider)?;
                voai_models::set_api_key(p, key.trim()).map_err(|e| e.to_string())?;
                self.has_key = !key.trim().is_empty();
                self.message = if self.has_key {
                    tr("app.settings.key_saved")
                } else {
                    tr("app.settings.key_removed")
                }
                .into();
                return Ok(());
            }
            Command::UseTextModel(id) => next.use_text_model(&self.downloads.store, id)?,
            Command::SelectModel(id) => next.select_model(&self.downloads.store, id)?,
            Command::ConnectModel { path, text_model } => {
                let kind = if text_model {
                    voai_models::ModelKind::Text
                } else {
                    voai_models::ModelKind::Speech
                };
                return self.connections.start(path, kind, self.notify.clone());
            }
            Command::DisconnectModel(id) => {
                next.disconnect(&id)?;
                next.save(&self.dir).map_err(|e| e.to_string())?;
                self.settings = next;
                self.connections.forget(&id);
                self.message = tr("app.models.disconnected").into();
                return Ok(());
            }
            Command::Download(id) => {
                self.downloads
                    .start(voai_models::find(&id).ok_or(tr("app.models.unavailable"))?);
                return Ok(());
            }
            Command::CancelDownload(id) => {
                self.downloads.cancel(&id);
                return Ok(());
            }
            Command::CompleteOnboarding => next.onboarding_complete = true,
            Command::RequestAccess => return macos::request_access(),
            Command::RequestMicrophone => {
                return macos::request_microphone();
            }
            Command::TestResultHandled(id) => {
                if self
                    .session
                    .result
                    .as_ref()
                    .is_some_and(|(last, _)| *last == id)
                {
                    self.session.result = None;
                }
                return Ok(());
            }
            Command::TestFocus(focus) => {
                self.test_focus = focus;
                return Ok(());
            }
            Command::Cancel => {
                self.session.reset();
                self.message.clear();
                return Ok(());
            }
            Command::UseOriginal => {
                self.use_original_pending = self.session.phase == Phase::RecoverText;
                return Ok(());
            }
            Command::DismissMessage => {
                self.message.clear();
                if self.session.phase == Phase::Error {
                    self.session.reset();
                }
                return Ok(());
            }
            Command::RefreshDevices => {
                self.refresh_devices();
                return Ok(());
            }
        }
        if let Err(e) = next.save(&self.dir) {
            if next.shortcut != self.settings.shortcut
                && let Some(keys) = &mut self.shortcuts
            {
                let _ = keys.set(&self.settings.shortcut);
            }
            return Err(e.to_string());
        }
        let refresh_key = next.mode == Mode::Cloud
            && (next.mode != self.settings.mode || next.provider != self.settings.provider);
        if self.settings.interface_language != next.interface_language {
            next.interface_language.apply();
        }
        self.settings = next;
        if refresh_server {
            self.server
                .request(self.settings.server_url.clone(), &self.notify);
        }
        self.message.clear();
        if refresh_key {
            self.has_key = match voai_models::AgentCli::from_name(&self.settings.provider) {
                Some(_) => false,
                None => voai_models::has_api_key(provider(&self.settings.provider)?)
                    .map_err(|e| e.to_string())?,
            };
        }
        Ok(())
    }

    fn refresh_devices(&mut self) {
        if !self.mic || self.device_list.is_some() {
            return;
        }
        let (send, receive) = std::sync::mpsc::channel();
        let notify = self.notify.clone();
        self.device_list = Some(receive);
        std::thread::spawn(move || {
            let _ = send.send(voai_speech::input_devices());
            let _ = notify.try_send(());
        });
    }

    fn pressed(&mut self) -> Result<(), String> {
        let recovering = self.session.phase == Phase::RecoverFocus;
        if !matches!(
            self.session.phase,
            Phase::Idle | Phase::RecoverFocus | Phase::Error
        ) {
            return Ok(());
        }
        self.message.clear();
        self.history.send(history::Request::Stop);
        self.files.stop();
        let own_window = macos::app_frontmost();
        let target = if own_window && self.test_focus {
            Destination::Test
        } else if own_window {
            return Err(tr("app.dictation.focus_field").into());
        } else {
            Destination::Field(macos::Destination::capture(self.settings.compact_widget)?)
        };
        if recovering {
            self.session.retarget(target, own_window && self.test_focus);
            return Ok(());
        }
        if !macos::microphone_allowed() {
            return Err(tr("app.dictation.mic_permission").into());
        }
        let path = self
            .settings
            .selected_path(&self.downloads.store, voai_models::ModelKind::Speech)?;
        self.session.start(&self.settings, target, path)
    }

    pub fn tick(&mut self) {
        if self.init_keys {
            self.init_keys = false;
            match Shortcuts::new(self.notify_key.clone()) {
                Ok(mut keys) => {
                    if let Err(e) = keys.set(&self.settings.shortcut) {
                        self.message = e;
                    }
                    self.shortcuts = Some(keys);
                }
                Err(e) => self.message = e,
            }
        }
        let now = SystemTime::now();
        if now.duration_since(self.last_tick).unwrap_or_default() > Duration::from_secs(5)
            && self.session.phase == Phase::Listening
        {
            self.session
                .fail(tr("app.dictation.recording_interrupted").into());
            self.held = false;
        }
        self.last_tick = now;
        while let Ok(event) = self.key_events.try_recv() {
            let Some(keys) = &self.shortcuts else {
                continue;
            };
            if keys.is_cancel(event.id) && event.state == HotKeyState::Pressed {
                self.session.reset();
                continue;
            }
            if !keys.dictation.is_some_and(|key| key.id() == event.id) {
                continue;
            }
            match event.state {
                HotKeyState::Pressed if !self.held => {
                    self.held = true;
                    if let Err(e) = self.pressed() {
                        if self.session.phase == Phase::RecoverFocus {
                            self.session.message = e;
                        } else {
                            self.session.fail(e);
                        }
                    }
                }
                HotKeyState::Released if self.held => {
                    self.held = false;
                    self.session.finish();
                }
                _ => {}
            }
        }
        let trial = self.test_focus && macos::app_frontmost();
        if std::mem::take(&mut self.use_original_pending) {
            self.session.use_original(trial);
        }
        if let Some(entry) = self.session.tick(&self.downloads.store, trial) {
            self.history.send(history::Request::Save(entry));
        }
        self.history.poll();
        self.files.poll();
        self.telegram.poll(|| {
            self.files
                .bot_pipeline(&self.settings, &self.downloads.store)
        });
        if let Some(keys) = &mut self.shortcuts {
            keys.listen_for_cancel(self.session.phase != Phase::Idle);
        }
        self.downloads.tick();
        self.connections.poll_health(&self.settings.local_models);
        if let Some(result) = self.connections.result() {
            match result {
                Ok(model) => {
                    // Merge with current settings: they may have changed during validation.
                    let mut next = self.settings.clone();
                    next.connect(model.clone());
                    match next.save(&self.dir) {
                        Ok(()) => {
                            self.connections.mark_available(&model);
                            self.settings = next;
                            self.message = tr("app.models.connected").into();
                        }
                        Err(e) => self.message = tf!("app.models.save_connection", error = e),
                    }
                }
                Err(error) => self.message = error,
            }
        }
        self.server.poll(&self.notify);
        if let Some(rx) = &self.device_list {
            use std::sync::mpsc::TryRecvError;
            match rx.try_recv() {
                Ok(result) => {
                    self.device_list = None;
                    match result {
                        Ok(devices) => self.devices = devices,
                        Err(message) => self.message = message,
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.device_list = None;
                    self.message = tr("app.audio.devices_refresh").into();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.refreshed.elapsed() >= Duration::from_secs(1) {
            self.downloads.reconcile();
            self.connections
                .refresh(&self.settings.local_models, self.notify.clone());
            self.ax = macos::trusted();
            let allowed = macos::microphone_allowed();
            if allowed != self.mic {
                self.mic = allowed;
                if allowed {
                    self.refresh_devices();
                } else {
                    self.devices.clear();
                    self.device_list = None;
                }
            }
            self.refreshed = Instant::now();
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.session.reset();
    }
}
