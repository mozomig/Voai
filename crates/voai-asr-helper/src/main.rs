//! Separate process: transcribe.cpp and whisper.cpp each link their own ggml.
use std::{path::Path, process::ExitCode};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, Session, TimestampKind};
use voai_asr_worker::{
    MIN_SAMPLES, RATE,
    file::{self, Word},
};
use voai_l10n::tr;

fn open(path: &Path, language: Option<&str>, backend: Backend) -> Result<Session, String> {
    let model = Model::load_with(
        path,
        &ModelOptions {
            backend,
            ..Default::default()
        },
    )
    .map_err(|_| tr("file.model.open"))?;
    let caps = model.capabilities().map_err(|_| tr("file.model.open"))?;
    if !matches!(model.arch().as_str(), "gigaam" | "parakeet")
        || !matches!(
            caps.max_timestamp_kind,
            TimestampKind::Word | TimestampKind::Token
        )
        || caps.native_sample_rate != RATE as i32
    {
        return Err(tr("speech.gguf.unsupported").into());
    }
    if language.is_some_and(|lang| {
        lang != "auto"
            && !caps.languages.is_empty()
            && !caps.languages.iter().any(|supported| supported == lang)
    }) {
        return Err(tr("speech.gguf.language").into());
    }
    model.session().map_err(|_| tr("file.model.open").into())
}

fn decode(
    session: &mut Session,
    samples: &[f32],
    timestamps: TimestampKind,
) -> Result<transcribe_cpp::Transcript, String> {
    if samples.len() < MIN_SAMPLES || !samples.iter().any(|s| s.abs() > 0.0003) {
        return Ok(Default::default());
    }
    session
        .run(
            samples,
            &RunOptions {
                timestamps,
                ..Default::default()
            },
        )
        .map_err(|_| tr("speech.gguf.decode").into())
}

fn recognize(session: &mut Session, samples: &[f32], offset: u64) -> Result<Vec<Word>, String> {
    let result = decode(session, samples, TimestampKind::Auto)?;
    let base = file::millis(offset);
    if result.words.is_empty() && !result.text.trim().is_empty() {
        let words = sentencepiece_words(&result.tokens, base);
        if words.is_empty() {
            return Err(tr("speech.gguf.timestamps").into());
        }
        return Ok(words);
    }
    Ok(result
        .words
        .into_iter()
        .filter(|word| !word.text.trim().is_empty())
        .map(|word| Word {
            text: word.text.trim().to_owned(),
            start_ms: base + word.t0_ms.max(0) as u64,
            end_ms: base + word.t1_ms.max(0) as u64,
        })
        .collect())
}

// GigaAM publishes timed SentencePiece tokens. A standalone space marker
// starts the next word, and punctuation shares the preceding word's interval.
fn sentencepiece_words(tokens: &[transcribe_cpp::Token], base: u64) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut new_word = true;
    for token in tokens {
        for ch in token.text.chars() {
            if ch == '▁' || ch.is_whitespace() {
                new_word = true;
                continue;
            }
            if new_word {
                words.push(Word {
                    text: String::new(),
                    start_ms: base + token.t0_ms.max(0) as u64,
                    end_ms: base + token.t1_ms.max(0) as u64,
                });
                new_word = false;
            }
            if let Some(word) = words.last_mut() {
                word.text.push(ch);
                word.end_ms = word.end_ms.max(base + token.t1_ms.max(0) as u64);
            }
        }
    }
    words
}

fn dictate(session: &mut Session, samples: &[f32]) -> Result<String, String> {
    // Drafts and short utterances keep native detokenization and omit alignment work.
    if samples.len() <= 24 * RATE {
        return Ok(decode(session, samples, TimestampKind::None)?
            .text
            .trim()
            .into());
    }
    let mut text = String::new();
    file::windows(
        samples.len() as u64,
        |left, right| recognize(session, &samples[left as usize..right as usize], left),
        |words, _| {
            for word in words {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&word.text);
            }
            Ok(())
        },
    )?;
    Ok(text)
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(|s| s.to_str()) {
        Some("--file-worker") => file::run(|model, language| {
            let mut session = open(model, language.as_deref(), Backend::Auto)?;
            Ok(move |samples: &[i16], offset| {
                let floats: Vec<_> = samples.iter().map(|s| *s as f32 / 32768.).collect();
                recognize(&mut session, &floats, offset)
            })
        }),
        Some("--speech-worker") => {
            let model = args.next().ok_or(tr("speech.path.missing"))?;
            if model == "--validate-v1" {
                let path = args.next().ok_or(tr("speech.path.missing"))?;
                open(Path::new(&path), None, Backend::Cpu)?;
                return Ok(());
            }
            let language = args.next().and_then(|s| s.into_string().ok());
            voai_asr_worker::serve(|| {
                let mut session = open(Path::new(&model), language.as_deref(), Backend::Auto)?;
                Ok(move |samples: &[f32], _finished| dictate(&mut session, samples))
            })
            .map_err(|e| e.to_string())
        }
        _ => Err(tr("file.args").into()),
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timed_subwords_preserve_word_boundaries_and_punctuation() {
        let pieces = ["▁При", "вет", ",", "▁", "мир", "!"];
        let tokens: Vec<_> = pieces
            .iter()
            .enumerate()
            .map(|(i, text)| transcribe_cpp::Token {
                text: text.to_string(),
                t0_ms: i as i64 * 40,
                t1_ms: (i as i64 + 1) * 40,
                ..Default::default()
            })
            .collect();
        let words = sentencepiece_words(&tokens, 1000);
        assert_eq!(
            words
                .iter()
                .map(|w| (w.text.as_str(), w.start_ms, w.end_ms))
                .collect::<Vec<_>>(),
            [("Привет,", 1000, 1120), ("мир!", 1160, 1240)]
        );
    }
}
