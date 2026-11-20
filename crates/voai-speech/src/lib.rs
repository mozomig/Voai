//! A recording owns the microphone until release or cancellation. The UI only polls events.
mod audio;
mod denoise;
mod devices;
mod worker;

pub use devices::input_devices;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
pub use worker::{validate_model, worker_main};

/// Mono sample rate shared by recognition and saved recordings.
pub const SAMPLE_RATE: u32 = 16_000;
const RATE: usize = SAMPLE_RATE as usize;
const MIN_SAMPLES: usize = RATE / 8;
const INPUT_TIMEOUT: Duration = Duration::from_secs(7);
const MAX_SAMPLES: usize = RATE * 60 * 5;
const RECORDING: u8 = 0;
const FINISHING: u8 = 1;
const CANCELLED: u8 = 2;

#[derive(Default)]
pub struct FinalTranscript {
    pub text: String,
    pub samples: Vec<f32>,
}

impl std::fmt::Debug for FinalTranscript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalTranscript")
            .field("samples", &self.samples.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum Event {
    Level(f32),
    Partial(String),
    Final(FinalTranscript),
    Error(String),
    Interrupted(String),
}

#[derive(Default)]
struct Mailbox {
    level: Option<f32>,
    interruption: Option<String>,
    partial: Option<String>,
    terminal: Option<Event>,
}

pub struct Recording {
    state: Arc<AtomicU8>,
    events: Arc<Mutex<Mailbox>>,
}

impl Recording {
    pub fn finish(&self) {
        let _ =
            self.state
                .compare_exchange(RECORDING, FINISHING, Ordering::AcqRel, Ordering::Acquire);
    }

    pub fn cancel(&self) {
        self.state.store(CANCELLED, Ordering::Release);
    }

    pub fn try_event(&self) -> Option<Event> {
        if self.state.load(Ordering::Acquire) == CANCELLED {
            return None;
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        events
            .terminal
            .take()
            .or_else(|| events.interruption.take().map(Event::Interrupted))
            .or_else(|| events.partial.take().map(Event::Partial))
            .or_else(|| events.level.take().map(Event::Level))
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start(
    model: PathBuf,
    language: Option<String>,
    device: Option<String>,
    noise_suppression: bool,
) -> Result<Recording, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    start_with_source(
        model,
        language,
        executable,
        noise_suppression,
        move |state| audio::Microphone::open(device.as_deref(), state),
    )
}

/// Feeds a mono PCM recording through the capture loop for offline acceptance tests.
#[cfg(feature = "test-fixtures")]
pub fn start_fixture(
    model: PathBuf,
    language: Option<String>,
    executable: PathBuf,
    samples: Vec<f32>,
    rate: usize,
    noise_suppression: bool,
) -> Result<(Recording, std::sync::mpsc::Receiver<()>), String> {
    let (eof, exhausted) = std::sync::mpsc::channel();
    let recording = start_with_source(
        model,
        language,
        executable,
        noise_suppression,
        move |state| audio::Microphone::fixture(samples, rate, state, eof),
    )?;
    Ok((recording, exhausted))
}

fn start_with_source(
    model: PathBuf,
    language: Option<String>,
    executable: PathBuf,
    noise_suppression: bool,
    open: impl FnOnce(Arc<AtomicU8>) -> Result<audio::Microphone, String> + Send + 'static,
) -> Result<Recording, String> {
    if !model.is_file() {
        return Err("Файл речевой модели отсутствует".into());
    }
    if language.as_deref().is_some_and(|s| {
        s.is_empty() || s.len() > 12 || !s.bytes().all(|c| c.is_ascii_alphabetic() || c == b'-')
    }) {
        return Err("Некорректный код языка".into());
    }
    let state = Arc::new(AtomicU8::new(RECORDING));
    let events = Arc::new(Mutex::new(Mailbox::default()));
    let capture_state = state.clone();
    let capture_events = events.clone();
    thread::Builder::new()
        .name("voai-capture".into())
        .spawn(move || {
            let session = Session {
                model,
                language,
                executable,
                state: &capture_state,
                events: &capture_events,
            };
            let result = session.run(open, noise_suppression);
            if capture_state.load(Ordering::Acquire) != CANCELLED {
                let mut events = capture_events.lock().unwrap_or_else(|e| e.into_inner());
                events.partial = None;
                events.level = None;
                events.interruption = None;
                events.terminal = Some(match result {
                    Ok(text) => Event::Final(text),
                    Err(e) => Event::Error(e),
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Recording { state, events })
}

struct Session<'a> {
    model: PathBuf,
    language: Option<String>,
    executable: PathBuf,
    state: &'a Arc<AtomicU8>,
    events: &'a Mutex<Mailbox>,
}

impl Session<'_> {
    fn run(
        &self,
        open: impl FnOnce(Arc<AtomicU8>) -> Result<audio::Microphone, String>,
        noise_suppression: bool,
    ) -> Result<FinalTranscript, String> {
        if self.state.load(Ordering::Acquire) != RECORDING {
            return Ok(FinalTranscript::default());
        }
        let mut mic = open(self.state.clone())?;
        let mut filter = denoise::NoiseSuppression::new(noise_suppression);
        let mut worker = None;
        let mut pcm = Vec::with_capacity(RATE * 5);
        let mut last_input = Instant::now();
        let mut last_partial = 0;
        let mut pending = None;
        while self.state.load(Ordering::Acquire) == RECORDING {
            let previous = pcm.len();
            if mic.drain(&mut pcm)? {
                last_input = Instant::now();
            }
            pcm.truncate(MAX_SAMPLES);
            let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
            if pcm.len() > previous {
                let samples = &pcm[previous..];
                let energy = samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32;
                events.level = Some((energy.sqrt() * 5.).clamp(0., 1.));
            }
            drop(events);
            let reason = mic.interruption().or_else(|| {
                if last_input.elapsed() >= INPUT_TIMEOUT {
                    Some(if pcm.is_empty() {
                        "Нет звука. Проверьте микрофон и доступ к нему."
                    } else {
                        "Микрофон перестал передавать звук"
                    })
                } else {
                    (pcm.len() >= MAX_SAMPLES).then_some("Прошло пять минут")
                }
            });
            if let Some(reason) = reason {
                mic.stop();
                if pcm.is_empty() {
                    return Err(reason.into());
                }
                self.events
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .interruption = Some(format!(
                    "{reason}. Отпустите клавиши, чтобы вставить записанную часть."
                ));
                while self.state.load(Ordering::Acquire) == RECORDING {
                    thread::sleep(Duration::from_millis(20));
                }
                break;
            }
            // A tap can end before there is decodable audio. Leave any warm worker
            // untouched until this recording actually needs the model.
            if previous < MIN_SAMPLES && pcm.len() >= MIN_SAMPLES {
                worker = Some(self.worker()?);
            }
            let speech = filter.audio(&pcm, false);
            if let Some(active) = &worker {
                let update = self.update_partial(active, speech, &mut last_partial, &mut pending);
                if update.is_err() {
                    // Keep capturing. A fresh helper will receive all PCM on key release.
                    worker.take();
                    pending = None;
                }
            }
            thread::sleep(Duration::from_millis(15));
        }
        if self.state.load(Ordering::Acquire) == CANCELLED {
            return Ok(FinalTranscript::default());
        }
        mic.finish(&mut pcm)?;
        pcm.truncate(MAX_SAMPLES);
        if pcm.len() < MIN_SAMPLES {
            return Ok(FinalTranscript::default());
        }
        let speech = filter.audio(&pcm, true);
        if let Some(active) = worker
            && let Ok(text) = self.finish(&active, speech, pending.is_some())
        {
            active.keep_warm();
            return Ok(FinalTranscript { text, samples: pcm });
        }
        if self.state.load(Ordering::Acquire) == CANCELLED {
            return Ok(FinalTranscript::default());
        }
        let fresh = self.worker()?;
        let text = self.finish(&fresh, speech, false)?;
        fresh.keep_warm();
        Ok(FinalTranscript { text, samples: pcm })
    }

    fn update_partial(
        &self,
        worker: &worker::Worker,
        pcm: &[f32],
        last: &mut usize,
        pending: &mut Option<Instant>,
    ) -> Result<(), String> {
        if let Some(text) = worker.response()? {
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .partial = Some(text);
            *pending = None;
        }
        if pending.is_some_and(|start| start.elapsed() > Duration::from_secs(60)) {
            return Err("Whisper не ответил вовремя".into());
        }
        if pending.is_none() && pcm.len() - *last >= RATE {
            *last = pcm.len();
            let start = pcm.len().saturating_sub(12 * RATE);
            worker.request(worker::DecodeRequest::Partial(pcm[start..].to_vec()))?;
            *pending = Some(Instant::now());
        }
        Ok(())
    }

    fn worker(&self) -> Result<worker::Worker, String> {
        worker::Worker::acquire(&self.executable, &self.model, self.language.as_deref())
    }

    fn finish(
        &self,
        worker: &worker::Worker,
        pcm: &[f32],
        partial_pending: bool,
    ) -> Result<String, String> {
        if partial_pending {
            self.wait(worker, Duration::from_secs(45))?;
        }
        worker.request(worker::DecodeRequest::Final(pcm.to_vec()))?;
        self.wait(worker, Duration::from_secs(45 + (pcm.len() / RATE) as u64))
    }

    fn wait(&self, worker: &worker::Worker, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        while self.state.load(Ordering::Acquire) != CANCELLED && Instant::now() < deadline {
            if let Some(text) = worker.response()? {
                return Ok(text);
            }
            thread::sleep(Duration::from_millis(15));
        }
        Err("Распознавание остановлено".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_press_finishes_without_starting_a_decoder() {
        let (eof, exhausted) = std::sync::mpsc::channel();
        let recording = start_with_source(
            std::env::current_exe().unwrap(),
            None,
            PathBuf::from("/nonexistent-voai-worker"),
            false,
            move |state| audio::Microphone::fixture(vec![0.25; RATE / 12], RATE, state, eof),
        )
        .unwrap();
        exhausted.recv_timeout(Duration::from_secs(3)).unwrap();
        recording.finish();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match recording.try_event() {
                Some(Event::Final(text)) => {
                    assert!(text.text.is_empty());
                    break;
                }
                Some(Event::Error(reason)) => panic!("Unnecessary worker startup: {reason}"),
                _ => (),
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn cancellation_hides_late_worker_reply_and_cannot_be_reversed() {
        let recording = Recording {
            state: Arc::new(AtomicU8::new(RECORDING)),
            events: Arc::new(Mutex::new(Mailbox {
                terminal: Some(Event::Final(FinalTranscript {
                    text: "previous session".into(),
                    samples: vec![],
                })),
                ..Mailbox::default()
            })),
        };
        recording.cancel();
        recording.finish();
        assert_eq!(recording.state.load(Ordering::Acquire), CANCELLED);
        assert!(recording.try_event().is_none());
    }
}
