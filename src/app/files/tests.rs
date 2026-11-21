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
        notices: Vec::new(),
        legacy_warning: None,
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
fn plain_document_joins_chunks_and_preserves_timing_when_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("voice.wav");
    fs::write(&source, b"fixture").unwrap();
    let mut doc = FileDocument {
        id: String::new(),
        title: "Запись".into(),
        source_stamp: SourceStamp::read(&source).unwrap(),
        source,
        speakers: Default::default(),
        lines: align(
            vec![
                word(0, 150, "Первая"),
                word(1200, 1800, "фраза."),
                word(7500, 9700, "Вторая фраза."),
            ],
            vec![],
        ),
        notices: Vec::new(),
        legacy_warning: Some("Расшифровано без определения спикеров. Обработка прервалась.".into()),
    };
    let db = store::Store::open(dir.path()).unwrap();
    db.save(&mut doc).unwrap();
    let read = db.load(doc.id.clone()).unwrap();
    assert_eq!(read.lines.len(), 3);
    assert_eq!(read.paragraphs(), ["Первая фраза.", "Вторая фраза."]);
    assert_eq!(
        read.warning_text().as_deref(),
        Some("Обработка прервалась.")
    );
    assert_eq!(read.lines[2].start_ms, 7500);
    doc.legacy_warning = Some("Не удалось определить спикеров. Текст и таймкоды сохранены. Обработка прервалась. У оставшихся реплик сохранён исходный текст.".into());
    db.save(&mut doc).unwrap();
    let mut migrated = db.load(doc.id).unwrap();
    assert!(migrated.legacy_warning.is_none());
    assert_eq!(
        migrated.warning_text(),
        Some(format!(
            "{} {}",
            tr("app.files.speakers_skipped"),
            tr("app.files.remaining_original")
        ))
    );
    db.save(&mut migrated).unwrap();
    let saved = db.load(migrated.id.clone()).unwrap();
    assert!(saved == migrated);
    let stored = serde_json::to_value(&saved).unwrap();
    assert!(stored.get("warning").is_none());
    assert_eq!(
        stored["notices"],
        serde_json::json!(["speakers_unavailable", "editing_interrupted"])
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
        text: Some(FileEditor::Local("qwen".into())),
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
    assert_eq!(restored.text, Some(FileEditor::Local("qwen".into())));
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
    let qa_cli = env::var("VOAI_FILE_QA_CLI")
        .ok()
        .map(|name| voai_models::AgentCli::from_name(&name).expect("Unknown QA CLI"));
    let editor = if let Some(agent) = qa_cli {
        let mut profiles = voai_models::AgentProfiles::default();
        profiles.set(
            agent,
            voai_models::AgentConfig {
                effort: "low".into(),
                ..Default::default()
            },
        );
        Some(job::Editor::Service(
            crate::app::ServiceModel::Cli {
                agent,
                model: String::new(),
            },
            profiles,
        ))
    } else {
        env::var_os("VOAI_FILE_QA_EDITOR").map(|p| job::Editor::Local(p.into()))
    };
    let pipeline = job::Pipeline {
        model: from_env("VOAI_FILE_QA_WHISPER"),
        language: "ru".into(),
        voices: env::var_os("VOAI_FILE_QA_VOICES").map(Into::into),
        editor,
        // Translation makes a successful edit observable even for a correctly punctuated fixture.
        prompt: "Translate this phrase into English. Return only the translation.".into(),
    };
    let (updates, rx) = mpsc::channel();
    let (wake, _) = async_channel::bounded(1);
    let work = Work {
        cancelled: Arc::new(false.into()),
        observer: Some((updates, wake)),
    };
    let started = Instant::now();
    let progress = thread::spawn(move || {
        let mut last = String::new();
        while let Ok(update) = rx.recv() {
            if let Update::Progress(p) = update
                && p.stage_key != last
            {
                println!(
                    "stage {} after {:.1}s",
                    p.stage_key,
                    started.elapsed().as_secs_f32()
                );
                last = p.stage_key.to_owned();
            }
        }
    });
    let mut doc = job::import(&tools, source, &pipeline, &work).unwrap();
    drop(work);
    progress.join().unwrap();
    assert!(!doc.lines.is_empty());
    let expect_fallback = env::var_os("VOAI_FILE_QA_FALLBACK").is_some();
    if expect_fallback {
        assert!(doc.warning_text().is_some() && doc.speakers.is_empty());
    } else if pipeline.voices.is_some() {
        assert!(doc.warning_text().is_none() && !doc.speakers.is_empty());
    }
    if pipeline.editor.is_some() {
        assert!(!doc.notices.contains(&document::Notice::EditingInterrupted));
        assert!(
            doc.lines
                .iter()
                .all(|l| l.text.chars().any(|c| c.is_ascii_alphabetic()))
        );
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
#[ignore = "needs signed helpers, local Whisper weights, a voice message and a video fixture"]
fn transcribes_bot_voice_and_video_with_packaged_workers() {
    let path = |key| PathBuf::from(std::env::var_os(key).expect("Missing QA path"));
    let data = tempfile::tempdir().unwrap();
    let store = voai_models::ModelStore::new(data.path().join("Models"));
    let model = voai_models::ConnectedModel::connect(
        &path("VOAI_FILE_QA_WHISPER"),
        voai_models::ModelKind::Speech,
        u64::MAX,
        |_| Ok(()),
    )
    .unwrap();
    let options = FileOptions {
        speech: Some(model.id()),
        ..Default::default()
    };
    let mut settings = super::super::Settings::default();
    settings.connect(model);
    let library = store::Store::open(&data.path().join("Transcripts")).unwrap();
    library.save_choice(&options).unwrap();
    let (wake, _) = async_channel::bounded(1);
    let mut files = Files::new(data.path(), wake);
    until_idle(&mut files);
    files.tools = Tools::from_executable(path("VOAI_FILE_QA_APP").join("Contents/MacOS/voai"));
    let bot = files.bot_pipeline(&settings, &store).unwrap();
    for media in ["VOAI_BOT_QA_VOICE", "VOAI_BOT_QA_VIDEO"] {
        let transcript = bot.transcribe(path(media), Arc::new(false.into())).unwrap();
        assert!(transcript.chars().any(char::is_alphabetic));
        assert_eq!(
            bot.transcribe(path(media), Arc::new(true.into()))
                .unwrap_err(),
            tr("app.files.import_cancelled")
        );
    }
    assert!(files.view.document.is_none() && library.list().unwrap().is_empty());
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
    files.start("app.files.preparing", true, move |_, work| {
        job::import(&tools, source, &pipeline, work).map(ResultView::Imported)
    });

    let parent = std::process::id().to_string();
    let deadline = Instant::now() + Duration::from_secs(90);
    let child = loop {
        files.poll();
        assert!(files.job.is_some() && Instant::now() < deadline);
        if files.view.progress.as_ref().is_some_and(|p| {
            p.stage_key == "app.files.recognizing" && (1..100).contains(&p.percent)
        }) {
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

#[test]
fn file_services_persist_and_never_reach_the_bot() {
    use crate::app::ServiceModel;
    let dir = tempfile::tempdir().unwrap();
    let db = store::Store::open(dir.path()).unwrap();
    let store = voai_models::ModelStore::new(dir.path().into());
    let settings = crate::app::Settings::default();
    for target in [
        ServiceModel::Server {
            endpoint: "http://127.0.0.1:11434/v1".into(),
            model: "local".into(),
        },
        ServiceModel::Api {
            provider: voai_models::Provider::OpenAI,
            model: "gpt-4o".into(),
        },
        ServiceModel::Cli {
            agent: voai_models::AgentCli::Claude,
            model: "sonnet".into(),
        },
    ] {
        db.save_choice(&FileOptions {
            text: Some(FileEditor::Service(target.clone())),
            ..Default::default()
        })
        .unwrap();
        let options = db.pipeline_options().unwrap();
        assert!(
            matches!(options.editor(&settings, &store, ImportSource::File).unwrap(), Some(job::Editor::Service(t, _)) if t == target)
        );
        assert!(
            options
                .editor(&settings, &store, ImportSource::Telegram)
                .unwrap()
                .is_none()
        );
    }
    let legacy: FileOptions = serde_json::from_str(r#"{"text":"existing-gguf"}"#).unwrap();
    assert_eq!(legacy.text, Some(FileEditor::Local("existing-gguf".into())));
}
