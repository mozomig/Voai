use super::{ModelRow, ModelState, Settings};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};
use voai_models::{CATALOG, ModelKind, ModelSpec, ModelStore};

struct Job {
    cancel: Arc<AtomicBool>,
    received: Arc<AtomicU64>,
    done: mpsc::Receiver<Result<(), String>>,
}

pub(super) struct Downloads {
    pub store: Arc<ModelStore>,
    jobs: HashMap<&'static str, Job>,
    errors: HashMap<&'static str, (String, u64)>,
    installed: Vec<&'static str>,
}

impl Downloads {
    pub fn new(dir: PathBuf) -> Self {
        let store = Arc::new(ModelStore::new(dir));
        let installed = CATALOG
            .iter()
            .filter(|m| store.installed(m))
            .map(|m| m.id)
            .collect();
        let errors = CATALOG
            .iter()
            .filter(|model| !store.installed(model) && store.partial_bytes(model) > 0)
            .map(|model| {
                (
                    model.id,
                    ("Загрузка прервана".to_string(), store.partial_bytes(model)),
                )
            })
            .collect();
        Self {
            store,
            jobs: HashMap::new(),
            errors,
            installed,
        }
    }

    pub fn start(&mut self, model: &'static ModelSpec) {
        if self.jobs.contains_key(model.id) || self.store.installed(model) {
            return;
        }
        self.installed.retain(|id| *id != model.id);
        self.errors.remove(model.id);
        let store = Arc::clone(&self.store);
        let cancel = Arc::new(AtomicBool::new(false));
        let received = Arc::new(AtomicU64::new(0));
        let (tx, done) = mpsc::channel();
        let flag = Arc::clone(&cancel);
        let progress = Arc::clone(&received);
        thread::spawn(move || {
            let result = store
                .download(model, &flag, |size| progress.store(size, Ordering::Relaxed))
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.jobs.insert(
            model.id,
            Job {
                cancel,
                received,
                done,
            },
        );
    }

    pub fn reconcile(&mut self) {
        self.installed
            .retain(|id| voai_models::find(id).is_some_and(|model| self.store.installed(model)));
    }

    pub fn cancel(&self, id: &str) {
        if let Some(job) = self.jobs.get(id) {
            job.cancel.store(true, Ordering::Release);
        }
    }

    pub fn tick(&mut self) {
        self.jobs.retain(|id, job| {
            let result = match job.done.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => Err("Загрузчик завершил работу".into()),
            };
            match result {
                Ok(()) => self.installed.push(id),
                Err(e) => {
                    let bytes =
                        voai_models::find(id).map_or(0, |model| self.store.partial_bytes(model));
                    self.errors.insert(id, (e, bytes));
                }
            }
            false
        });
    }

    pub fn rows(&self, settings: &Settings) -> Vec<ModelRow> {
        CATALOG
            .iter()
            .map(|model| {
                let state = if let Some(job) = self.jobs.get(model.id) {
                    ModelState::Downloading {
                        received: job.received.load(Ordering::Relaxed),
                    }
                } else if let Some(error) = self.errors.get(model.id) {
                    ModelState::Failed(error.0.clone())
                } else if self.installed.contains(&model.id) {
                    if settings.speech == model.id || settings.text == model.id {
                        ModelState::Selected
                    } else {
                        ModelState::Installed
                    }
                } else {
                    ModelState::Available
                };
                ModelRow {
                    id: model.id.into(),
                    name: model.name.into(),
                    description: model.description.into(),
                    bytes: model.bytes,
                    local_path: None,
                    on_disk: match &state {
                        ModelState::Selected | ModelState::Installed => model.bytes,
                        ModelState::Downloading { received } => *received,
                        ModelState::Failed(_) => self.errors[model.id].1,
                        ModelState::Available | ModelState::Unavailable(_) => 0,
                    },
                    text_model: model.kind == ModelKind::Text,
                    state,
                }
            })
            .collect()
    }
}

impl Drop for Downloads {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Release);
        }
    }
}
