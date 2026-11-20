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
use voai_l10n::tr;
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
        let mut downloads = Self {
            store,
            jobs: HashMap::new(),
            errors: HashMap::new(),
            installed: Vec::new(),
        };
        downloads.reconcile();
        downloads
    }

    pub fn start(&mut self, model: &'static ModelSpec) {
        if self.jobs.contains_key(model.id) {
            return;
        }
        if self.store.installed(model) {
            self.reconcile();
            return;
        }
        self.installed.retain(|id| *id != model.id);
        self.errors.remove(model.id);
        let store = Arc::clone(&self.store);
        let cancel = Arc::new(AtomicBool::new(false));
        let received = Arc::new(AtomicU64::new(self.store.partial_bytes(model)));
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
        self.installed.clear();
        for model in CATALOG {
            if self.jobs.contains_key(model.id) {
                continue;
            }
            if self.store.installed(model) {
                self.installed.push(model.id);
                self.errors.remove(model.id);
            } else {
                let bytes = self.store.partial_bytes(model);
                if let Some(error) = self.errors.get_mut(model.id) {
                    error.1 = bytes;
                } else if bytes > 0 {
                    self.errors
                        .insert(model.id, (tr("app.downloads.interrupted").into(), bytes));
                }
            }
        }
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
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(tr("app.downloads.interrupted").into())
                }
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
                let selected = settings.speech == model.id || settings.text == model.id;
                let state = if let Some(job) = self.jobs.get(model.id) {
                    ModelState::Downloading {
                        received: job.received.load(Ordering::Relaxed),
                    }
                } else if let Some(error) = self.errors.get(model.id) {
                    ModelState::Failed(error.0.clone())
                } else if self.installed.contains(&model.id) {
                    if selected {
                        ModelState::Selected
                    } else {
                        ModelState::Installed
                    }
                } else if selected {
                    ModelState::Failed(tr("app.models.file_missing").into())
                } else {
                    ModelState::Available
                };
                ModelRow {
                    id: model.id.into(),
                    name: model.name.into(),
                    description: model.description().into(),
                    bytes: model.bytes,
                    local_path: None,
                    on_disk: match &state {
                        ModelState::Selected | ModelState::Installed => model.bytes,
                        ModelState::Downloading { received } => *received,
                        ModelState::Failed(_) => {
                            self.errors.get(model.id).map_or(0, |error| error.1)
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn catalog_recovers_selected_install_after_the_model_folder_returns() {
        let tmp = tempfile::tempdir().unwrap();
        let model = &CATALOG[0];
        let subdir = tmp.path().join(model.sha256);
        let mut downloads = Downloads::new(tmp.path().to_owned());
        let mut settings = Settings::default();
        settings.speech = model.id.to_owned();
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("download.part"), b"abc").unwrap();
        downloads.reconcile();
        assert_eq!(downloads.rows(&settings)[0].on_disk, 3);

        // Simulate mount/unmount of a pre-existing immutable catalog cache.
        // A sparse fixture only tests discovery of the legacy receipt.
        let name = model.url.rsplit('/').next().unwrap();
        let file = fs::File::create(subdir.join(name)).unwrap();
        file.set_len(model.bytes).unwrap();
        let time = file
            .metadata()
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        fs::write(
            subdir.join("verified"),
            format!("{} {} {time}", model.sha256, model.bytes),
        )
        .unwrap();
        fs::remove_file(subdir.join("download.part")).unwrap();
        downloads.reconcile();
        assert!(downloads.rows(&settings)[0].state == ModelState::Selected);

        let offline = tmp.path().join("offline");
        fs::rename(&subdir, &offline).unwrap();
        downloads.reconcile();
        assert!(matches!(
            downloads.rows(&settings)[0].state,
            ModelState::Failed(_)
        ));
        assert_eq!(downloads.rows(&settings)[0].on_disk, 0);
        fs::rename(offline, subdir).unwrap();
        downloads.reconcile();
        assert!(downloads.rows(&settings)[0].state == ModelState::Selected);
    }
}
