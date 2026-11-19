#![cfg(target_os = "macos")]

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use voai_models::{ConnectedModel, ModelKind};

/// Run with VOAI_TEST_SPEECH_MODEL and VOAI_TEST_TEXT_MODEL pointing to real
/// weights. Build the workspace first so both bundled executables are available.
#[test]
#[ignore = "requires real GGML/GGUF model files and the built text helper"]
fn connects_external_models_with_bundled_engines() {
    let app = Path::new(env!("CARGO_BIN_EXE_voai"));
    let text_helper = app.with_file_name("voai-text-helper");
    let ram_limit = voai_models::local_model_size_limit().unwrap();
    for (kind, env_name, binary, args) in [
        (
            ModelKind::Speech,
            "VOAI_TEST_SPEECH_MODEL",
            app,
            &["--speech-worker", "--validate-v1"][..],
        ),
        (
            ModelKind::Text,
            "VOAI_TEST_TEXT_MODEL",
            text_helper.as_path(),
            &["--validate-model-v1"][..],
        ),
    ] {
        let original = PathBuf::from(std::env::var_os(env_name).expect(env_name));
        let model = ConnectedModel::connect(&original, kind, ram_limit, |path| {
            let mut child = Command::new(binary)
                .args(args)
                .arg(path)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if let Some(status) = child.try_wait()? {
                    assert!(
                        status.success(),
                        "native validator for {kind:?} exited with {status}"
                    );
                    break;
                }
                if Instant::now() >= deadline {
                    child.kill()?;
                    child.wait()?;
                    panic!("native validator for {kind:?} exceeded timeout");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        })
        .unwrap();
        let encoded = serde_json::to_vec(&model).unwrap();
        let restored: ConnectedModel = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            restored.resolve().unwrap(),
            original.canonicalize().unwrap()
        );
        assert_eq!(model.id(), restored.id());
        assert_eq!(model.bytes(), original.metadata().unwrap().len());
    }
}
