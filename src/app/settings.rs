use super::Mode;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use voai_l10n::tr;
use voai_models::{ConnectedModel, ModelKind, ModelStore};

const LEGACY_PROCESSING_PROMPT: &str = "Отредактируй фразу как корректор. Убери слова-паразиты и случайные повторы. Начни предложение с заглавной буквы, заверши подходящим знаком. Сохрани исходный язык, слова, обращение на ты или вы и смысл. Верни только исправленную фразу.";

fn is_default_prompt(prompt: &str) -> bool {
    ["en", "ru"]
        .into_iter()
        .any(|locale| prompt == voai_l10n::tr_for(locale, "prompt.default"))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    version: u32,
    #[serde(default = "existing_profile")]
    pub onboarding_complete: bool,
    pub shortcut: String,
    pub microphone: Option<String>,
    pub noise_suppression: bool,
    pub compact_widget: bool,
    pub interface_language: voai_l10n::InterfaceLanguage,
    #[serde(default = "legacy_speech_language")]
    pub language: String,
    pub speech: String,
    pub text: String,
    pub local_models: Vec<ConnectedModel>,
    pub mode: Mode,
    #[serde(skip_serializing_if = "is_default_prompt")]
    pub processing_prompt: String,
    pub provider: String,
    pub cloud_model: String,
    pub agent_profiles: voai_models::AgentProfiles,
    pub server_url: String,
    pub server_model: String,
}

// Profiles saved before the introduction already went through manual setup.
fn existing_profile() -> bool {
    true
}

fn legacy_speech_language() -> String {
    "ru".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 2,
            onboarding_complete: false,
            shortcut: "Alt+Space".into(),
            microphone: None,
            noise_suppression: false,
            compact_widget: false,
            interface_language: Default::default(),
            language: "auto".into(),
            speech: String::new(),
            text: String::new(),
            local_models: Vec::new(),
            mode: Mode::Off,
            processing_prompt: voai_l10n::tr_for(
                voai_l10n::InterfaceLanguage::System.resolve(),
                "prompt.default",
            )
            .into(),
            provider: "OpenAI".into(),
            agent_profiles: Default::default(),
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
            Err(io::Error::other(tr("app.settings.absolute_data_dir")))
        };
    }
    std::env::home_dir()
        .map(|p| p.join("Library/Application Support/Voai"))
        .ok_or_else(|| io::Error::other(tr("app.settings.home_missing")))
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
            .ok_or(tr("app.models.connection_missing"))?;
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
            .ok_or(tr("app.models.choose_first"))?;
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
            return Err(tr("app.models.need_text").into());
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
        self.path_for(store, id, kind)
    }
    pub fn path_for(
        &self,
        store: &ModelStore,
        id: &str,
        kind: ModelKind,
    ) -> Result<PathBuf, String> {
        let (actual, path) = self.resolve_model(store, id)?;
        if actual != kind {
            return Err(tr("app.models.wrong_type").into());
        }
        Ok(path)
    }
    pub fn set_processing_prompt(&mut self, prompt: String) -> Result<(), String> {
        voai_models::validate_processing_prompt(&prompt).map_err(|e| e.to_string())?;
        self.processing_prompt = prompt;
        Ok(())
    }

    pub fn set_interface_language(&mut self, language: voai_l10n::InterfaceLanguage) {
        self.interface_language = language;
        if is_default_prompt(&self.processing_prompt) {
            self.reset_processing_prompt();
        }
    }

    pub fn reset_processing_prompt(&mut self) {
        self.processing_prompt =
            voai_l10n::tr_for(self.interface_language.resolve(), "prompt.default").into();
    }

    pub fn load(dir: &Path) -> io::Result<Self> {
        match fs::read(dir.join("settings.json")) {
            Ok(bytes) => {
                let mut settings: Self = serde_json::from_slice(&bytes)?;
                match settings.version {
                    1 => {
                        if settings.processing_prompt == LEGACY_PROCESSING_PROMPT {
                            settings.reset_processing_prompt();
                        }
                        settings.version = 2;
                    }
                    2 => {}
                    _ => return Err(io::Error::other(tr("app.settings.version_unsupported"))),
                }
                if is_default_prompt(&settings.processing_prompt) {
                    settings.reset_processing_prompt();
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
    fn introduction_is_pending_only_for_unfinished_new_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert!(!settings.onboarding_complete);
        settings.save(dir.path()).unwrap();
        assert!(!Settings::load(dir.path()).unwrap().onboarding_complete);
        settings.onboarding_complete = true;
        settings.save(dir.path()).unwrap();
        assert!(Settings::load(dir.path()).unwrap().onboarding_complete);
        for version in [1, 2] {
            fs::write(
                dir.path().join("settings.json"),
                format!(r#"{{"version":{version}}}"#),
            )
            .unwrap();
            assert!(Settings::load(dir.path()).unwrap().onboarding_complete);
        }
    }

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
            voai_l10n::tr_for(
                voai_l10n::InterfaceLanguage::System.resolve(),
                "prompt.default"
            )
        );
        fs::write(
            dir.path().join("settings.json"),
            serde_json::to_vec(
                &serde_json::json!({ "version": 1, "processing_prompt": LEGACY_PROCESSING_PROMPT }),
            )
            .unwrap(),
        )
        .unwrap();
        settings = Settings::load(dir.path()).unwrap();
        assert_eq!(
            settings.processing_prompt,
            voai_l10n::tr_for(
                voai_l10n::InterfaceLanguage::System.resolve(),
                "prompt.default"
            )
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
            voai_l10n::tr_for(
                voai_l10n::InterfaceLanguage::System.resolve(),
                "prompt.default"
            )
        );
        settings.reset_processing_prompt();
        settings.save(dir.path()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
        assert!(saved.get("processing_prompt").is_none());
        assert_eq!(
            Settings::load(dir.path()).unwrap().processing_prompt,
            voai_l10n::tr_for(
                voai_l10n::InterfaceLanguage::System.resolve(),
                "prompt.default"
            )
        );
    }

    #[test]
    fn changing_interface_language_keeps_speech_and_custom_prompt() {
        use voai_l10n::InterfaceLanguage::{English, Russian, System};
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), r#"{"version":2}"#).unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert_eq!(settings.interface_language, System);
        assert_eq!(settings.language, "ru");
        settings.set_interface_language(English);
        settings.save(dir.path()).unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert_eq!(settings.interface_language, English);
        assert_eq!(
            settings.processing_prompt,
            voai_l10n::tr_for("en", "prompt.default")
        );
        settings
            .set_processing_prompt("Keep punctuation as dictated.".into())
            .unwrap();
        settings.set_interface_language(Russian);
        settings.save(dir.path()).unwrap();
        let mut settings = Settings::load(dir.path()).unwrap();
        assert_eq!(settings.language, "ru");
        assert_eq!(settings.processing_prompt, "Keep punctuation as dictated.");
        settings.reset_processing_prompt();
        assert_eq!(
            settings.processing_prompt,
            voai_l10n::tr_for("ru", "prompt.default")
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
        assert!(!settings.noise_suppression);

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
        assert!(!settings.noise_suppression);
        settings.noise_suppression = true;
        settings.language = "auto".into();
        settings.save(dir.path()).unwrap();
        let saved = Settings::load(dir.path()).unwrap();
        assert_eq!(saved.language, "auto");
        assert!(saved.noise_suppression);
        fs::write(dir.path().join("settings.json"), r#"{"version":99}"#).unwrap();
        assert!(Settings::load(dir.path()).is_err());
    }
}
