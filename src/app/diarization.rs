use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use voai_models::{ModelFile, ModelStore};

const KIT: &str = "pyannote3-campplus-v1";
const MODELS: [ModelFile; 2] = [
    ModelFile {
        filename: "segmentation.onnx",
        bytes: 1_540_506,
        sha256: "d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d",
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/9403a6902bb58e3d5ae8c7e77c3422de279db2e0/model.int8.onnx",
    },
    ModelFile {
        filename: "embedding.onnx",
        bytes: 29_596_979,
        sha256: "98daf9bcf1c8abf3db374fec4c27a5eda7c58705bb3f180d057b085a72a88bb5",
        url: "https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/a8055d6daef723a6a4ecf4de233708575bf4994f/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
    },
];

fn artifact_link(model: &ModelFile) -> PathBuf {
    Path::new("../objects")
        .join(model.sha256)
        .join(model.filename)
}

pub(super) fn installed(dir: &Path) -> Option<PathBuf> {
    let store = ModelStore::new(dir.join("objects"));
    let kit = dir.join(KIT);
    MODELS
        .iter()
        .all(|model| {
            store.installed(model)
                && fs::read_link(kit.join(model.filename))
                    .is_ok_and(|link| link == artifact_link(model))
        })
        .then_some(kit)
}

pub(super) fn download(
    dir: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(u8),
) -> Result<PathBuf, String> {
    let store = ModelStore::new(dir.join("objects"));
    let mut done = 0;
    let total: u64 = MODELS.iter().map(|m| m.bytes).sum();
    for model in &MODELS {
        store
            .download(model, cancel, |n| progress(((done + n) * 99 / total) as u8))
            .map_err(|e| e.to_string())?;
        done += model.bytes;
    }
    if cancel.load(Ordering::Acquire) {
        return Err("Загрузка отменена".into());
    }
    let kit = dir.join(KIT);
    publish(dir, &kit).map_err(|_| "Не удалось сохранить комплект моделей".to_owned())?;
    progress(100);
    Ok(kit)
}

fn publish(dir: &Path, kit: &Path) -> std::io::Result<()> {
    let staging = tempfile::tempdir_in(dir)?;
    for model in &MODELS {
        symlink(artifact_link(model), staging.path().join(model.filename))?;
    }
    // New installations expose both names at once. Repair a previous installation
    // using atomic per-name renames; installed() checks both verified objects and links.
    match fs::rename(staging.path(), kit) {
        Ok(()) => (),
        Err(_) if kit.symlink_metadata().is_ok_and(|m| m.is_dir()) => {
            for model in &MODELS {
                fs::rename(
                    staging.path().join(model.filename),
                    kit.join(model.filename),
                )?;
            }
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

pub(super) fn validate(dir: &Path) -> Result<(), String> {
    if !MODELS.iter().all(|model| {
        fs::metadata(dir.join(model.filename)).is_ok_and(|m| m.is_file() && m.len() > 0)
    }) {
        return Err("Выберите папку с файлами segmentation.onnx и embedding.onnx".into());
    }
    // Untrusted local ONNX files are opened only by an isolated helper.
    let launch_error = || "Не удалось запустить проверку моделей".to_owned();
    let exe = std::env::current_exe()
        .map_err(|_| launch_error())?
        .with_file_name("voai-diarize");
    let mut child = Command::new(exe)
        .arg("--validate")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| launch_error())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                return Err(
                    "Нужны совместимые ONNX-модели сегментации и голоса для sherpa-onnx".into(),
                );
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Не удалось проверить модели спикеров".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_starts_no_transfer_and_publishes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(download(tmp.path(), &AtomicBool::new(true), |_| {}).is_err());
        assert!(tmp.path().read_dir().unwrap().next().is_none());
        assert!(installed(tmp.path()).is_none());
    }

    #[test]
    fn aliases_require_both_verified_weights_and_can_be_repaired() {
        let tmp = tempfile::tempdir().unwrap();
        let kit = tmp.path().join(KIT);
        publish(tmp.path(), &kit).unwrap();
        let name = kit.join(MODELS[0].filename);
        fs::remove_file(&name).unwrap();
        publish(tmp.path(), &kit).unwrap();
        assert_eq!(fs::read_link(name).unwrap(), artifact_link(&MODELS[0]));
        assert!(installed(tmp.path()).is_none());
        assert!(validate(&kit).is_err());
    }
}
