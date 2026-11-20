//! User-visible local history. One worker serializes disk access and playback.
mod store;

use objc2::{AllocAnyThread, rc::Retained};
use objc2_avf_audio::AVAudioPlayer;
use objc2_foundation::NSData;
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
use store::Store;
use voai_l10n::tr;

#[derive(Clone, PartialEq)]
pub enum Outcome {
    Off,
    Edited(String),
    Failed,
}

#[derive(Clone, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub started: i64,
    pub duration_ms: u64,
    pub original: String,
    pub outcome: Outcome,
}

pub(super) struct NewEntry {
    pub started: i64,
    pub audio: Vec<f32>,
    pub original: String,
    pub outcome: Outcome,
}

#[derive(Clone, Default, PartialEq)]
pub struct HistoryView {
    pub entries: Arc<[Entry]>,
    pub playing: Option<i64>,
    pub error: Option<&'static str>,
    pub loading: bool,
}

pub(super) enum Request {
    Load,
    Save(NewEntry),
    Delete(i64),
    Play(i64),
    Stop,
}

pub(super) struct History {
    requests: Option<mpsc::Sender<Request>>,
    updates: mpsc::Receiver<HistoryView>,
    worker: Option<thread::JoinHandle<()>>,
    pub view: HistoryView,
}

impl History {
    pub fn new(dir: PathBuf, wake: async_channel::Sender<()>) -> Self {
        let (send, receive) = mpsc::channel();
        let (publish, updates) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut state = Worker::default();
            let mut db = None;
            let mut command = Request::Load;
            loop {
                let result = objc2::rc::autoreleasepool(|_| {
                    let store = match &mut db {
                        Some(store) => store,
                        None => db.insert(Store::open(&dir)?),
                    };
                    state.handle(command, store)
                });
                state.view.error = result.err();
                let _ = publish.send(state.view.clone());
                let _ = wake.try_send(());
                let Some(next) = state.next(&receive) else {
                    break;
                };
                command = next;
            }
        });
        Self {
            requests: Some(send),
            updates,
            worker: Some(worker),
            view: HistoryView {
                loading: true,
                ..Default::default()
            },
        }
    }
    pub fn send(&mut self, cmd: Request) {
        if self
            .requests
            .as_ref()
            .is_none_or(|sender| sender.send(cmd).is_err())
        {
            self.view.error = Some(tr("app.history.unavailable"));
        }
    }
    pub fn poll(&mut self) {
        for update in self.updates.try_iter() {
            self.view = update;
        }
    }
}

#[derive(Default)]
struct Worker {
    view: HistoryView,
    player: Option<Retained<AVAudioPlayer>>,
}

impl Worker {
    fn handle(&mut self, command: Request, store: &mut Store) -> Result<(), &'static str> {
        match command {
            Request::Load => {}
            Request::Save(entry) => store.save(entry)?,
            Request::Delete(id) => store.delete(id)?,
            Request::Play(id) => {
                self.stop();
                let bytes = NSData::with_bytes(&store.audio(id)?);
                // AVAudioPlayer retains its data. All access stays on the worker.
                let audio =
                    unsafe { AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &bytes) }
                        .map_err(|_| tr("app.history.open_recording"))?;
                if !unsafe { audio.play() } {
                    return Err(tr("app.audio.start_playback"));
                }
                self.player = Some(audio);
                self.view.playing = Some(id);
                return Ok(());
            }
            Request::Stop => {
                self.stop();
                return Ok(());
            }
        }
        self.view.entries = store.list()?.into();
        if self
            .view
            .playing
            .is_some_and(|id| !self.view.entries.iter().any(|row| row.id == id))
        {
            self.stop();
        }
        Ok(())
    }

    fn next(&self, requests: &mpsc::Receiver<Request>) -> Option<Request> {
        loop {
            // An idle history has no timer. Poll only while audio is playing.
            if let Some(audio) = &self.player {
                match requests.recv_timeout(Duration::from_millis(120)) {
                    Ok(cmd) => return Some(cmd),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                    Err(mpsc::RecvTimeoutError::Timeout) if unsafe { audio.isPlaying() } => {
                        continue;
                    }
                    Err(_) => return Some(Request::Stop),
                }
            }
            match requests.recv().ok()? {
                Request::Stop => continue,
                cmd => return Some(cmd),
            }
        }
    }

    fn stop(&mut self) {
        if let Some(audio) = self.player.take() {
            unsafe { audio.stop() };
        }
        self.view.playing = None;
    }
}

impl Drop for History {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
