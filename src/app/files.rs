//! File transcription has its own child processes, cancellation token and durable library.
mod document;
mod job;
mod store;
pub use document::{ExportFormat, FileDocument};
use document::{FileLine, SourceStamp};
use job::{ChildGuard, Tools, Work};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};
use voai_l10n::tr;

#[derive(Clone, PartialEq, Eq)]
pub struct FileSummary {
    pub id: String,
    pub title: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct FileProgress {
    pub stage_key: &'static str,
    pub percent: u8,
    pub cancellable: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct FileView {
    pub library: Arc<[FileSummary]>,
    pub document: Option<Arc<FileDocument>>,
    pub progress: Option<FileProgress>,
    pub error: Option<String>,
    pub exported: Option<PathBuf>,
    pub playing: Option<usize>,
    pub speech_model: String,
    pub text_model: Option<String>,
    pub identify_speakers: bool,
    pub voices_ready: bool,
    pub voices_local: Option<PathBuf>,
    pub voices_downloaded: bool,
    pub using_local_voices: bool,
}

pub enum FileCommand {
    Import(PathBuf),
    Cancel,
    Open(String),
    Play(usize),
    Stop,
    Assign { line: usize, speaker: Option<u32> },
    Rename { speaker: u32, name: String },
    AddSpeaker { line: usize, name: String },
    Export { path: PathBuf, format: ExportFormat },
    SetSpeech(String),
    SetText(Option<String>),
    SetIdentifySpeakers(bool),
    InstallVoices,
    ConnectVoices(PathBuf),
    UseDownloadedVoices,
    UseLocalVoices,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct FileOptions {
    folder: Option<PathBuf>,
    use_local: bool,
    speech: Option<String>,
    text: Option<String>,
    identify_speakers: bool,
}
impl Default for FileOptions {
    fn default() -> Self {
        Self {
            folder: None,
            use_local: false,
            speech: None,
            text: None,
            identify_speakers: true,
        }
    }
}

impl FileOptions {
    fn pipeline(
        &self,
        settings: &super::Settings,
        store: &voai_models::ModelStore,
        voices: Option<PathBuf>,
    ) -> Result<job::Pipeline, String> {
        let speech = self.speech.as_deref().unwrap_or(&settings.speech);
        Ok(job::Pipeline {
            model: settings.path_for(store, speech, voai_models::ModelKind::Speech)?,
            language: settings.language.clone(),
            voices,
            editor: self
                .text
                .as_deref()
                .map(|id| settings.path_for(store, id, voai_models::ModelKind::Text))
                .transpose()?,
            prompt: settings.processing_prompt.clone(),
        })
    }
}

#[derive(Clone)]
pub(super) struct BotPipeline {
    tools: Tools,
    pipeline: job::Pipeline,
}

impl BotPipeline {
    pub(super) fn transcribe(
        &self,
        source: PathBuf,
        cancelled: Arc<AtomicBool>,
    ) -> Result<String, String> {
        let work = Work {
            cancelled,
            observer: None,
        };
        work.check()?;
        let document = job::import(&self.tools, source, &self.pipeline, &work)?;
        let text = document.paragraphs().join("\n\n");
        work.check()?;
        Ok(text)
    }
}

enum ResultView {
    Library(Vec<FileSummary>, FileOptions, Option<PathBuf>),
    Imported(FileDocument),
    Saved(FileDocument, Vec<FileSummary>),
    Document(FileDocument),
    // The download/validation can be cancelled until this result is accepted.
    Voices(FileOptions, Option<PathBuf>),
    Selected(FileOptions, Option<PathBuf>),
    Exported(PathBuf),
}

enum Update {
    Progress(FileProgress),
    Done(Box<Result<ResultView, String>>),
}

struct Job {
    cancelled: Arc<AtomicBool>,
    rx: mpsc::Receiver<Update>,
    worker: thread::JoinHandle<()>,
}

pub(super) struct Files {
    pub view: FileView,
    dir: PathBuf,
    model_dir: PathBuf,
    tools: Tools,
    downloaded: Option<PathBuf>,
    options: FileOptions,
    options_loaded: bool,
    wake: async_channel::Sender<()>,
    job: Option<Job>,
    playback: Option<ChildGuard>,
    retired: Vec<thread::JoinHandle<()>>,
}

impl Files {
    pub fn new(data: &Path, wake: async_channel::Sender<()>) -> Self {
        let mut this = Self {
            view: FileView::default(),
            dir: data.join("Transcripts"),
            model_dir: data.join("Diarization"),
            tools: Tools::bundled(),
            options: FileOptions::default(),
            options_loaded: false,
            downloaded: None,
            wake,
            job: None,
            retired: Vec::new(),
            playback: None,
        };
        let models = this.model_dir.clone();
        this.start("app.files.opening_library", false, move |dir, _| {
            let db = store::Store::open(dir)?;
            Ok(ResultView::Library(
                db.list()?,
                db.pipeline_options()?,
                super::diarization::installed(&models),
            ))
        });
        this
    }

    fn start(
        &mut self,
        stage_key: &'static str,
        cancellable: bool,
        task: impl FnOnce(&Path, &Work) -> Result<ResultView, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let ctx = Work {
            cancelled: cancelled.clone(),
            observer: Some((tx, self.wake.clone())),
        };
        let dir = self.dir.clone();
        let worker = thread::spawn(move || {
            let result = task(&dir, &ctx);
            ctx.notify(Update::Done(Box::new(result)));
        });
        self.view.error = None;
        self.view.progress = Some(FileProgress {
            stage_key,
            percent: 0,
            cancellable,
        });
        self.job = Some(Job {
            cancelled,
            rx,
            worker,
        });
    }

    fn snapshot_voices(&mut self, choice: FileOptions, downloaded: Option<PathBuf>) {
        self.options = choice.clone();
        self.options_loaded = true;
        self.view.speech_model = choice.speech.clone().unwrap_or_default();
        self.view.text_model = choice.text.clone();
        self.view.identify_speakers = choice.identify_speakers;
        self.view.voices_downloaded = downloaded.is_some();
        self.downloaded = downloaded;
        self.view.using_local_voices = choice.use_local;
        self.view.voices_local = choice.folder;
        self.view.voices_ready = self.selected_voices().is_some_and(|p| {
            p.join("segmentation.onnx").is_file() && p.join("embedding.onnx").is_file()
        });
    }

    fn selected_voices(&self) -> Option<PathBuf> {
        if self.view.using_local_voices {
            self.view.voices_local.clone()
        } else {
            self.downloaded.clone()
        }
    }

    pub(super) fn bot_pipeline(
        &self,
        settings: &super::Settings,
        store: &voai_models::ModelStore,
    ) -> Result<BotPipeline, String> {
        if !self.options_loaded {
            return Err(tr("app.files.options_unavailable").into());
        }
        Ok(BotPipeline {
            tools: self.tools.clone(),
            pipeline: self.options.pipeline(settings, store, None)?,
        })
    }

    pub fn stop(&mut self) {
        self.playback.take();
        self.view.playing = None;
    }

    fn cancel(&mut self) {
        if self.view.progress.as_ref().is_some_and(|p| p.cancellable)
            && let Some(job) = self.job.take()
        {
            job.cancelled.store(true, Ordering::Release);
            self.retired.push(job.worker);
            // Dropping this receiver invalidates every late response, including a completed import.
            self.view.progress = None;
        }
    }

    fn persist(&mut self, mut doc: FileDocument) {
        self.start("app.files.saving", false, move |dir, _| {
            let db = store::Store::open(dir)?;
            db.save(&mut doc)?;
            Ok(ResultView::Saved(doc, db.list()?))
        });
    }

    fn select(&mut self, choice: FileOptions, downloaded: Option<PathBuf>) {
        self.start("app.files.saving_settings", false, move |dir, _| {
            store::Store::open(dir)?.save_choice(&choice)?;
            Ok(ResultView::Selected(choice, downloaded))
        });
    }

    pub fn poll(&mut self) {
        self.retired.retain(|worker| !worker.is_finished());
        if let Some(player) = &mut self.playback {
            match player.0.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) if status.success() => self.stop(),
                _ => {
                    self.stop();
                    self.view.error = Some(tr("app.files.cannot_play").into());
                }
            }
        }
        while let Some(job) = &self.job {
            let done = match job.rx.try_recv() {
                Ok(Update::Progress(progress)) => {
                    self.view.progress = Some(progress);
                    continue;
                }
                Ok(Update::Done(result)) => *result,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(tr("app.files.task_interrupted").into())
                }
            };
            self.job = None;
            self.view.progress = None;
            match done {
                Err(e) => self.view.error = Some(e),
                Ok(ResultView::Library(items, choice, downloaded)) => {
                    self.view.library = items.into();
                    self.snapshot_voices(choice, downloaded);
                }
                Ok(ResultView::Imported(doc)) => self.persist(doc),
                Ok(ResultView::Saved(doc, items)) => {
                    self.view.document = Some(Arc::new(doc));
                    self.view.library = items.into();
                }
                Ok(ResultView::Document(doc)) => self.view.document = Some(Arc::new(doc)),
                Ok(ResultView::Voices(choice, d)) => self.select(choice, d),
                Ok(ResultView::Selected(choice, d)) => self.snapshot_voices(choice, d),
                Ok(ResultView::Exported(path)) => self.view.exported = Some(path),
            }
            break;
        }
    }

