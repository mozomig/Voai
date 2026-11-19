use super::{Phase, Settings};
use crate::{insertion::InsertionError, macos};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};
use voai_models::{ModelStore, Provider};
use voai_speech::{Event, Recording};

pub(super) enum Destination {
    Field(macos::Destination),
    Test,
}

pub(super) struct Session {
    pub phase: Phase,
    pub text: String,
    pub message: String,
    pub level: f32,
    pub result: Option<(u64, String)>,
    pub started: Option<Instant>,
    elapsed: u64,
    id: u64,
    target: Option<Destination>,
    recording: Option<Recording>,
    receipt: Option<macos::Receipt>,
    settings: Settings,
    processor: Option<mpsc::Receiver<Result<String, String>>>,
    cancelled: Arc<AtomicBool>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            text: String::new(),
            message: String::new(),
            level: 0.,
            result: None,
            started: None,
            elapsed: 0,
            id: 0,
            target: None,
            recording: None,
            receipt: None,
            settings: Settings::default(),
            processor: None,
            cancelled: Arc::default(),
        }
    }
}

pub(super) fn provider(name: &str) -> Result<Provider, String> {
    match name {
        "OpenAI" => Ok(Provider::OpenAI),
        "Anthropic" => Ok(Provider::Anthropic),
        _ => Err("Неизвестный провайдер".into()),
    }
}

impl Session {
    pub fn elapsed(&self) -> u64 {
        self.started.map_or(self.elapsed, |t| t.elapsed().as_secs())
    }

    pub fn start(
        &mut self,
        settings: &Settings,
        target: Destination,
        model: PathBuf,
    ) -> Result<(), String> {
        let language = (settings.language != "auto").then(|| settings.language.clone());
        let recording = voai_speech::start(model, language, settings.microphone.clone())?;
        self.begin(settings, target, recording);
        Ok(())
    }

    fn begin(&mut self, settings: &Settings, target: Destination, recording: Recording) {
        self.reset();
        self.recording = Some(recording);
        self.target = Some(target);
        self.settings = settings.clone();
        self.phase = Phase::Listening;
        self.started = Some(Instant::now());
    }

    pub fn finish(&mut self) {
        if self.phase == Phase::Listening
            && let Some(recording) = &self.recording
        {
            recording.finish();
            self.elapsed = self.elapsed();
            self.started = None;
            self.level = 0.;
            self.phase = Phase::Finishing;
        }
    }

    pub fn reset(&mut self) {
        if let Some(recording) = self.recording.take() {
            recording.cancel();
        }
        self.cancelled.store(true, Ordering::Release);
        // Each session owns its receiver; late results die with that receiver.
        let next = self.id.wrapping_add(1);
        *self = Self {
            id: next,
            ..Self::default()
        };
    }

    pub fn fail(&mut self, message: String) {
        self.reset();
        self.phase = Phase::Error;
        self.message = message;
    }

    pub fn retarget(&mut self, target: Destination, test_active: bool) {
        if self.phase == Phase::RecoverFocus {
            self.target = Some(target);
            self.deliver(test_active);
        }
    }

    pub fn use_original(&mut self, test_active: bool) {
        if self.phase == Phase::RecoverText {
            self.deliver(test_active);
        }
    }

    fn deliver(&mut self, test_active: bool) {
        self.message.clear();
        let outcome = match self.target.take() {
            Some(Destination::Field(destination)) => destination.deliver(&self.text),
            Some(Destination::Test) if test_active => {
                let text = std::mem::take(&mut self.text);
                self.reset();
                self.result = Some((self.id, text));
                return;
            }
            _ => Err(InsertionError::TargetChanged),
        };
        match outcome {
            Ok(None) => self.reset(),
            Ok(Some(receipt)) => {
                self.receipt = Some(receipt);
                self.phase = Phase::Inserting;
            }
            Err(InsertionError::DeliveryUnknown(e)) => {
                self.phase = Phase::Uncertain;
                self.message = e;
            }
            Err(reason) => {
                self.phase = Phase::RecoverFocus;
                self.message = match reason {
                    InsertionError::Unavailable(e) => e,
                    _ => "Выберите поле и снова нажмите сочетание клавиш.".into(),
                };
            }
        }
    }

