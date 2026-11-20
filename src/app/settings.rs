use super::Mode;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use voai_models::{ConnectedModel, ModelKind, ModelStore};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    version: u32,
    pub shortcut: String,
    pub microphone: Option<String>,
    pub language: String,
    pub speech: String,
    pub text: String,
    pub local_models: Vec<ConnectedModel>,
    pub mode: Mode,
    pub processing_prompt: String,
    pub provider: String,
    pub cloud_model: String,
    pub server_url: String,
    pub server_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            shortcut: "Alt+Space".into(),
            microphone: None,
            language: "ru".into(),
            speech: String::new(),
            text: String::new(),
            local_models: Vec::new(),
            mode: Mode::Off,
            processing_prompt: voai_models::DEFAULT_PROCESSING_PROMPT.into(),
            provider: "OpenAI".into(),
            cloud_model: voai_models::Provider::OpenAI.models()[0].into(),
            server_url: "http://127.0.0.1:11434/v1".into(),
            server_model: String::new(),
        }
    }
}

pub(super) fn data_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("VOAI_DATA_DIR") {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            Ok(dir)
        } else {
            Err(io::Error::other(
                "VOAI_DATA_DIR должен содержать абсолютный путь",
            ))
        };
    }
    std::env::home_dir()
        .map(|p| p.join("Library/Application Support/Voai"))
        .ok_or_else(|| io::Error::other("Домашний каталог недоступен"))
}

impl Settings {
    pub fn connect(&mut self, model: ConnectedModel) {
        if let Some(old) = self
            .local_models
            .iter_mut()
            .find(|old| old.path() == model.path() && old.kind == model.kind)
        {
            *old = model;
        } else {
            self.local_models.push(model);
        }
    }

    pub fn disconnect(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .local_models
            .iter()
            .position(|model| model.id() == id)
            .ok_or("Подключение не найдено")?;
        self.local_models.remove(index);
        if self.speech == id {
            self.speech.clear();
        }
        if self.text == id {
            self.text.clear();
        }
        Ok(())
    }

    fn resolve_model(&self, store: &ModelStore, id: &str) -> Result<(ModelKind, PathBuf), String> {
        if let Some(model) = voai_models::find(id) {
            return store
                .installed_path(model)
                .map(|path| (model.kind, path))
                .map_err(|e| e.to_string());
        }
        let model = self
            .local_models
            .iter()
            .find(|model| model.id() == id)
            .ok_or("Скачайте или подключите модель в настройках")?;
        model
            .resolve()
            .map(|path| (model.kind, path.to_owned()))
            .map_err(|e| e.to_string())
    }

    pub fn select_model(&mut self, store: &ModelStore, id: String) -> Result<(), String> {
        match self.resolve_model(store, &id)?.0 {
            ModelKind::Speech => self.speech = id,
            ModelKind::Text => self.text = id,
        }
        Ok(())
    }

    pub fn use_text_model(&mut self, store: &ModelStore, id: String) -> Result<(), String> {
        if self.resolve_model(store, &id)?.0 != ModelKind::Text {
            return Err("Для обработки нужна текстовая модель.".into());
        }
        self.text = id;
        self.mode = Mode::Local;
        Ok(())
    }

    pub fn selected_path(&self, store: &ModelStore, kind: ModelKind) -> Result<PathBuf, String> {
        let id = match kind {
            ModelKind::Speech => &self.speech,
            ModelKind::Text => &self.text,
        };
        let (actual, path) = self.resolve_model(store, id)?;
        if actual != kind {
            return Err("Выберите модель подходящего типа в настройках".into());
        }
        Ok(path)
    }
    pub fn set_processing_prompt(&mut self, prompt: String) -> Result<(), String> {
        voai_models::validate_processing_prompt(&prompt).map_err(|e| e.to_string())?;
        self.processing_prompt = prompt;
        Ok(())
    }

    pub fn reset_processing_prompt(&mut self) {
        self.processing_prompt = voai_models::DEFAULT_PROCESSING_PROMPT.into();
    }

