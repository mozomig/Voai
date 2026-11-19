#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelKind {
    Speech,
    Text,
}

/// All byte counts and digests are the LFS object metadata of the pinned revision.
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: ModelKind,
    pub bytes: u64,
    pub url: &'static str,
    pub sha256: &'static str,
    pub license: &'static str,
    pub(crate) filename: &'static str,
}

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "small",
        name: "Whisper Small",
        description: "Быстрое распознавание на каждый день",
        kind: ModelKind::Speech,
        bytes: 487_601_967,
        filename: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        license: "https://github.com/openai/whisper/blob/main/LICENSE",
    },
    ModelSpec {
        id: "medium",
        name: "Whisper Medium",
        description: "Повышенная точность распознавания",
        kind: ModelKind::Speech,
        bytes: 1_533_763_059,
        filename: "ggml-medium.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-medium.bin",
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        license: "https://github.com/openai/whisper/blob/main/LICENSE",
    },
    ModelSpec {
        id: "large",
        name: "Whisper Large v3 Turbo",
        description: "Высокая точность для сложной речи",
        kind: ModelKind::Speech,
        bytes: 1_624_555_275,
        filename: "ggml-large-v3-turbo.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        license: "https://github.com/openai/whisper/blob/main/LICENSE",
    },
    ModelSpec {
        id: "qwen",
        name: "Qwen2.5 1.5B",
        description: "Лёгкая модель для правки фраз",
        kind: ModelKind::Text,
        bytes: 986_048_768,
        filename: "Qwen2.5-1.5B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-1.5B-Instruct-GGUF/resolve/9eadc66189c7641e1ddd226b8267a9119b2ce2d4/Qwen2.5-1.5B-Instruct-Q4_K_M.gguf",
        sha256: "1adf0b11065d8ad2e8123ea110d1ec956dab4ab038eab665614adba04b6c3370",
        license: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct/blob/main/LICENSE",
    },
    ModelSpec {
        id: "gemma",
        name: "Gemma 2 2B",
        description: "Редактирование длинного текста",
        kind: ModelKind::Text,
        bytes: 1_708_582_752,
        filename: "gemma-2-2b-it-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/gemma-2-2b-it-GGUF/resolve/855f67caed130e1befc571b52bd181be2e858883/gemma-2-2b-it-Q4_K_M.gguf",
        sha256: "e0aee85060f168f0f2d8473d7ea41ce2f3230c1bc1374847505ea599288a7787",
        license: "https://ai.google.dev/gemma/terms",
    },
];

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|model| model.id == id)
}
