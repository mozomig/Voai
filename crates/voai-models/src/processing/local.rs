use super::{MAX_OUTPUT, edited, validate_input, validate_processing_prompt};
use crate::check_cancelled;
use anyhow::{Context, Result, ensure};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn process_local(
    model_path: &Path,
    prompt: &str,
    transcript: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    let helper = std::env::current_exe()?.with_file_name("voai-text-helper");
    run_local(&helper, model_path, prompt, transcript, cancel)
}

fn run_local(
    helper: &Path,
    model_path: &Path,
    prompt: &str,
    transcript: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    validate_input(transcript)?;
    validate_processing_prompt(prompt)?;
    check_cancelled(cancel)?;
    let mut child = Worker(
        Command::new(helper)
            .arg("--edit-v2")
            .arg(model_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Не удалось запустить локальный движок Voai")?,
    );
    let mut stdin = child
        .0
        .stdin
        .take()
        .context("Не удалось открыть ввод движка")?;
    let stdout = child
        .0
        .stdout
        .take()
        .context("Не удалось открыть вывод движка")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    thread::scope(|scope| -> Result<String> {
        // stdin stays open in the parent after the request. EOF signals process exit to the helper.
        let input = scope.spawn(|| {
            for message in [prompt, transcript] {
                stdin.write_all(&(message.len() as u32).to_be_bytes())?;
                stdin.write_all(message.as_bytes())?;
            }
            Ok::<(), std::io::Error>(())
        });
        let output = scope.spawn(move || {
            let mut data = String::new();
            stdout
                .take(MAX_OUTPUT as u64 + 1)
                .read_to_string(&mut data)?;
            Ok::<String, std::io::Error>(data)
        });
        // Kill before joining the pipes so a wedged or cancelled engine cannot block teardown.
        loop {
            if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                let _ = child.0.kill();
                let _ = child.0.wait();
                check_cancelled(cancel)?;
                anyhow::bail!("Локальная модель не ответила вовремя");
            }
            match child.0.try_wait() {
                Ok(Some(status)) => {
                    ensure!(status.success(), "Локальная модель не завершила обработку");
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(40)),
                Err(err) => {
                    let _ = child.0.kill();
                    return Err(err.into());
                }
            }
        }
        input
            .join()
            .map_err(|_| anyhow::anyhow!("Движок прервал ввод"))??;
        let text = output
            .join()
            .map_err(|_| anyhow::anyhow!("Движок прервал ответ"))??;
        check_cancelled(cancel)?;
        edited(&text)
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::processing::MAX_INPUT;
    #[test]
    #[ignore = "requires VOAI_TEST_GGUF and VOAI_TEST_TEXT_HELPER (Qwen2.5 Instruct, built helper)"]
    fn custom_system_prompts_change_real_local_output() {
        let model = std::env::var_os("VOAI_TEST_GGUF").expect("VOAI_TEST_GGUF");
        let helper = std::env::var_os("VOAI_TEST_TEXT_HELPER").expect("VOAI_TEST_TEXT_HELPER");
        let phrase = "давай перенесём встречу на пятницу";
        for (prompt, expected) in [
            (
                "Исправь регистр и пунктуацию. Верни только исправленный русский текст.",
                "пятниц",
            ),
            (
                "Translate the user's text into English. Return only the translation.",
                "friday",
            ),
        ] {
            let output = run_local(
                Path::new(&helper),
                Path::new(&model),
                prompt,
                phrase,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(output.to_lowercase().contains(expected));
        }
    }
    #[test]
    fn cancellation_terminates_a_stuck_local_process_even_if_its_stdin_is_full() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helper");
        std::fs::write(&helper, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let flag = AtomicBool::new(false);
        let text = "a".repeat(MAX_INPUT);
        thread::scope(|scope| {
            let job =
                scope.spawn(|| run_local(&helper, dir.path(), "Сократи текст.", &text, &flag));
            thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Release);
            let started = Instant::now();
            assert_eq!(
                job.join().unwrap().unwrap_err().to_string(),
                "Операция отменена"
            );
            assert!(started.elapsed() < Duration::from_secs(5));
        });
    }
}
