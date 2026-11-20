use super::{
    document::{SourceStamp, Turn, Word, align},
    *,
};
use std::{
    fs,
    time::{Duration, Instant},
};

fn word(s: u64, e: u64, t: &str) -> Word {
    Word {
        start_ms: s,
        end_ms: e,
        text: t.into(),
    }
}
fn turn(s: u64, e: u64, id: u32) -> Turn {
    Turn {
        start_ms: s,
        end_ms: e,
        speaker: id,
    }
}

#[test]
fn assigns_words_across_a_change_and_marks_overlapping_speech() {
    let lines = align(
        vec![
            word(100, 400, "Привет."),
            word(430, 600, "Да,"),
            word(650, 850, "слушаю."),
            word(1_800_000, 1_800_400, "Снова."),
        ],
        vec![
            turn(0, 450, 9),
            turn(450, 900, 4),
            turn(700, 900, 9),
            turn(1_800_000, 1_800_450, 9),
        ],
    );
    assert_eq!(
        lines
            .iter()
            .map(|r| (r.speaker, r.review))
            .collect::<Vec<_>>(),
        [
            (Some(9), false),
            (Some(4), false),
            (Some(4), true),
            (Some(9), false)
        ]
    );
    assert_eq!(lines[3].start_ms, 1_800_000);
}

#[test]
fn saves_renames_and_exports_the_selected_document() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("dialogue.m4a");
    fs::write(&src, b"fixture").unwrap();
    let private = tmp.path().join("Voai");
    let db = store::Store::open(&private.join("Transcripts")).unwrap();
    let mut doc = FileDocument {
        id: String::new(),
        title: "dialogue".into(),
        source_stamp: SourceStamp::read(&src).unwrap(),
        source: src,
        warning: None,
        speakers: Default::default(),
        lines: align(vec![word(3600010, 3600780, "Привет!")], vec![]),
    };
    db.save(&mut doc).unwrap();
    doc.set_name(7, "Анна".into()).unwrap();
    doc.assign(0, Some(7)).unwrap();
    db.save(&mut doc).unwrap();
    let loaded = db.load(doc.id.clone()).unwrap();
    assert_eq!(db.list().unwrap().len(), 1);
    assert_eq!(
        loaded.export_text(ExportFormat::Srt),
        "1\n01:00:00,010 --> 01:00:00,780\nАнна: Привет!\n\n"
    );
    let target = tmp.path().join("dialogue.txt");
    store::export(&target, ExportFormat::Txt, &loaded, &private).unwrap();
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "[01:00:00] Анна: Привет!\n"
    );
}

#[test]
fn file_model_choices_survive_reopening_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let db = store::Store::open(dir.path()).unwrap();
    let defaults = db.pipeline_options().unwrap();
    assert!(defaults.text.is_none() && defaults.speech.is_none());
    let folder = dir.path().join("custom-voices");
    db.save_choice(&FileOptions {
        speech: Some("medium".into()),
        text: Some("qwen".into()),
        folder: Some(folder.clone()),
        use_local: true,
        identify_speakers: false,
    })
    .unwrap();
    drop(db);
    let restored = store::Store::open(dir.path())
        .unwrap()
        .pipeline_options()
        .unwrap();
    assert_eq!(restored.speech.as_deref(), Some("medium"));
    assert_eq!(restored.text.as_deref(), Some("qwen"));
    assert_eq!(restored.folder, Some(folder));
    assert!(restored.use_local && !restored.identify_speakers);
}

fn until_idle(f: &mut Files) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while f.job.is_some() {
        f.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(f.view.error.is_none());
}

#[test]
fn cancellation_discards_a_late_result_without_changing_the_library() {
    let tmp = tempfile::tempdir().unwrap();
    let (wake, _rx) = async_channel::bounded(1);
    let mut files = Files::new(tmp.path(), wake);
    until_idle(&mut files);
    let (send, recv) = mpsc::channel();
    files.start("Import", true, move |_, _| {
        recv.recv().unwrap();
        Err("late".into())
    });
    files.cancel();
    send.send(()).unwrap();
    drop(files);
    assert!(
        store::Store::open(&tmp.path().join("Transcripts"))
            .unwrap()
            .list()
            .unwrap()
            .is_empty()
    );
}

