use crate::{RATE, worker::decode_params};
use voai_asr_worker::file::{Word, millis};
use voai_l10n::tr;
use whisper_rs::{WhisperContext, WhisperContextParameters, WhisperState};

pub fn file_worker_main() -> Result<(), String> {
    voai_asr_worker::file::run(|model, language| {
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
            .map_err(|_| tr("file.model.open"))?;
        let mut state = ctx.create_state().map_err(|_| tr("file.whisper.start"))?;
        Ok(move |samples: &[i16], left| recognize(&mut state, samples, left, language.as_deref()))
    })
}

fn recognize(
    state: &mut WhisperState,
    samples: &[i16],
    offset: u64,
    language: Option<&str>,
) -> Result<Vec<Word>, String> {
    let mut words = Vec::new();
    for range in speech_ranges(samples) {
        let base = offset + range.start as u64;
        recognize_region(state, &samples[range], base, language, &mut words)?;
    }
    Ok(words)
}

// Whisper may stretch word timestamps across a long pause inside a window.
// Split on sustained silence, retaining padding for the first/last phoneme.
fn speech_ranges(samples: &[i16]) -> Vec<std::ops::Range<usize>> {
    const FRAME: usize = RATE / 100;
    const SPLIT_GAP: usize = RATE * 3 / 2;
    let mut ranges = Vec::new();
    let mut start = None;
    let mut last = 0;
    for (index, frame) in samples.chunks(FRAME).enumerate() {
        let position = index * FRAME;
        if position.saturating_sub(last) >= SPLIT_GAP
            && let Some(from) = start.take()
        {
            ranges.push(from..last + RATE / 3);
        }
        let energy: i64 = frame.iter().map(|&v| i64::from(v).pow(2)).sum();
        if energy > 107 * frame.len() as i64 {
            start.get_or_insert(position.saturating_sub(RATE / 3));
            last = position + frame.len();
        }
    }
    if let Some(start) = start {
        ranges.push(start..(last + RATE / 3).min(samples.len()));
    }
    ranges
}

fn recognize_region(
    state: &mut WhisperState,
    samples: &[i16],
    offset: u64,
    language: Option<&str>,
    words: &mut Vec<Word>,
) -> Result<(), String> {
    if samples.len() < crate::MIN_SAMPLES {
        return Ok(());
    }
    let mut pcm: Vec<_> = samples.iter().map(|&v| f32::from(v) / 32768.).collect();
    pcm.resize(pcm.len().max(RATE), 0.);
    let mut params = decode_params(language);
    params.set_token_timestamps(true);
    params.set_split_on_word(true);
    params.set_max_len(1);
    state
        .full(params, &pcm)
        .map_err(|_| tr("file.whisper.segment"))?;
    let base = millis(offset);
    let limit = millis(offset + samples.len() as u64);
    for piece in state.as_iter().filter(|s| s.no_speech_probability() < 0.8) {
        let text = piece.to_str_lossy().map_err(|_| tr("file.whisper.text"))?;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let from = (base + piece.start_timestamp().max(0) as u64 * 10).min(limit.saturating_sub(1));
        let to =
            (base + piece.end_timestamp().max(0) as u64 * 10).clamp(from + 1, limit.max(from + 1));
        words.push(Word {
            start_ms: from,
            end_ms: to,
            text: text.to_owned(),
        });
    }
    Ok(())
}