    pub fn dispatch(
        &mut self,
        cmd: FileCommand,
        settings: &super::Settings,
        store: &voai_models::ModelStore,
        allow_play: bool,
    ) {
        if let Err(error) = self.handle(cmd, settings, store, allow_play) {
            self.view.error = Some(error);
        }
    }

    fn handle(
        &mut self,
        cmd: FileCommand,
        settings: &super::Settings,
        store: &voai_models::ModelStore,
        allow_play: bool,
    ) -> Result<(), String> {
        match cmd {
            FileCommand::Stop => {
                self.stop();
                self.view.error = None;
                self.view.exported = None;
                return Ok(());
            }
            FileCommand::Cancel => {
                self.cancel();
                return Ok(());
            }
            _ if self.job.is_some() => return Err(tr("app.files.task_busy").into()),
            _ => {}
        }
        self.view.error = None;
        self.view.exported = None;
        match cmd {
            FileCommand::Import(source) => {
                let settings = settings.clone();
                let store = store.clone();
                let options = self.options.clone();
                self.stop();
                let tools = self.tools.clone();
                let voices = self.selected_voices().filter(|_| options.identify_speakers);
                self.start("app.files.preparing", true, move |_, work| {
                    let pipeline = options.pipeline(&settings, &store, voices)?;
                    job::import(&tools, source, &pipeline, work).map(ResultView::Imported)
                });
            }
            FileCommand::Open(id) => {
                self.stop();
                self.start("app.files.opening", false, move |dir, _| {
                    store::Store::open(dir)?.load(id).map(ResultView::Document)
                });
            }
            FileCommand::SetSpeech(id) => {
                self.select(
                    FileOptions {
                        speech: Some(id),
                        ..self.options.clone()
                    },
                    self.downloaded.clone(),
                );
            }
            FileCommand::SetText(id) => {
                self.select(
                    FileOptions {
                        text: id,
                        ..self.options.clone()
                    },
                    self.downloaded.clone(),
                );
            }
            FileCommand::SetIdentifySpeakers(enabled) => {
                self.select(
                    FileOptions {
                        identify_speakers: enabled,
                        ..self.options.clone()
                    },
                    self.downloaded.clone(),
                );
            }
            FileCommand::InstallVoices => {
                let options = self.options.clone();
                let models = self.model_dir.clone();
                self.start("app.speakers.downloading", true, move |_, work| {
                    let path = super::diarization::download(&models, &work.cancelled, |p| {
                        work.progress("app.speakers.downloading", p)
                    })?;
                    Ok(ResultView::Voices(
                        FileOptions {
                            use_local: false,
                            identify_speakers: true,
                            ..options
                        },
                        Some(path),
                    ))
                });
            }
            FileCommand::ConnectVoices(folder) => {
                let options = self.options.clone();
                let d = self.downloaded.clone();
                self.start("app.speakers.checking", true, move |_, _| {
                    let folder = folder
                        .canonicalize()
                        .map_err(|_| tr("app.files.folder_unavailable"))?;
                    super::diarization::validate(&folder)?;
                    Ok(ResultView::Voices(
                        FileOptions {
                            folder: Some(folder),
                            use_local: true,
                            identify_speakers: true,
                            ..options
                        },
                        d,
                    ))
                });
            }
            FileCommand::UseDownloadedVoices | FileCommand::UseLocalVoices => {
                let use_local = matches!(cmd, FileCommand::UseLocalVoices);
                if (use_local && self.view.voices_local.is_none())
                    || (!use_local && self.downloaded.is_none())
                {
                    return Err(tr("app.speakers.choose_models").into());
                }
                self.select(
                    FileOptions {
                        use_local,
                        identify_speakers: true,
                        ..self.options.clone()
                    },
                    self.downloaded.clone(),
                );
            }
            FileCommand::Play(index) => {
                if !allow_play {
                    return Err(tr("app.dictation.busy").into());
                }
                let was_playing = self.view.playing == Some(index);
                self.stop();
                if was_playing {
                    return Ok(());
                }
                let doc = self
                    .view
                    .document
                    .as_ref()
                    .ok_or(tr("app.files.select_transcript"))?;
                let line = doc
                    .lines
                    .get(index)
                    .ok_or(tr("app.files.line_unavailable"))?;
                if SourceStamp::read(&doc.source)? != doc.source_stamp {
                    return Err(tr("app.files.playback_source_changed").into());
                }
                let player = Command::new(&self.tools.media)
                    .env(voai_l10n::WORKER_LANGUAGE_ENV, voai_l10n::language())
                    .arg("play")
                    .arg(&doc.source)
                    .arg(line.start_ms.to_string())
                    .arg(line.end_ms.to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| tr("app.audio.start_playback"))?;
                self.playback = Some(ChildGuard(player));
                self.view.playing = Some(index);
            }
            FileCommand::Export { path, format } => {
                let doc = self
                    .view
                    .document
                    .clone()
                    .ok_or(tr("app.files.select_transcript"))?;
                self.start("app.files.exporting", false, move |dir, _| {
                    store::export(&path, format, &doc, dir.parent().unwrap_or(dir))?;
                    Ok(ResultView::Exported(path))
                });
            }
            change @ (FileCommand::Assign { .. }
            | FileCommand::Rename { .. }
            | FileCommand::AddSpeaker { .. }) => {
                let mut doc = self
                    .view
                    .document
                    .as_deref()
                    .cloned()
                    .ok_or(tr("app.files.select_transcript"))?;
                match change {
                    FileCommand::Assign { line, speaker } => doc.assign(line, speaker)?,
                    FileCommand::Rename { speaker, name } => {
                        if !doc.speakers.contains_key(&speaker) {
                            return Err(tr("app.speakers.unavailable").into());
                        }
                        doc.set_name(speaker, name)?;
                    }
                    FileCommand::AddSpeaker { line, name } => {
                        let id = doc
                            .speakers
                            .keys()
                            .next_back()
                            .copied()
                            .unwrap_or(0)
                            .checked_add(1)
                            .ok_or(tr("app.speakers.limit"))?;
                        doc.set_name(id, name)?;
                        doc.assign(line, Some(id))?;
                    }
                    _ => unreachable!(),
                }
                self.persist(doc);
            }
            FileCommand::Stop | FileCommand::Cancel => unreachable!(),
        }
        Ok(())
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        self.stop();
        if let Some(job) = self.job.take() {
            job.cancelled.store(true, Ordering::Release);
            let _ = job.worker.join();
        }
        for worker in self.retired.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "files/tests.rs"]
mod tests;
