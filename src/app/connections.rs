use super::{ModelRow, ModelState, Settings};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc,
    thread,
};
use voai_models::{ConnectedModel, ModelKind};

type Validation = Result<ConnectedModel, String>;
type HealthCheck = Vec<(ConnectedModel, Option<String>)>;

#[derive(Default)]
pub(super) struct Connections {
    validation: Option<mpsc::Receiver<Validation>>,
    forgotten_during_validation: HashSet<String>,
    checking: Option<mpsc::Receiver<HealthCheck>>,
    /// A missing value means that this file has yet to be checked after launch.
    health: HashMap<String, Option<String>>,
}

impl Connections {
    pub fn busy(&self) -> bool {
        self.validation.is_some()
    }

    pub fn start(
        &mut self,
        path: PathBuf,
        kind: ModelKind,
        wake: async_channel::Sender<()>,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("Сначала дождитесь проверки текущего файла".into());
        }
        let (tx, rx) = mpsc::channel();
        self.forgotten_during_validation.clear();
        self.validation = Some(rx);
        thread::spawn(move || {
            let validation = voai_models::local_model_size_limit()
                .and_then(|limit| {
                    ConnectedModel::connect(&path, kind, limit, |path| match kind {
                        ModelKind::Speech => {
                            voai_speech::validate_model(path).map_err(std::io::Error::other)?;
                            Ok(())
                        }
                        ModelKind::Text => voai_models::validate_text_model(path),
                    })
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(validation);
            let _ = wake.try_send(());
        });
        Ok(())
    }

    pub fn result(&mut self) -> Option<Validation> {
        let receiver = self.validation.as_ref()?;
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("Проверка модели прервана".to_string()),
        };
        self.validation = None;
        if let Ok(model) = &result
            && self.forgotten_during_validation.contains(&model.id())
        {
            return None;
        }
        Some(result)
    }

    pub fn mark_available(&mut self, model: &ConnectedModel) {
        self.health.insert(model.id(), None);
    }
    pub fn forget(&mut self, id: &str) {
        self.health.remove(id);
        if self.busy() {
            self.forgotten_during_validation.insert(id.to_owned());
        }
    }

    pub fn refresh(&mut self, models: &[ConnectedModel], wake: async_channel::Sender<()>) {
        if self.checking.is_some() || models.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.checking = Some(rx);
        let models = models.to_vec();
        thread::spawn(move || {
            let result = models
                .into_iter()
                .map(|model| {
                    let error = model.resolve().err().map(|e| e.to_string());
                    (model, error)
                })
                .collect();
            let _ = tx.send(result);
            let _ = wake.try_send(());
        });
    }
    pub fn poll_health(&mut self, models: &[ConnectedModel]) {
        let Some(rx) = self.checking.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(results) => {
                // A reconnected or forgotten file can supersede this probe in flight.
                for (model, error) in results {
                    if models.contains(&model) {
                        self.health.insert(model.id(), error);
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
        self.checking = None;
    }

    pub fn rows(&self, settings: &Settings) -> Vec<ModelRow> {
        settings
            .local_models
            .iter()
            .map(|model| {
                let id = model.id();
                let state = match self.health.get(&id) {
                    None => ModelState::Unavailable("Проверяем доступ к файлу…".into()),
                    Some(Some(error)) => ModelState::Unavailable(error.clone()),
                    Some(None) if id == settings.speech || id == settings.text => {
                        ModelState::Selected
                    }
                    Some(None) => ModelState::Installed,
                };
                ModelRow {
                    id,
                    name: model
                        .path()
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    description: model.path().display().to_string(),
                    text_model: model.kind == ModelKind::Text,
                    bytes: model.bytes(),
                    on_disk: 0,
                    local_path: Some(model.path().to_owned()),
                    state,
                }
            })
            .collect()
    }
}
