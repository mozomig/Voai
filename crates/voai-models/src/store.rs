use crate::{ModelSpec, check_cancelled};
use anyhow::{Context, Result, ensure};
use ring::digest;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn dir(&self, model: &ModelSpec) -> PathBuf {
        self.root.join(model.sha256)
    }

    pub fn installed_path(&self, model: &ModelSpec) -> Result<PathBuf> {
        let path = self.dir(model).join(model.filename);
        let metadata = path.symlink_metadata().context("Модель не установлена")?;
        ensure!(
            metadata.is_file() && metadata.len() == model.bytes,
            "Файл модели повреждён"
        );
        let stamp =
            fs::read_to_string(self.dir(model).join("verified")).context("Модель не проверена")?;
        ensure!(
            stamp == verified_stamp(&metadata, model)?,
            "Файл модели изменился после проверки"
        );
        Ok(path)
    }

    pub fn installed(&self, model: &ModelSpec) -> bool {
        self.installed_path(model).is_ok()
    }

    pub fn partial_bytes(&self, model: &ModelSpec) -> u64 {
        fs::metadata(self.dir(model).join("download.part")).map_or(0, |m| m.len().min(model.bytes))
    }

    /// The byte identity comes from the pinned revision and SHA-256. A server
    /// ignoring Range causes a restart from zero. No partial file is published.
    pub fn download(
        &self,
        model: &ModelSpec,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64),
    ) -> Result<PathBuf> {
        check_cancelled(cancel)?;
        if let Ok(path) = self.installed_path(model) {
            progress(model.bytes);
            return Ok(path);
        }
        let dir = self.dir(model);
        fs::create_dir_all(&dir).context("Не удалось создать каталог модели")?;
        // The permanent lock file is deliberately not unlinked. Recreating the inode
        // while another process waits for the lock could grant two concurrent writers.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("install.lock"))?;
        lock.try_lock()
            .context("Модель уже загружается другим процессом")?;
        if let Ok(path) = self.installed_path(model) {
            progress(model.bytes);
            return Ok(path);
        }
        let part = dir.join("download.part");
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&part)?;
        let mut offset = output.seek(SeekFrom::End(0))?;
        if offset > model.bytes {
            output.set_len(0)?;
            output.rewind()?;
            offset = 0;
        }
        progress(offset);
        if offset < model.bytes {
            let agent = ureq::Agent::config_builder()
                .https_only(true)
                .timeout_connect(Some(Duration::from_secs(10)))
                .timeout_recv_response(Some(Duration::from_secs(20)))
                .timeout_per_read(Some(Duration::from_secs(15)))
                .build()
                .new_agent();
            let mut request = agent.get(model.url).header("Accept-Encoding", "identity");
            if offset > 0 {
                request = request.header("Range", format!("bytes={offset}-"));
            }
            let mut response = request.call().map_err(|err| {
                anyhow::anyhow!("Не удалось скачать модель ({})", http_failure(&err))
            })?;
            match response.status().as_u16() {
                206 => {
                    let expected = format!("bytes {offset}-{}/{}", model.bytes - 1, model.bytes);
                    ensure!(
                        response
                            .headers()
                            .get("Content-Range")
                            .and_then(|v| v.to_str().ok())
                            == Some(&expected),
                        "Сервер вернул неверный диапазон модели"
                    );
                }
                200 => {
                    output.set_len(0)?;
                    output.rewind()?;
                    offset = 0;
                    progress(0);
                }
                code => anyhow::bail!("Сервис загрузки вернул HTTP {code}"),
            }
            transfer(
                response.body_mut().as_reader(),
                &mut output,
                offset,
                model.bytes,
                cancel,
                &mut progress,
            )?;
        }
        output.sync_all()?;
        output.rewind()?;
        if !verify(&mut output, model, cancel)? {
            drop(output);
            fs::remove_file(&part)?;
            anyhow::bail!("Контрольная сумма модели не совпала. Повторите загрузку");
        }
        check_cancelled(cancel)?;
        let installed = dir.join(model.filename);
        let mut receipt = File::create(dir.join("verified.next"))?;
        receipt.write_all(verified_stamp(&output.metadata()?, model)?.as_bytes())?;
        receipt.sync_all()?;
        fs::rename(dir.join("verified.next"), dir.join("verified"))?;
        fs::rename(part, &installed)?;
        File::open(dir)?.sync_all()?;
        Ok(installed)
    }
}

