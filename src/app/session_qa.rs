use super::*;
use std::{env, fs, time::Duration};
use voai_models::{ConnectedModel, ModelKind, local_model_size_limit, validate_text_model};

fn fixture_path(variable: &str) -> PathBuf {
    env::var_os(variable)
        .expect("run tests/macos/dictation.py")
        .into()
}

fn tick_until(
    session: &mut Session,
    store: &ModelStore,
    mut reached: impl FnMut(&Session) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        session.tick(store, true);
        assert!(session.message.is_empty(), "{}", session.message);
        if reached(session) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out. Last text: {:?}",
            session.text
        );
        thread::sleep(Duration::from_millis(15));
    }
}

#[test]
#[ignore = "requires bundled workers, local GGML/GGUF weights and the Russian speech fixture"]
fn recorded_speech_reaches_test_field() {
    let temp = tempfile::tempdir().unwrap();
    let store = ModelStore::new(temp.path().into());
    let mut settings = Settings::default();
    settings.mode = crate::app::Mode::Local;
    let qwen = ConnectedModel::connect(
        &fixture_path("VOAI_QA_QWEN"),
        ModelKind::Text,
        local_model_size_limit().unwrap(),
        validate_text_model,
    )
    .unwrap();
    let text_id = qwen.id();
    settings.connect(qwen);
    settings.select_model(&store, text_id).unwrap();
    settings
        .set_processing_prompt(
            "Translate the user's text into English. Return only the translation.".into(),
        )
        .unwrap();
    let bytes = fs::read(fixture_path("VOAI_QA_PCM")).unwrap();
    let (frames, remainder) = bytes.as_chunks::<2>();
    assert!(remainder.is_empty());
    let samples: Vec<_> = frames
        .iter()
        .map(|&frame| f32::from(i16::from_le_bytes(frame)) / 32768.)
        .collect();
    let rate = env::var("VOAI_QA_RATE").unwrap().parse().unwrap();
    let start = |session: &mut Session| {
        let (recording, eof) = voai_speech::start_fixture(
            fixture_path("VOAI_QA_WHISPER"),
            Some(settings.language.clone()),
            fixture_path("VOAI_QA_WORKER"),
            samples.clone(),
            rate,
        )
        .unwrap();
        session.begin(&settings, Destination::Test, recording);
        eof
    };

    let mut session = Session::default();
    let eof = start(&mut session);
    let mut all_samples = false;
    let mut heard = false;
    tick_until(&mut session, &store, |s| {
        assert!(s.phase == Phase::Listening && s.result.is_none());
        all_samples |= eof.try_recv().is_ok();
        heard |= s.level > 0.;
        all_samples && heard && !s.text.is_empty()
    });
    println!("Live partial: {}", session.text);
    session.finish();
    assert!(session.phase == Phase::Finishing);
    tick_until(&mut session, &store, |s| s.phase == Phase::Editing);
    println!("Final transcript: {}", session.text);
    assert!(session.text.to_lowercase().contains("пятниц"));
    tick_until(&mut session, &store, |s| s.result.is_some());
    assert!(session.phase == Phase::Idle);
    let (_, delivered) = session.result.take().unwrap();
    println!("Test field received: {delivered}");
    assert!(delivered.to_ascii_lowercase().contains("friday"));
    assert!(!delivered.to_lowercase().contains("пятниц"));
    session.tick(&store, true);
    assert!(session.result.is_none());

    // Cancel a second real capture after the warm worker begins decoding it.
    let _eof = start(&mut session);
    tick_until(&mut session, &store, |s| !s.text.is_empty());
    let cancelled = session.cancelled.clone();
    session.reset();
    assert!(cancelled.load(Ordering::Acquire));
    session.finish();
    session.tick(&store, true);
    assert!(session.phase == Phase::Idle && session.result.is_none() && session.text.is_empty());
    println!("Cancelled live session discarded");
}