#[test]
#[ignore = "needs a signed app, a selected local media file and downloaded weights"]
fn imports_media_with_packaged_workers() {
    use std::env;
    let from_env = |key| PathBuf::from(env::var_os(key).expect("Missing QA path"));
    let tools = Tools::from_executable(from_env("VOAI_FILE_QA_APP").join("Contents/MacOS/voai"));
    let source = from_env("VOAI_FILE_QA_MEDIA");
    let pipeline = job::Pipeline {
        model: from_env("VOAI_FILE_QA_WHISPER"),
        language: "ru".into(),
        voices: env::var_os("VOAI_FILE_QA_VOICES").map(Into::into),
        editor: env::var_os("VOAI_FILE_QA_EDITOR").map(Into::into),
        prompt: voai_models::DEFAULT_PROCESSING_PROMPT.into(),
    };
    let (updates, rx) = mpsc::channel();
    let (wake, _) = async_channel::bounded(1);
    let work = Work {
        cancelled: Arc::new(false.into()),
        updates,
        wake,
    };
    let started = Instant::now();
    let progress = thread::spawn(move || {
        let mut last = String::new();
        while let Ok(update) = rx.recv() {
            if let Update::Progress(p) = update
                && p.stage != last
            {
                println!(
                    "stage {} after {:.1}s",
                    p.stage,
                    started.elapsed().as_secs_f32()
                );
                last = p.stage;
            }
        }
    });
    let mut doc = job::import(&tools, source, &pipeline, &work).unwrap();
    drop(work);
    progress.join().unwrap();
    assert!(!doc.lines.is_empty());
    let expect_fallback = env::var_os("VOAI_FILE_QA_FALLBACK").is_some();
    if expect_fallback {
        assert!(doc.warning.is_some() && doc.speakers.is_empty());
    } else if pipeline.voices.is_some() {
        assert!(doc.warning.is_none() && !doc.speakers.is_empty());
    }
    if pipeline.editor.is_some() {
        assert!(doc.lines.iter().any(|l| l.original.is_some()));
    }
    let tmp = tempfile::tempdir().unwrap();
    let db = store::Store::open(tmp.path()).unwrap();
    db.save(&mut doc).unwrap();
    let read = db.load(doc.id).unwrap();
    assert!(read.lines.iter().all(|l| l.end_ms > l.start_ms));
    assert!(!read.export_text(ExportFormat::Srt).is_empty());
    println!(
        "lines={} speakers={} last_ms={} elapsed_s={:.1}",
        read.lines.len(),
        read.speakers.len(),
        read.lines.last().unwrap().end_ms,
        started.elapsed().as_secs_f32()
    );
}

#[test]
#[ignore = "needs a signed app, a multi-window synthetic recording and local Whisper weights"]
fn cancel_stops_packaged_recognition_without_saving() {
    let required = |key| PathBuf::from(std::env::var_os(key).expect("Missing QA path"));
    let tools = Tools::from_executable(required("VOAI_FILE_QA_APP").join("Contents/MacOS/voai"));
    let source = required("VOAI_FILE_QA_MEDIA");
    let pipeline = job::Pipeline {
        model: required("VOAI_FILE_QA_WHISPER"),
        language: "ru".into(),
        voices: None,
        editor: None,
        prompt: String::new(),
    };
    let tmp = tempfile::tempdir().unwrap();
    let (wake, _rx) = async_channel::bounded(1);
    let mut files = Files::new(tmp.path(), wake);
    until_idle(&mut files);
    files.start("Подготовка", true, move |_, work| {
        job::import(&tools, source, &pipeline, work).map(ResultView::Imported)
    });

    let parent = std::process::id().to_string();
    let deadline = Instant::now() + Duration::from_secs(90);
    let child = loop {
        files.poll();
        assert!(files.job.is_some() && Instant::now() < deadline);
        if files
            .view
            .progress
            .as_ref()
            .is_some_and(|p| p.stage == "Распознавание речи" && (1..100).contains(&p.percent))
        {
            let list = Command::new("/usr/bin/pgrep")
                .args(["-P", &parent, "-f", " --file-worker "])
                .output()
                .unwrap();
            if let Some(pid) = String::from_utf8_lossy(&list.stdout).lines().next() {
                break pid.to_owned();
            }
        }
        thread::sleep(Duration::from_millis(10));
    };
    files.cancel();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !files.retired.is_empty() {
        assert!(
            Instant::now() < deadline,
            "Worker was not reaped after cancellation"
        );
        thread::sleep(Duration::from_millis(10));
        files.poll();
    }
    assert!(
        !Command::new("/bin/ps")
            .args(["-p", &child, "-o", "pid="])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(files.view.document.is_none() && files.view.library.is_empty());
    assert!(
        store::Store::open(&files.dir)
            .unwrap()
            .list()
            .unwrap()
            .is_empty()
    );
}
