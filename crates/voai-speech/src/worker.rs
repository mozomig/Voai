//! One outstanding decode per helper. All IPC and model loads run off the UI and audio threads.
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use voai_l10n::{tf, tr};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::{MAX_SAMPLES, MIN_SAMPLES, RATE};

const PROTOCOL: &[u8; 8] = b"VOAIS001";
const MAX_REPLY: usize = 1 << 20;
const KEEP_WARM: Duration = Duration::from_secs(90);

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_reply(w: &mut impl Write, reply: Result<&str, &str>) -> io::Result<()> {
    let (code, text) = match reply {
        Ok(text) => (0, text),
        Err(text) => (1, text),
    };
    if text.len() > MAX_REPLY {
        return Err(io::Error::other(tr("speech.reply.large")));
    }
    w.write_all(&[code])?;
    w.write_all(&(text.len() as u32).to_le_bytes())?;
    w.write_all(text.as_bytes())?;
    w.flush()
}

fn read_reply(r: &mut impl Read) -> Result<String, String> {
    fn read(r: &mut impl Read) -> io::Result<(u8, String)> {
        let mut status = [0];
        r.read_exact(&mut status)?;
        let size = read_u32(r)? as usize;
        if size > MAX_REPLY {
            return Err(io::Error::other(tr("speech.reply.large")));
        }
        let mut bytes = vec![0; size];
        r.read_exact(&mut bytes)?;
        let text = String::from_utf8(bytes).map_err(io::Error::other)?;
        Ok((status[0], text))
    }
    match read(r) {
        Ok((0, text)) => Ok(text),
        Ok((1, error)) => Err(error),
        Ok(_) => Err(tr("speech.reply.invalid").into()),
        Err(e) => Err(tf!("speech.worker.lost", e = e)),
    }
}

/// Verify imported weights in an isolated CPU-only context, without capturing audio.
pub fn validate_model(model: &Path) -> Result<(), String> {
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .env(voai_l10n::WORKER_LANGUAGE_ENV, voai_l10n::language())
        .arg("--speech-worker")
        .arg("--validate-v1")
        .arg(model)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| tf!("speech.check.start", e = e))?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(tr("speech.check.load").into()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(40)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => e.to_string(),
                    _ => tr("speech.check.timeout").into(),
                });
            }
        }
    }
}

pub fn worker_main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(2);
    let model = args.next().ok_or(tr("speech.path.missing"))?;
    if model == "--validate-v1" {
        let path = args.next().ok_or(tr("speech.path.missing"))?;
        whisper_rs::install_logging_hooks();
        let mut config = WhisperContextParameters::default();
        config.use_gpu(false);
        WhisperContext::new_with_params(Path::new(&path), config).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let language = args
        .next()
        .and_then(|s| s.into_string().ok())
        .filter(|s| s != "auto");
    run_worker(
        Path::new(&model),
        language.as_deref(),
        io::stdin().lock(),
        io::stdout().lock(),
    )
    .map_err(|e| e.to_string())
}