    fn process(&mut self, store: &ModelStore, test_active: bool) {
        use super::Mode;
        if self.settings.mode == Mode::Off {
            self.deliver(test_active);
            return;
        }
        let text = self.text.clone();
        let cancel = self.cancelled.clone();
        let settings = self.settings.clone();
        let store = store.clone();
        let (tx, rx) = mpsc::channel();
        self.processor = Some(rx);
        self.phase = Phase::Editing;
        thread::spawn(move || {
            let result = match settings.mode {
                Mode::Off => Ok(text),
                Mode::Local => settings
                    .selected_path(&store, voai_models::ModelKind::Text)
                    .and_then(|p| {
                        voai_models::process_local(&p, &settings.processing_prompt, &text, &cancel)
                            .map_err(|e| e.to_string())
                    }),
                Mode::Server => voai_models::process_server(
                    &settings.server_url,
                    &settings.server_model,
                    &settings.processing_prompt,
                    &text,
                    &cancel,
                )
                .map_err(|e| e.to_string()),
                Mode::Cloud => provider(&settings.provider).and_then(|p| {
                    voai_models::process_cloud(
                        p,
                        &settings.cloud_model,
                        &settings.processing_prompt,
                        &text,
                        &cancel,
                    )
                    .map_err(|e| e.to_string())
                }),
            };
            let _ = tx.send(result);
        });
    }

    pub fn tick(&mut self, store: &ModelStore, test_active: bool) {
        if let Some(receipt) = &self.receipt {
            if let Some(result) = receipt.poll() {
                self.receipt = None;
                match result {
                    Ok(()) => self.reset(),
                    Err(message) => {
                        self.phase = Phase::Uncertain;
                        self.message = message;
                    }
                }
            }
            return;
        }
        while let Some(event) = self.recording.as_ref().and_then(Recording::try_event) {
            match event {
                Event::Level(value) if self.phase == Phase::Listening => self.level = value,
                Event::Partial(text) if self.phase == Phase::Listening => self.text = text,
                Event::Final(text) => {
                    self.recording.take();
                    self.level = 0.;
                    if text.trim().is_empty() {
                        self.reset();
                    } else {
                        self.text = text;
                        self.process(store, test_active);
                    }
                    break;
                }
                Event::Interrupted(reason) if self.phase == Phase::Listening => {
                    self.level = 0.;
                    self.message = reason;
                }
                Event::Error(message) => {
                    self.fail(message);
                    break;
                }
                _ => {}
            }
        }
        if let Some(rx) = &self.processor {
            let response = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err("Редактор завершил работу.".into()),
            };
            self.processor = None;
            match response {
                Ok(text) if !text.trim().is_empty() => {
                    self.text = text;
                    self.deliver(test_active);
                }
                other => {
                    self.phase = Phase::RecoverText;
                    self.message = other
                        .err()
                        .unwrap_or_else(|| "Модель вернула пустой текст.".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editing() -> (Session, mpsc::Sender<Result<String, String>>, ModelStore) {
        let (tx, rx) = mpsc::channel();
        let session = Session {
            phase: Phase::Editing,
            text: "исходный текст".into(),
            target: Some(Destination::Test),
            processor: Some(rx),
            ..Session::default()
        };
        (session, tx, ModelStore::new(PathBuf::new()))
    }

    #[test]
    fn cancellation_detaches_late_processing_result() {
        let (mut session, tx, store) = editing();
        let flag = session.cancelled.clone();
        session.reset();
        assert!(flag.load(Ordering::Acquire));
        assert!(tx.send(Ok("поздний ответ".into())).is_err());
        session.tick(&store, true);
        assert!(session.phase == Phase::Idle && session.result.is_none());
    }

    #[test]
    fn processing_failure_preserves_original_for_explicit_insertion() {
        let (mut session, tx, store) = editing();
        tx.send(Err("timeout".into())).unwrap();
        session.tick(&store, true);
        assert!(session.phase == Phase::RecoverText && session.result.is_none());
        session.use_original(true);
        assert_eq!(session.result.as_ref().unwrap().1, "исходный текст");
    }

    #[test]
    fn focus_loss_preserves_edited_text_until_explicit_retarget() {
        let (mut session, tx, store) = editing();
        tx.send(Ok("Итог.".into())).unwrap();
        session.tick(&store, false);
        assert!(session.phase == Phase::RecoverFocus && session.result.is_none());
        session.retarget(Destination::Test, true);
        assert_eq!(session.result.as_ref().unwrap().1, "Итог.");
        session.retarget(Destination::Test, true);
        assert!(session.phase == Phase::Idle);
    }

    #[test]
    fn uncertain_delivery_is_never_retried_via_recovery_actions() {
        let mut session = Session {
            phase: Phase::Uncertain,
            text: "итог".into(),
            ..Session::default()
        };
        session.retarget(Destination::Test, true);
        session.use_original(true);
        assert!(session.phase == Phase::Uncertain && session.result.is_none());
    }
}

#[cfg(test)]
#[path = "session_qa.rs"]
mod acceptance;