    pub fn load(dir: &Path) -> io::Result<Self> {
        match fs::read(dir.join("settings.json")) {
            Ok(bytes) => {
                let settings: Self = serde_json::from_slice(&bytes)?;
                if settings.version != 1 {
                    return Err(io::Error::other("Неподдерживаемая версия настроек"));
                }
                Ok(settings)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let mut tmp = NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, self)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        tmp.persist(dir.join("settings.json"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_migrates_persists_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.json"),
            r#"{"version":1,"language":"en"}"#,
        )
        .unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert_eq!(
            settings.processing_prompt,
            voai_models::DEFAULT_PROCESSING_PROMPT
        );
        let original_session = settings.clone();
        let prompt = "Исправь пунктуацию.\nСохрани имена.";
        settings.set_processing_prompt(prompt.into()).unwrap();
        assert!(settings.set_processing_prompt("  ".into()).is_err());
        assert!(
            settings
                .set_processing_prompt("ю".repeat(voai_models::MAX_PROMPT_BYTES))
                .is_err()
        );
        assert!(settings.set_processing_prompt("a\0b".into()).is_err());
        settings.save(dir.path()).unwrap();
        assert_eq!(
            Settings::load(dir.path()).unwrap().processing_prompt,
            prompt
        );
        assert_eq!(
            original_session.processing_prompt,
            voai_models::DEFAULT_PROCESSING_PROMPT
        );
        settings.reset_processing_prompt();
        settings.save(dir.path()).unwrap();
        assert_eq!(
            Settings::load(dir.path()).unwrap().processing_prompt,
            voai_models::DEFAULT_PROCESSING_PROMPT
        );
    }

    #[test]
    fn external_files_remain_selectable_after_restart_and_disconnect_only_removes_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().join("Models"));
        fs::write(
            dir.path().join("settings.json"),
            r#"{"version":1,"speech":"base"}"#,
        )
        .unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert!(settings.local_models.is_empty());

        let path = dir.path().join("моя модель.gguf");
        let mut file = b"GGUF".to_vec();
        file.extend(3u32.to_le_bytes());
        file.extend(1u64.to_le_bytes());
        file.extend(1u64.to_le_bytes());
        file.extend(1u64.to_le_bytes());
        file.push(b'x');
        file.extend(2u32.to_le_bytes());
        file.extend(1u16.to_le_bytes());
        file.resize(100, 0);
        fs::write(&path, &file).unwrap();
        let model = ConnectedModel::connect(&path, ModelKind::Text, 1024, |_| Ok(())).unwrap();
        let id = model.id();
        settings.connect(model.clone());
        settings.connect(model);
        assert_eq!(settings.local_models.len(), 1);
        settings.select_model(&store, id.clone()).unwrap();
        assert_eq!(settings.speech, "base");
        settings.save(dir.path()).unwrap();
        let mut after_restart = Settings::load(dir.path()).unwrap();
        assert_eq!(
            after_restart
                .selected_path(&store, ModelKind::Text)
                .unwrap(),
            fs::canonicalize(&path).unwrap()
        );
        after_restart.speech = id.clone();
        assert!(
            after_restart
                .selected_path(&store, ModelKind::Speech)
                .is_err()
        );
        after_restart.speech = "base".into();
        after_restart.disconnect(&id).unwrap();
        assert!(
            after_restart
                .selected_path(&store, ModelKind::Text)
                .is_err()
        );
        assert_eq!(after_restart.speech, "base");
        assert_eq!(fs::read(&path).unwrap(), file);
        assert!(settings.selected_path(&store, ModelKind::Text).is_ok());
        fs::remove_file(&path).unwrap();
        assert!(settings.selected_path(&store, ModelKind::Text).is_err());
    }

    #[test]
    fn round_trip_and_reject_future_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        settings.language = "auto".into();
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()).unwrap().language, "auto");
        fs::write(dir.path().join("settings.json"), r#"{"version":99}"#).unwrap();
        assert!(Settings::load(dir.path()).is_err());
    }
}
