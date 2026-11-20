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
    description_key: &'static str,
    pub kind: ModelKind,
    pub license: &'static str,
    pub file: ModelFile,
}

/// Installation metadata shared by selectable models and multi-file model packs.
#[derive(Clone, Copy, Debug)]
pub struct ModelFile {
    pub bytes: u64,
    pub url: &'static str,
    pub sha256: &'static str,
    pub filename: &'static str,
}

impl ModelSpec {
    pub fn description(&self) -> &'static str {
        voai_l10n::tr(self.description_key)
    }
}

impl std::ops::Deref for ModelSpec {
    type Target = ModelFile;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "small",
        name: "Whisper Small",
        description_key: "catalog.small",
        kind: ModelKind::Speech,
        license: "https://github.com/openai/whisper/blob/main/LICENSE",
        file: ModelFile {
            bytes: 487_601_967,
            filename: "ggml-small.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin",
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        },
    },
    ModelSpec {
        id: "medium",
        name: "Whisper Medium",
        description_key: "catalog.medium",
        kind: ModelKind::Speech,
        license: "https://github.com/openai/whisper/blob/main/LICENSE",
        file: ModelFile {
            bytes: 1_533_763_059,
            filename: "ggml-medium.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-medium.bin",
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        },
    },
    ModelSpec {
        id: "large",
        name: "Whisper Large v3 Turbo",
        description_key: "catalog.large",
        kind: ModelKind::Speech,
        license: "https://github.com/openai/whisper/blob/main/LICENSE",
        file: ModelFile {
            bytes: 1_624_555_275,
            filename: "ggml-large-v3-turbo.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        },
    },
    ModelSpec {
        id: "gigaam-v3",
        name: "GigaAM v3 E2E",
        description_key: "catalog.gigaam-v3",
        kind: ModelKind::Speech,
        license: "https://huggingface.co/ai-sage/GigaAM-v3",
        file: ModelFile {
            bytes: 272_151_136,
            filename: "gigaam-v3-e2e-ctc-Q8_0.gguf",
            url: "https://huggingface.co/handy-computer/gigaam-v3-e2e-ctc-gguf/resolve/59717de63d50dd9dd93b3249da6ad3f2f1c4ad6b/gigaam-v3-e2e-ctc-Q8_0.gguf",
            sha256: "9ccce4750dc813a493d96ca15ee251712bedec15ac9a02fa3d2bd732f08ae5eb",
        },
    },
    ModelSpec {
        id: "parakeet-v3",
        name: "Parakeet TDT 0.6B v3",
        description_key: "catalog.parakeet-v3",
        kind: ModelKind::Speech,
        license: "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
        file: ModelFile {
            bytes: 739_508_576,
            filename: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
            url: "https://huggingface.co/handy-computer/parakeet-tdt-0.6b-v3-gguf/resolve/d8d8c32cb2017c57e5307767343e97870f8a7f53/parakeet-tdt-0.6b-v3-Q8_0.gguf",
            sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7",
        },
    },
    ModelSpec {
        id: "gemma4-e4b",
        name: "Gemma 4 E4B",
        description_key: "catalog.gemma4-e4b",
        kind: ModelKind::Text,
        license: "https://huggingface.co/google/gemma-4-E4B-it-qat-q4_0-gguf",
        file: ModelFile {
            bytes: 5_154_941_280,
            filename: "gemma-4-E4B_q4_0-it.gguf",
            url: "https://huggingface.co/google/gemma-4-E4B-it-qat-q4_0-gguf/resolve/4b4a2c1d584be7264f87aac328a1bc739ce81b6c/gemma-4-E4B_q4_0-it.gguf",
            sha256: "676c35070db6dbe52f93e9c864ee0fba4eddea94b9c875d9cb10daff453fbaee",
        },
    },
    ModelSpec {
        id: "qwen3-4b",
        name: "Qwen3 4B Instruct 2507",
        description_key: "catalog.qwen3-4b",
        kind: ModelKind::Text,
        license: "https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507",
        file: ModelFile {
            bytes: 2_497_280_448,
            filename: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            url: "https://huggingface.co/lmstudio-community/Qwen3-4B-Instruct-2507-GGUF/resolve/4edb920b6f14e3b9284d4502a6485103d72cde05/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            sha256: "8cdb57cbb880d313736a9bc4e3d3d2485f145b5e19cf33783746e753e82641fc",
        },
    },
    ModelSpec {
        id: "qwen",
        name: "Qwen2.5 1.5B",
        description_key: "catalog.qwen",
        kind: ModelKind::Text,
        license: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct/blob/main/LICENSE",
        file: ModelFile {
            bytes: 986_048_768,
            filename: "Qwen2.5-1.5B-Instruct-Q4_K_M.gguf",
            url: "https://huggingface.co/bartowski/Qwen2.5-1.5B-Instruct-GGUF/resolve/9eadc66189c7641e1ddd226b8267a9119b2ce2d4/Qwen2.5-1.5B-Instruct-Q4_K_M.gguf",
            sha256: "1adf0b11065d8ad2e8123ea110d1ec956dab4ab038eab665614adba04b6c3370",
        },
    },
    ModelSpec {
        id: "gemma",
        name: "Gemma 2 2B",
        description_key: "catalog.gemma",
        kind: ModelKind::Text,
        license: "https://ai.google.dev/gemma/terms",
        file: ModelFile {
            bytes: 1_708_582_752,
            filename: "gemma-2-2b-it-Q4_K_M.gguf",
            url: "https://huggingface.co/bartowski/gemma-2-2b-it-GGUF/resolve/855f67caed130e1befc571b52bd181be2e858883/gemma-2-2b-it-Q4_K_M.gguf",
            sha256: "e0aee85060f168f0f2d8473d7ea41ce2f3230c1bc1374847505ea599288a7787",
        },
    },
];

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|model| model.id == id)
}
