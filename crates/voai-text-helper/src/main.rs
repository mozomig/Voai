//! One-shot local editor. Only the public path to the GGUF model is in argv.
//! The app sends two length-prefixed UTF-8 messages (system, user), keeps stdin
//! open during inference, and reads the final UTF-8 response from stdout.
use anyhow::{Context, Result, ensure};
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{LlamaChatMessage, LlamaModel, params::LlamaModelParams},
    sampling::LlamaSampler,
};
use std::{
    io::{self, Read, Write},
    num::NonZeroU32,
    path::Path,
};

fn edit(path: &Path, instruction: String, transcript: String) -> Result<String> {
    const CONTEXT: u32 = 4096;
    const BATCH: usize = 256;
    let backend = LlamaBackend::init()?;
    let model = LlamaModel::load_from_file(
        &backend,
        path,
        &LlamaModelParams::default().with_n_gpu_layers(u32::MAX),
    )?;
    let messages = [
        LlamaChatMessage::new("system".into(), instruction)?,
        LlamaChatMessage::new("user".into(), transcript)?,
    ];
    let prompt = model.apply_chat_template(&model.chat_template(None)?, &messages, true)?;
    let vocab = model.vocab();
    // Some GGUF chat templates explicitly supply BOS (e.g. Gemma).
    let mut tokens = vocab.tokenize(prompt.as_bytes(), false, true);
    if vocab.should_add_bos() && tokens.first() != Some(&vocab.bos()) {
        tokens.insert(0, vocab.bos());
    }
    ensure!(
        !tokens.is_empty() && tokens.len() + 64 < CONTEXT as usize,
        "Фраза превышает контекст модели"
    );
    let mut ctx = model.new_context(
        &backend,
        LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(CONTEXT))
            .with_n_batch(BATCH as u32),
    )?;
    let mut batch = LlamaBatch::new(BATCH, 1);
    let mut pos = 0i32;
    for chunk in tokens.chunks(BATCH) {
        batch.clear();
        for &token in chunk {
            batch.add(token, pos, &[0], pos as usize + 1 == tokens.len())?;
            pos += 1;
        }
        ctx.decode(&mut batch)?;
    }
    let mut sampler = LlamaSampler::greedy();
    let mut generated = Vec::new();
    while pos < CONTEXT as i32 {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        if vocab.is_eog(token) {
            let bytes = vocab.detokenize(&generated, true, false);
            return Ok(String::from_utf8(bytes)?);
        }
        generated.push(token);
        batch.clear();
        batch.add(token, pos, &[0], true)?;
        ctx.decode(&mut batch)?;
        pos += 1;
    }
    anyhow::bail!("Модель исчерпала контекст до завершения ответа")
}

fn read_message(input: &mut impl Read, max_bytes: usize) -> Result<String> {
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    ensure!(
        (1..=max_bytes).contains(&len),
        "Сообщение пусто или слишком велико"
    );
    let mut data = vec![0; len];
    input.read_exact(&mut data)?;
    Ok(String::from_utf8(data)?)
}

fn watch_parent() {
    // Terminate even when Voai is force-quit mid-inference.
    std::thread::spawn(|| {
        let _ = io::stdin().read(&mut [0]);
        std::process::exit(1);
    });
}

fn validate_model(path: &Path) -> Result<()> {
    watch_parent();
    let backend = LlamaBackend::init()?;
    let model = LlamaModel::load_from_file(
        &backend,
        path,
        &LlamaModelParams::default()
            .with_n_gpu_layers(0)
            .with_use_mmap(true),
    )?;
    let messages = [
        LlamaChatMessage::new("system".into(), "Proofread the transcript".into())?,
        LlamaChatMessage::new("user".into(), "test".into())?,
    ];
    model.apply_chat_template(&model.chat_template(None)?, &messages, true)?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let operation = args.next().context("Не задан режим работы движка")?;
    let path = args.next().context("Укажите путь к GGUF модели")?;
    if operation == "--validate-model-v1" {
        return validate_model(Path::new(&path));
    }
    ensure!(
        operation == "--edit-v2",
        "Неподдерживаемая версия протокола"
    );
    let mut stdin = io::stdin().lock();
    let instruction = read_message(&mut stdin, 8192)?;
    let transcript = read_message(&mut stdin, 120_000)?;
    drop(stdin);
    watch_parent();
    let text = edit(Path::new(&path), instruction, transcript)?;
    ensure!(!text.trim().is_empty(), "Пустой результат обработки");
    io::stdout().lock().write_all(text.trim().as_bytes())?;
    Ok(())
}
