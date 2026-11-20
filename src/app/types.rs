use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Off,
    Local,
    Cloud,
    Server,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Listening,
    Finishing,
    Editing,
    Inserting,
    RecoverText,
    RecoverFocus,
    Uncertain,
    Error,
}

#[derive(Clone, PartialEq)]
pub enum ModelState {
    Available,
    Downloading { received: u64 },
    Installed,
    Selected,
    Failed(String),
    Unavailable(String),
}

#[derive(Clone, PartialEq)]
pub struct ModelRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub text_model: bool,
    pub bytes: u64,
    pub on_disk: u64,
    pub local_path: Option<std::path::PathBuf>,
    pub state: ModelState,
}

#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub history: super::history::HistoryView,
    pub phase: Phase,
    pub transcript: String,
    pub elapsed_secs: u64,
    pub level: f32,
    pub message: String,
    pub shortcut: String,
    pub microphone: Option<String>,
    pub microphones: Vec<String>,
    pub language: String,
    pub mode: Mode,
    pub processing_prompt: String,
    pub provider: String,
    pub cloud_model: String,
    pub server_url: String,
    pub server_model: String,
    pub server_models: Vec<String>,
    pub server_loading: bool,
    pub server_error: String,
    pub server_has_key: bool,
    pub has_key: bool,
    pub access_allowed: bool,
    pub mic_allowed: bool,
    pub models: Vec<ModelRow>,
    pub connecting_model: bool,
    pub test_result: Option<(u64, String)>,
}

pub enum Command {
    PlayEntry(i64),
    StopPlayback,
    DeleteEntry(i64),
    CaptureShortcut(bool),
    SetShortcut(String),
    SetMicrophone(Option<String>),
    SetLanguage(String),
    SetMode(Mode),
    SetProcessingPrompt(String),
    ResetProcessingPrompt,
    SetProvider(String),
    SetCloudModel(String),
    DiscoverServer(String),
    SetServerModel(String),
    SaveServerKey(String),
    SaveKey(String),
    Download(String),
    CancelDownload(String),
    SelectModel(String),
    UseTextModel(String),
    ConnectModel {
        path: std::path::PathBuf,
        text_model: bool,
    },
    DisconnectModel(String),
    RequestAccess,
    RequestMicrophone,
    TestFocus(bool),
    TestResultHandled(u64),
    Cancel,
    UseOriginal,
    DismissMessage,
    RefreshDevices,
}
