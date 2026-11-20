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
use voai_l10n::tr;

fn prompt_tokens(
    model: &LlamaModel,
    instruction: &str,
    transcript: String,
) -> Result<Vec<llama_cpp_2::token::LlamaToken>> {
    let vocab = model.vocab();
    let template = model.chat_template(None)?;
    // llama.cpp's legacy chat formatter lacks Gemma 4. Use the model's
    // two-turn, text-only format with thinking disabled. The user text and
    // instruction must not be interpreted as special turn delimiters.
    if model.meta_val_str("general.architecture").as_deref().ok() == Some("gemma4")
        && template.to_str()?.contains("<|turn>")
    {
        let mut tokens = vec![vocab.bos()];
        for (part, special) in [
            ("<|turn>system\n", true),
            (instruction.trim(), false),
            ("<turn|>\n<|turn>user\n", true),
            (transcript.trim(), false),
            ("<turn|>\n<|turn>model\n", true),
        ] {
            tokens.extend(vocab.tokenize(part.as_bytes(), false, special));
        }
        return Ok(tokens);
    }
    let messages = [
        LlamaChatMessage::new("system".into(), instruction.to_owned())?,
        LlamaChatMessage::new("user".into(), transcript)?,
    ];
    let prompt = model.apply_chat_template(&template, &messages, true)?;
    let mut tokens = vocab.tokenize(prompt.as_bytes(), false, true);
    if vocab.should_add_bos() && tokens.first() != Some(&vocab.bos()) {
        tokens.insert(0, vocab.bos());
    }
    Ok(tokens)
}

fn edit(
    backend: &LlamaBackend,
    model: &LlamaModel,
    instruction: &str,
    transcript: String,
) -> Result<String> {
    const CONTEXT: u32 = 4096;
    const BATCH: usize = 256;
    let tokens = prompt_tokens(model, instruction, transcript)?;
    let vocab = model.vocab();
    ensure!(
        !tokens.is_empty() && tokens.len() + 64 < CONTEXT as usize,
        tr("text.context.input")
    );
    let mut ctx = model.new_context(
        backend,
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
    anyhow::bail!(tr("text.context.output"))
}

fn read_message(input: &mut impl Read, max_bytes: usize) -> Result<String> {
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    ensure!((1..=max_bytes).contains(&len), tr("text.message.size"));
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
    prompt_tokens(&model, "Proofread the transcript", "test".into())?;
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let operation = args.next().context(tr("text.mode.missing"))?;
    let path = args.next().context(tr("text.path.missing"))?;
    if operation == "--validate-model-v1" {
        return validate_model(Path::new(&path));
    }
    ensure!(
        operation == "--edit-v2" || operation == "--edit-lines-v1",
        tr("text.protocol")
    );
    let mut stdin = io::stdin().lock();
    let instruction = read_message(&mut stdin, 8192)?;
    let load = || -> Result<(LlamaBackend, LlamaModel)> {
        let backend = LlamaBackend::init()?;
        let model = LlamaModel::load_from_file(
            &backend,
            Path::new(&path),
            &LlamaModelParams::default().with_n_gpu_layers(u32::MAX),
        )?;
        Ok((backend, model))
    };
    if operation == "--edit-lines-v1" {
        // The parent supplies an anonymous, private file as stdin. Load the GGUF
        // once and start each turn with a fresh context and its original speaker.
        let mut size = [0; 4];
        stdin.read_exact(&mut size)?;
        let count = u32::from_be_bytes(size);
        let (backend, model) = load()?;
        let mut out = io::BufWriter::new(io::stdout().lock());
        for index in 0..count {
            let input = read_message(&mut stdin, 120_000)?;
            let text = edit(&backend, &model, &instruction, input)?;
            ensure!(!text.trim().is_empty(), tr("text.output.empty"));
            serde_json::to_writer(
                &mut out,
                &serde_json::json!({"edited":{"index":index,"text":text.trim()}}),
            )?;
            writeln!(out)?;
            out.flush()?;
        }
        return Ok(());
    }
    let transcript = read_message(&mut stdin, 120_000)?;
    drop(stdin);
    watch_parent();
    let (backend, model) = load()?;
    let text = edit(&backend, &model, &instruction, transcript)?;
    ensure!(!text.trim().is_empty(), tr("text.output.empty"));
    io::stdout().lock().write_all(text.trim().as_bytes())?;
    Ok(())
}