fn run_worker(
    model: &Path,
    language: Option<&str>,
    stdin: impl Read,
    stdout: impl Write,
) -> io::Result<()> {
    let mut input = BufReader::new(stdin);
    let mut output = BufWriter::new(stdout);
    let mut protocol = [0; 8];
    input.read_exact(&mut protocol)?;
    if &protocol != PROTOCOL {
        return Err(io::Error::other(tr("speech.protocol")));
    }

    whisper_rs::install_logging_hooks();
    let ctx = match WhisperContext::new_with_params(model, WhisperContextParameters::default()) {
        Ok(ctx) => ctx,
        Err(err) => {
            return write_reply(&mut output, Err(&tf!("speech.open.detail", err = err)));
        }
    };
    let mut state = None;
    write_reply(&mut output, Ok(""))?;
    loop {
        let mut kind = [0u8];
        match input.read_exact(&mut kind) {
            Ok(()) if kind[0] < 2 => {}
            Ok(()) => return Err(io::Error::other(tr("speech.command"))),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let size = read_u32(&mut input)? as usize;
        if size > MAX_SAMPLES {
            return Err(io::Error::other(tr("speech.limit.samples")));
        }
        // Both processes are slices of the same macOS binary and share the float ABI.
        let mut samples = vec![0.0f32; size];
        input.read_exact(bytemuck::cast_slice_mut(&mut samples))?;
        let decoder = match &mut state {
            Some(state) => state,
            None => match ctx.create_state() {
                Ok(created) => state.insert(created),
                Err(e) => return write_reply(&mut output, Err(&e.to_string())),
            },
        };
        let result = decode(decoder, &samples, language);
        if kind[0] == 1 {
            state.take();
        }
        write_reply(&mut output, result.as_deref().map_err(String::as_str))?;
    }
}

pub(super) fn decode_params(language: Option<&str>) -> FullParams<'_, 'static> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(language);
    params.set_n_threads(thread::available_parallelism().map_or(4, |n| n.get().min(6)) as i32);
    params.set_translate(false);
    params.set_no_context(true);
    params.set_suppress_nst(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params
}

fn decode(
    state: &mut whisper_rs::WhisperState,
    samples: &[f32],
    language: Option<&str>,
) -> Result<String, String> {
    if samples.len() < MIN_SAMPLES
        || !samples.chunks(160).any(|frame| {
            let energy: f32 = frame.iter().map(|v| v * v).sum();
            energy / frame.len() as f32 > 1e-7
        })
    {
        return Ok(String::new());
    }
    let params = decode_params(language);
    // whisper.cpp expects at least one second, including for short utterances.
    let mut padded;
    let samples = if samples.len() < RATE {
        padded = samples.to_vec();
        padded.resize(RATE, 0.0);
        padded.as_slice()
    } else {
        samples
    };
    state
        .full(params, samples)
        .map_err(|e| tf!("speech.decode.error", e = e))?;
    let mut text = String::new();
    for segment in state.as_iter().filter(|s| s.no_speech_probability() < 0.8) {
        text.push_str(&segment.to_str_lossy().map_err(|e| e.to_string())?);
    }
    Ok(text.trim().to_owned())
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileStamp {
    fn read(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = path.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

pub(super) struct Worker {
    executable: PathBuf,
    model: PathBuf,
    stamp: FileStamp,
    language: Option<String>,
    interface_language: &'static str,
    child: Child,
    tx: mpsc::Sender<DecodeRequest>,
    rx: mpsc::Receiver<Result<String, String>>,
}

pub(super) enum DecodeRequest {
    Partial(Vec<f32>),
    Final(Vec<f32>),
}

static IDLE: Mutex<Option<(Instant, Worker)>> = Mutex::new(None);

impl Worker {
    pub(super) fn acquire(
        executable: &Path,
        model: &Path,
        language: Option<&str>,
    ) -> Result<Self, String> {
        let stamp = FileStamp::read(model).map_err(|e| tf!("speech.open.error", e = e))?;
        let interface_language = voai_l10n::language();
        let idle = IDLE.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((_, mut worker)) = idle
            && worker.executable == executable
            && worker.model == model
            && worker.stamp == stamp
            && worker.interface_language == interface_language
            && worker.language.as_deref() == language
            && matches!(worker.child.try_wait(), Ok(None))
        {
            return Ok(worker);
        }
        let mut child = Command::new(executable)
            .env(voai_l10n::WORKER_LANGUAGE_ENV, interface_language)
            .arg("--speech-worker")
            .arg(model)
            .arg(language.unwrap_or("auto"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| tf!("speech.worker.start", e = e))?;
        let mut input = BufWriter::new(child.stdin.take().expect("piped stdin"));
        let mut output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let (tx, requests) = mpsc::channel::<DecodeRequest>();
        let (results, rx) = mpsc::channel();
        thread::spawn(move || {
            let protocol = input
                .write_all(PROTOCOL)
                .and_then(|()| input.flush())
                .map_err(|e| e.to_string())
                .and_then(|()| read_reply(&mut output));
            if let Err(e) = protocol {
                let _ = results.send(Err(e));
                return;
            }
            for request in requests {
                let (kind, audio) = match request {
                    DecodeRequest::Partial(audio) => (0, audio),
                    DecodeRequest::Final(audio) => (1, audio),
                };
                let result = input
                    .write_all(&[kind])
                    .and_then(|()| input.write_all(&(audio.len() as u32).to_le_bytes()))
                    .and_then(|()| input.write_all(bytemuck::cast_slice(&audio)))
                    .and_then(|()| input.flush())
                    .map_err(|e| e.to_string())
                    .and_then(|()| read_reply(&mut output));
                let failed = result.is_err();
                if results.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            executable: executable.into(),
            model: model.into(),
            stamp,
            interface_language,
            language: language.map(Into::into),
            child,
            tx,
            rx,
        })
    }

    pub(super) fn request(&self, request: DecodeRequest) -> Result<(), String> {
        self.tx
            .send(request)
            .map_err(|_| tr("speech.worker.exited").into())
    }

    pub(super) fn response(&self) -> Result<Option<String>, String> {
        match self.rx.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(tr("speech.worker.exited").into()),
        }
    }

    pub(super) fn keep_warm(self) {
        let now = Instant::now();
        let previous = IDLE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace((now, self));
        drop(previous);
        thread::spawn(move || {
            thread::sleep(KEEP_WARM);
            let expired = IDLE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take_if(|(last, _)| *last == now);
            drop(expired);
        });
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Unblocks a pipe thread even during model load or stuck native inference.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_reply_length_is_rejected_before_allocation() {
        let bytes = [0, 255, 255, 255, 255];
        assert!(read_reply(&mut &bytes[..]).is_err());
    }
    #[test]
    fn preserves_error_and_unicode_text() {
        for expected in [Ok("Сегодня пятница 🎙"), Err("Нет модели")] {
            let mut bytes = vec![];
            write_reply(&mut bytes, expected).unwrap();
            assert_eq!(
                read_reply(&mut bytes.as_slice())
                    .as_deref()
                    .map_err(String::as_str),
                expected
            );
        }
    }
}
