use super::{
    FileDocument, FileLine, FileProgress, Update,
    document::{SourceStamp, Turn, Word, align},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
pub(super) struct Tools {
    pub executable: PathBuf,
    pub media: PathBuf,
    text: PathBuf,
    diarize: PathBuf,
}

impl Tools {
    pub fn bundled() -> Self {
        let executable = std::env::current_exe().unwrap_or_default();
        Self::from_executable(executable)
    }
    pub(super) fn from_executable(executable: PathBuf) -> Self {
        let bin = executable.parent().unwrap_or(Path::new(""));
        Self {
            text: bin.join("voai-text-helper"),
            media: bin.join("voai-media-helper"),
            diarize: bin.join("voai-diarize"),
            executable,
        }
    }
}

pub(super) struct Work {
    pub cancelled: Arc<AtomicBool>,
    pub updates: mpsc::Sender<Update>,
    pub wake: async_channel::Sender<()>,
}

impl Work {
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) {
            Err("Импорт отменён.".into())
        } else {
            Ok(())
        }
    }
    pub(super) fn progress(&self, stage: &str, percent: u8) {
        let _ = self.updates.send(Update::Progress(FileProgress {
            stage: stage.to_owned(),
            percent,
            cancellable: true,
        }));
        let _ = self.wake.try_send(());
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Reply {
    Progress { progress: u8 },
    Words { words: Vec<Word> },
    Turns { turns: Vec<Turn> },
    Edited { edited: EditedLine },
}

#[derive(Deserialize)]
struct EditedLine {
    index: usize,
    text: String,
}

pub(super) struct Pipeline {
    pub model: PathBuf,
    pub language: String,
    pub voices: Option<PathBuf>,
    pub editor: Option<PathBuf>,
    pub prompt: String,
}

/// Dropping an import always stops its child. Reading stdout on a separate thread lets
/// the supervisor observe cancellation even during a long native inference call.
pub(super) struct ChildGuard(pub Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run(
    command: &mut Command,
    work: &Work,
    stage: &str,
    error: &str,
    mut accept: impl FnMut(Reply) -> Result<(), String>,
) -> Result<(), String> {
    work.check()?;
    work.progress(stage, 0);
    let mut child = ChildGuard(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| error)?,
    );
    let pipe = child.0.stdout.take().ok_or(error)?;
    let (tx, rx) = mpsc::sync_channel(4);
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = Vec::new();
        const MAX: u64 = 16 * 1024 * 1024;
        loop {
            line.clear();
            let reply = match reader.by_ref().take(MAX + 1).read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(n) if n <= MAX as usize => serde_json::from_slice(&line).map_err(|_| ()),
                _ => Err(()),
            };
            let invalid = reply.is_err();
            if tx.send(reply).is_err() || invalid {
                break;
            }
        }
    });
    let mut last = 0;
    loop {
        work.check()?;
        match rx.recv_timeout(Duration::from_millis(80)) {
            Ok(Ok(Reply::Progress { progress })) => {
                let percent = progress.min(100);
                if percent > last {
                    work.progress(stage, percent);
                    last = percent;
                }
            }
            Ok(Ok(value)) => accept(value)?,
            Ok(Err(())) => return Err(error.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    loop {
        work.check()?;
        if let Some(status) = child.0.try_wait().map_err(|_| error)? {
            return if status.success() {
                Ok(())
            } else {
                Err(error.into())
            };
        }
        thread::sleep(Duration::from_millis(30));
    }
}

pub(super) fn import(
    tools: &Tools,
    source: PathBuf,
    pipeline: &Pipeline,
    work: &Work,
) -> Result<FileDocument, String> {
    let source = source
        .canonicalize()
        .map_err(|_| "Не удалось открыть исходный файл.")?;
    let stamp = SourceStamp::read(&source)?;
    let temp = tempfile::tempdir().map_err(|_| "Не удалось подготовить временную папку.")?;
    let pcm = temp.path().join("audio.pcm");
    run(
        Command::new(&tools.media)
            .arg("decode")
            .arg(&source)
            .arg(&pcm)
            .stdin(Stdio::null()),
        work,
        "Чтение аудио",
        "Не удалось прочитать аудиодорожку. Проверьте формат файла и наличие звука.",
        |_| Err("Ошибка декодирования.".into()),
    )?;
    if SourceStamp::read(&source)? != stamp {
        return Err("Исходный файл изменился во время чтения.".into());
    }
    let bytes = pcm
        .metadata()
        .map_err(|_| "Не удалось прочитать звук.")?
        .len();
    if bytes == 0 || bytes % 2 != 0 {
        return Err("Файл не содержит звука.".into());
    }
    let duration_ms = bytes.div_ceil(32);
    let mut words = vec![];
    run(
        Command::new(&tools.executable)
            .arg("--file-worker")
            .arg(&pipeline.model)
            .arg(&pipeline.language)
            .arg(&pcm)
            .stdin(Stdio::null()),
        work,
        "Распознавание речи",
        "Не удалось распознать файл. Проверьте речевую модель и свободную память.",
        |reply| {
            let Reply::Words { words: chunk } = reply else {
                return Err("Ошибка речевого процесса.".into());
            };
            words.extend(chunk.into_iter().map(|mut w| {
                w.end_ms = w.end_ms.min(duration_ms);
                w
            }));
            Ok(())
        },
    )?;
    let mut turns = vec![];
    let mut received = false;
    let mut warning = if words.is_empty() {
        Some("Речь в файле не обнаружена.".into())
    } else if let Some(voices) = &pipeline.voices {
        let result = run(
            Command::new(&tools.diarize)
                .arg(&pcm)
                .arg(voices)
                .stdin(Stdio::null()),
            work,
            "Определение спикеров",
            "Не удалось определить спикеров.",
            |reply| {
                let Reply::Turns { turns: chunk } = reply else {
                    return Err("Ошибка определения спикеров.".into());
                };
                received = true;
                turns.extend(chunk);
                Ok(())
            },
        );
        work.check()?;
        if result.is_err() || !received || turns.is_empty() {
            turns.clear();
            Some("Не удалось определить спикеров. Текст и таймкоды сохранены.".to_owned())
        } else {
            None
        }
    } else {
        Some("Расшифровано без определения спикеров.".into())
    };
    let mut lines = align(words, turns);
    // Use chronological labels regardless of the clustering backend's internal numbering.
    let mut ids = BTreeMap::new();
    let mut speakers = BTreeMap::new();

    for line in &mut lines {
        if let Some(id) = &mut line.speaker {
            let next = ids.len() as u32 + 1;
            *id = *ids.entry(*id).or_insert(next);
            speakers
                .entry(*id)
                .or_insert_with(|| format!("Спикер {id}"));
        }
    }
    work.check()?;
    if let Some(editor) = &pipeline.editor
        && !lines.is_empty()
        && edit_lines(&tools.text, editor, &pipeline.prompt, &mut lines, work).is_err()
    {
        work.check()?;
        let message = "Обработка прервалась. У оставшихся реплик сохранён исходный текст.";
        match &mut warning {
            Some(previous) => {
                previous.push(' ');
                previous.push_str(message);
            }
            None => warning = Some(message.into()),
        }
    }
    work.check()?;
    Ok(FileDocument {
        id: String::new(),
        title: source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        source,
        speakers,
        lines,
        warning,
        source_stamp: stamp,
    })
}

fn edit_lines(
    helper: &Path,
    model: &Path,
    prompt: &str,
    lines: &mut [FileLine],
    work: &Work,
) -> Result<(), String> {
    const STAGE: &str = "Обработка текста";
    let error = "Не удалось обработать текст.";
    let mut input = tempfile::tempfile().map_err(|_| error)?;
    fn write_field(w: &mut impl Write, s: &str) -> std::io::Result<()> {
        w.write_all(
            &u32::try_from(s.len())
                .map_err(std::io::Error::other)?
                .to_be_bytes(),
        )?;
        w.write_all(s.as_bytes())
    }
    write_field(&mut input, prompt).map_err(|_| error)?;
    let count = u32::try_from(lines.len()).map_err(|_| error)?;
    input.write_all(&count.to_be_bytes()).map_err(|_| error)?;
    for line in lines.iter() {
        write_field(&mut input, &line.text).map_err(|_| error)?;
    }
    input.rewind().map_err(|_| error)?;
    let mut next = 0;
    run(
        Command::new(helper)
            .arg("--edit-lines-v1")
            .arg(model)
            .stdin(input),
        work,
        STAGE,
        error,
        |r| {
            let Reply::Edited { edited } = r else {
                return Err(error.into());
            };
            if edited.index != next || edited.text.trim().is_empty() {
                return Err(error.into());
            }
            let row = lines.get_mut(next).ok_or(error)?;
            if row.text != edited.text {
                row.original = Some(std::mem::replace(&mut row.text, edited.text));
            }
            next += 1;
            work.progress(STAGE, (next * 100 / lines.len()) as u8);
            Ok(())
        },
    )?;
    if next != lines.len() {
        return Err(error.into());
    }
    Ok(())
}