// A cheap startup check ties the SHA-verified artifact to its last modification
// time. Hashing a gigabyte of weights on the UI thread would stall each launch.
fn verified_stamp(metadata: &fs::Metadata, model: &ModelSpec) -> Result<String> {
    let modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    Ok(format!("{} {} {modified}", model.sha256, model.bytes))
}

// Keep signed redirect URLs, provider bodies and transcript contents out of diagnostics.
pub(crate) fn http_failure(error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        ureq::Error::Timeout(_) => "истекло время ожидания".into(),
        _ => "ошибка соединения".into(),
    }
}

fn transfer(
    mut input: impl Read,
    out: &mut impl Write,
    mut count: u64,
    bytes: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<()> {
    let mut buf = vec![0; 128 * 1024];
    loop {
        check_cancelled(cancel)?;
        let n = input.read(&mut buf).context("Загрузка прервана")?;
        if n == 0 {
            break;
        }
        count += n as u64;
        ensure!(
            count <= bytes,
            "Сервер прислал больше данных, чем ожидалось"
        );
        out.write_all(&buf[..n])
            .context("Не удалось сохранить веса на диске")?;
        progress(count);
    }
    ensure!(
        count == bytes,
        "Загрузка прервана. Скачано {count} из {bytes} байт"
    );
    Ok(())
}

fn verify(input: &mut impl Read, model: &ModelSpec, cancel: &AtomicBool) -> Result<bool> {
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut buf = vec![0; 256 * 1024];
    loop {
        check_cancelled(cancel)?;
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex::encode(hash.finish().as_ref()) == model.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CATALOG, ModelSpec};
    use std::sync::atomic::Ordering;

    fn fixture() -> ModelSpec {
        ModelSpec {
            bytes: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ..CATALOG[0]
        }
    }
    #[test]
    fn completes_interrupted_download_after_digest_check() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().into());
        let model = fixture();
        fs::create_dir_all(store.dir(&model)).unwrap();
        fs::write(store.dir(&model).join("download.part"), b"abc").unwrap();
        assert!(!store.installed(&model));
        store
            .download(&model, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(
            fs::read(store.installed_path(&model).unwrap()).unwrap(),
            b"abc"
        );
        assert_eq!(store.partial_bytes(&model), 0);
    }
    #[test]
    fn refuses_a_modified_installed_file_with_the_same_length() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().into());
        let model = fixture();
        fs::create_dir_all(store.dir(&model)).unwrap();
        fs::write(store.dir(&model).join("download.part"), b"abc").unwrap();
        let path = store
            .download(&model, &AtomicBool::new(false), |_| {})
            .unwrap();
        let mut out = File::create(path).unwrap();
        out.write_all(b"bad").unwrap();
        out.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        assert!(!store.installed(&model));
    }

    #[test]
    fn rejects_bad_digest_and_clears_bad_partial() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().into());
        let model = fixture();
        fs::create_dir_all(store.dir(&model)).unwrap();
        fs::write(store.dir(&model).join("download.part"), b"bad").unwrap();
        assert!(
            store
                .download(&model, &AtomicBool::new(false), |_| {})
                .is_err()
        );
        assert!(!store.installed(&model));
        assert_eq!(store.partial_bytes(&model), 0);
    }
    #[test]
    fn interrupted_or_excess_bytes_never_report_complete() {
        let stop = AtomicBool::new(false);
        let mut out = Vec::new();
        assert!(transfer(&b"bc"[..], &mut out, 1, 5, &stop, &mut |_| {}).is_err());
        assert_eq!(out, b"bc");
        out.clear();
        assert!(transfer(&b"bc"[..], &mut out, 2, 3, &stop, &mut |_| {}).is_err());
        assert!(out.is_empty());
        stop.store(true, Ordering::Release);
        assert!(transfer(&b"abc"[..], &mut out, 0, 3, &stop, &mut |_| {}).is_err());
        assert!(out.is_empty());
    }
}
