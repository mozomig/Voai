mod gguf;
mod whisper;
use crate::ModelKind;
use anyhow::{Context, Result, ensure};
use ring::digest;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, Metadata},
    io::{BufReader, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// A user-owned file. The application only reads it. The stamp permits an
/// inexpensive recheck after validation, including replacement at the same path.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectedModel {
    path: PathBuf,
    pub kind: ModelKind,
    stamp: FileStamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileStamp {
    fn read(meta: &Metadata) -> Result<Self> {
        ensure!(meta.is_file(), "Выберите обычный файл модели");
        Ok(Self {
            device: meta.dev(),
            inode: meta.ino(),
            bytes: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

impl ConnectedModel {
    /// Validate away from the UI thread. `engine_check` must use the engine for
    /// this category in an isolated process with a deadline.
    pub fn connect(
        path: &Path,
        kind: ModelKind,
        max_bytes: u64,
        engine_check: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<Self> {
        let path = fs::canonicalize(path).context("Не удалось найти файл модели")?;
        ensure!(path.to_str().is_some(), "Путь к модели должен быть в UTF-8");
        let stamp = FileStamp::read(&fs::symlink_metadata(&path)?)?;
        ensure!(
            stamp.bytes <= max_bytes,
            "Файл модели превышает допустимый размер для памяти этого Mac ({} ГБ)",
            max_bytes / 1_000_000_000
        );
        let file = File::open(&path).context("Не удалось прочитать файл модели")?;
        ensure!(
            FileStamp::read(&file.metadata()?)? == stamp,
            "Файл изменился во время подключения"
        );
        check_container(&file, kind, stamp.bytes)?;
        engine_check(&path)?;
        let connection = Self { path, kind, stamp };
        connection.resolve()?;
        Ok(connection)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn bytes(&self) -> u64 {
        self.stamp.bytes
    }

    pub fn id(&self) -> String {
        let hash = digest::digest(&digest::SHA256, self.path.as_os_str().as_encoded_bytes());
        let kind = match self.kind {
            ModelKind::Speech => "speech",
            ModelKind::Text => "text",
        };
        format!("local:{kind}:{}", hex::encode(hash.as_ref()))
    }

    pub fn resolve(&self) -> Result<&Path> {
        let meta = fs::symlink_metadata(&self.path)
            .context("Файл модели недоступен. Проверьте путь и подключение диска")?;
        ensure!(
            FileStamp::read(&meta)? == self.stamp,
            "Файл модели изменился. Подключите его повторно"
        );
        Ok(&self.path)
    }
}

pub fn local_model_size_limit() -> Result<u64> {
    let output = Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()?;
    ensure!(
        output.status.success(),
        "Не удалось определить объём памяти Mac"
    );
    let ram: u64 = std::str::from_utf8(&output.stdout)?.trim().parse()?;
    // Leave unified memory for the OS, decoder context and Metal buffers.
    Ok(ram - ram / 4)
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut bytes = [0; 4];
    reader
        .read_exact(&mut bytes)
        .context("Файл модели обрезан")?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut bytes = [0; 8];
    reader
        .read_exact(&mut bytes)
        .context("Файл модели обрезан")?;
    Ok(u64::from_le_bytes(bytes))
}

fn check_container(file: &File, kind: ModelKind, bytes: u64) -> Result<()> {
    let mut r = BufReader::new(file);
    let magic = read_u32(&mut r)?;
    match kind {
        ModelKind::Text => {
            ensure!(
                magic == u32::from_le_bytes(*b"GGUF"),
                "Для обработки текста выберите файл llama.cpp GGUF"
            );
            gguf::check(&mut r, bytes)?;
        }
        ModelKind::Speech => {
            ensure!(
                magic == u32::from_le_bytes(*b"lmgg"),
                "Для распознавания выберите модель whisper.cpp в формате GGML"
            );
            let mut dims = [0; 13];
            for value in &mut dims {
                *value = read_u32(&mut r)?;
            }
            let [
                vocab,
                audio_ctx,
                width,
                audio_heads,
                audio_layers,
                text_ctx,
                text_width,
                text_heads,
                text_layers,
                mels,
                ftype,
                filter_mels,
                fft,
            ] = dims;
            ensure!(
                (1..=128_000).contains(&vocab)
                    && (1..=8192).contains(&audio_ctx)
                    && (1..=8192).contains(&text_ctx)
                    && (1..=4096).contains(&width)
                    && width == text_width
                    && (1..=128).contains(&audio_heads)
                    && (1..=128).contains(&text_heads)
                    && width.is_multiple_of(audio_heads)
                    && width.is_multiple_of(text_heads)
                    && (1..=64).contains(&audio_layers)
                    && (1..=64).contains(&text_layers)
                    && mels == filter_mels
                    && (1..=256).contains(&mels)
                    && (1..=8192).contains(&fft),
                "Некорректные параметры whisper.cpp"
            );
            r.seek_relative(i64::from(mels) * i64::from(fft) * 4)?;
            let words = read_u32(&mut r)?;
            ensure!(
                words > 0 && words <= vocab,
                "Некорректный словарь whisper.cpp"
            );
            for _ in 0..words {
                let len = read_u32(&mut r)?;
                ensure!(len <= 65_536, "Повреждён словарь whisper.cpp");
                r.seek_relative(i64::from(len))?;
            }
            whisper::check_weights(
                &mut r,
                bytes,
                ftype,
                width,
                vocab,
                audio_layers,
                text_layers,
            )?;
        }
    }
    Ok(())
}

/// The bundled CPU-only validator maps the weights and checks chat-template
/// support. An incompatible native model can terminate only this process.
pub fn validate_text_model(path: &Path) -> Result<()> {
    let helper = std::env::current_exe()?.with_file_name("voai-text-helper");
    let mut child = Command::new(helper)
        .arg("--validate-model-v1")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Не удалось запустить проверку текстовой модели")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                ensure!(
                    status.success(),
                    "llama.cpp не смог открыть модель или применить её шаблон диалога. Выберите совместимую текстовую GGUF-модель"
                );
                return Ok(());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            status => {
                let _ = child.kill();
                let _ = child.wait();
                status.context("Проверка модели прервана")?;
                anyhow::bail!("Время проверки текстовой модели истекло")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gguf() -> Vec<u8> {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(1u64.to_le_bytes());
        bytes.push(b'x');
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.resize(100, 0);
        bytes
    }

    #[test]
    fn linked_models_detect_replacement_and_disconnection_leaves_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("моя модель.gguf");
        fs::write(&path, gguf()).unwrap();
        let first = ConnectedModel::connect(&path, ModelKind::Text, 1024, |_| Ok(())).unwrap();
        let second = ConnectedModel::connect(&path, ModelKind::Text, 1024, |_| Ok(())).unwrap();
        assert_eq!(first.id(), second.id());
        assert_eq!(first.resolve().unwrap(), fs::canonicalize(&path).unwrap());
        let replacement = dir.path().join("replacement");
        fs::write(&replacement, gguf()).unwrap();
        fs::rename(replacement, &path).unwrap();
        assert!(first.resolve().is_err());
        let restored = ConnectedModel::connect(&path, ModelKind::Text, 1024, |_| Ok(())).unwrap();
        assert_eq!(first.id(), restored.id());
        drop(restored);
        assert!(path.is_file());
        fs::remove_file(&path).unwrap();
        assert!(first.resolve().is_err());
    }

    #[test]
    fn rejects_wrong_kind_bad_counts_and_size_before_native_code_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weights");
        fs::write(&path, gguf()).unwrap();
        let unexpected = |_: &Path| -> Result<()> { panic!("invalid file reached native loader") };
        assert!(ConnectedModel::connect(&path, ModelKind::Speech, 1024, unexpected).is_err());
        assert!(ConnectedModel::connect(&path, ModelKind::Text, 3, unexpected).is_err());
        let mut malformed = gguf();
        malformed[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(&path, malformed).unwrap();
        assert!(ConnectedModel::connect(&path, ModelKind::Text, 1024, unexpected).is_err());
    }

    #[test]
    fn rejects_shards_and_oversize_native_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        let unexpected =
            |_: &Path| -> Result<()> { panic!("invalid metadata reached native loader") };
        for (key, value) in [
            (
                "split.count",
                [2u32.to_le_bytes().as_slice(), &2u16.to_le_bytes()].concat(),
            ),
            (
                "unused",
                [
                    9u32.to_le_bytes().as_slice(),
                    &8u32.to_le_bytes(),
                    &u64::MAX.to_le_bytes(),
                ]
                .concat(),
            ),
        ] {
            let mut bytes = gguf();
            bytes.truncate(24);
            bytes.extend((key.len() as u64).to_le_bytes());
            bytes.extend(key.bytes());
            bytes.extend(value);
            bytes.resize(100, 0);
            fs::write(&path, bytes).unwrap();
            assert!(ConnectedModel::connect(&path, ModelKind::Text, 1024, unexpected).is_err());
        }
    }

    #[test]
    fn rejects_models_that_change_during_native_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        fs::write(&path, gguf()).unwrap();
        assert!(
            ConnectedModel::connect(&path, ModelKind::Text, 1024, |path| {
                fs::write(path, b"replaced")?;
                Ok(())
            })
            .is_err()
        );
    }
}
