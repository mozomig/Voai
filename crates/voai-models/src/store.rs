use crate::{ModelFile, check_cancelled};
use anyhow::{Context, Result, ensure};
use ring::digest;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
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

    fn dir(&self, model: &ModelFile) -> PathBuf {
        self.root.join(model.sha256)
    }

    pub fn installed_path(&self, model: &ModelFile) -> Result<PathBuf> {
        let path = self.dir(model).join(model.filename);
        let metadata = path.symlink_metadata().context("Модель не установлена")?;
        ensure!(
            metadata.is_file() && metadata.len() == model.bytes,
            "Файл модели повреждён"
        );
        let stamp =
            fs::read_to_string(self.dir(model).join("verified")).context("Модель не проверена")?;
        ensure!(
            stamp == verified_stamp(&metadata, model)? || stamp == legacy_stamp(&metadata, model)?,
            "Файл модели изменился после проверки"
        );
        Ok(path)
    }

    pub fn installed(&self, model: &ModelFile) -> bool {
        self.installed_path(model).is_ok()
    }

    pub fn partial_bytes(&self, model: &ModelFile) -> u64 {
        fs::metadata(self.dir(model).join("download.part")).map_or(0, |m| m.len().min(model.bytes))
    }

    /// The byte identity comes from the pinned revision and SHA-256. A server
    /// ignoring Range streams the prefix again while keeping the saved bytes.
    pub fn download(
        &self,
        model: &ModelFile,
        cancel: &AtomicBool,
        progress: impl FnMut(u64),
    ) -> Result<PathBuf> {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_per_read(Some(Duration::from_secs(15)))
            .build()
            .new_agent();
        self.download_with(model, cancel, progress, &agent, model.url)
    }

    fn download_with(
        &self,
        model: &ModelFile,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64),
        agent: &ureq::Agent,
        url: &str,
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
        // A restored cache or interrupted publish can have valid weights and
        // no usable receipt. Reverify locally before starting a network transfer.
        let installed = dir.join(model.filename);
        if installed
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.len() == model.bytes)
        {
            let mut existing =
                File::open(&installed).context("Не удалось прочитать установленную модель")?;
            let stamp = verified_stamp(&existing.metadata()?, model)?;
            if verify(&mut existing, model, cancel)?
                && stamp == verified_stamp(&installed.symlink_metadata()?, model)?
            {
                check_cancelled(cancel)?;
                save_receipt(&dir, &stamp)?;
                progress(model.bytes);
                return Ok(installed);
            }
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
            let get = || agent.get(url).header("Accept-Encoding", "identity");
            let response = if offset == 0 {
                get().call()
            } else {
                match get().header("Range", format!("bytes={offset}-")).call() {
                    Err(ureq::Error::StatusCode(416)) => {
                        check_cancelled(cancel)?;
                        get().call()
                    }
                    other => other,
                }
            };
            let mut response = response.map_err(|err| {
                anyhow::anyhow!(
                    "Не удалось скачать модель ({}). Повторите загрузку",
                    http_failure(&err)
                )
            })?;
            let mut skip_prefix = 0;
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
                    // The URL pins immutable content. Overlap already on disk is
                    // skipped. Final SHA-256 still verifies the entire file.
                    skip_prefix = offset;
                }
                code => anyhow::bail!("Сервис загрузки вернул HTTP {code}"),
            }
            transfer(
                response.body_mut().as_reader(),
                &mut output,
                skip_prefix,
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
        fs::rename(part, &installed)?;
        save_receipt(&dir, &verified_stamp(&output.metadata()?, model)?)?;
        Ok(installed)
    }
}

fn save_receipt(dir: &Path, stamp: &str) -> Result<()> {
    let mut receipt = File::create(dir.join("verified.next"))?;
    receipt.write_all(stamp.as_bytes())?;
    receipt.sync_all()?;
    fs::rename(dir.join("verified.next"), dir.join("verified"))?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn verified_stamp(metadata: &fs::Metadata, model: &ModelFile) -> Result<String> {
    Ok(format!(
        "v2 {} {} {} {} {}",
        legacy_stamp(metadata, model)?,
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec()
    ))
}

// A cheap startup check ties the SHA-verified artifact to its last modification
// time. Hashing a gigabyte of weights on the UI thread would stall each launch.
fn legacy_stamp(metadata: &fs::Metadata, model: &ModelFile) -> Result<String> {
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
    mut skip_prefix: u64,
    mut count: u64,
    bytes: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<()> {
    let mut buf = vec![0; 128 * 1024];
    loop {
        check_cancelled(cancel)?;
        let n = input
            .read(&mut buf)
            .context("Соединение прервано. Повторите загрузку, чтобы продолжить")?;
        if n == 0 {
            break;
        }
        let skip = skip_prefix.min(n as u64);
        skip_prefix -= skip;
        let fresh = &buf[skip as usize..n];
        count += fresh.len() as u64;
        ensure!(
            count <= bytes,
            "Сервер прислал больше данных, чем ожидалось"
        );
        out.write_all(fresh)
            .context("Не удалось сохранить веса на диске")?;
        progress(count);
    }
    ensure!(
        count == bytes && skip_prefix == 0,
        "Загрузка прервана. Скачано {count} из {bytes} байт"
    );
    Ok(())
}

fn verify(input: &mut impl Read, model: &ModelFile, cancel: &AtomicBool) -> Result<bool> {
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
mod tests;
