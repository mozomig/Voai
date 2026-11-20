//! Offline file recognition in an isolated process. PTS zero is the beginning
//! of the original media file. A single Whisper state serves bounded windows.
use crate::{RATE, worker::decode_params};
use serde::Serialize;
use std::{
    fs::File,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};
use whisper_rs::{WhisperContext, WhisperContextParameters, WhisperState};

const STRIDE: u64 = 24 * RATE as u64;
const CONTEXT: u64 = 3 * RATE as u64;

#[derive(Debug, Serialize)]
struct Word {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

impl Word {
    fn middle(&self) -> u64 {
        self.start_ms.midpoint(self.end_ms)
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Update<'a> {
    Progress { progress: u64 },
    Words { words: &'a [Word] },
}

fn report(writer: &mut impl Write, update: Update<'_>) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, &update)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub fn file_worker_main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(2);
    let model = args.next().ok_or("Не указана модель Whisper.")?;
    let language = args
        .next()
        .and_then(|s| s.into_string().ok())
        .ok_or("Не указан язык речи.")?;
    let path = args.next().ok_or("Не указан путь к звуку.")?;
    if args.next().is_some()
        || language.is_empty()
        || language.len() > 12
        || !language
            .bytes()
            .all(|c| c.is_ascii_alphabetic() || c == b'-')
    {
        return Err("Некорректные параметры распознавания.".into());
    }
    let mut input = File::open(path).map_err(|_| "Временная аудиодорожка недоступна.")?;
    let len = input
        .metadata()
        .map_err(|_| "Не удалось прочитать длину записи.")?
        .len();
    if len == 0 || len % 2 != 0 {
        return Err("Аудиодорожка пуста или повреждена.".into());
    }
    whisper_rs::install_logging_hooks();
    let ctx =
        WhisperContext::new_with_params(Path::new(&model), WhisperContextParameters::default())
            .map_err(|_| "Не удалось открыть речевую модель.")?;
    let mut state = ctx
        .create_state()
        .map_err(|_| "Не удалось запустить Whisper.")?;
    let mut writer = BufWriter::new(io::stdout().lock());
    let total = len / 2;
    let mut pending = Vec::new();
    let mut last_end = 0;
    for center in (0..total).step_by(STRIDE as usize) {
        let left = center.saturating_sub(CONTEXT);
        let right = (center + STRIDE + CONTEXT).min(total);
        input
            .seek(SeekFrom::Start(left * 2))
            .map_err(|_| "Не удалось перейти к звуку.")?;
        let mut samples = vec![0i16; (right - left) as usize];
        input
            .read_exact(bytemuck::cast_slice_mut(&mut samples))
            .map_err(|_| "Не удалось дочитать звук.")?;
        let words = recognize(
            &mut state,
            &samples,
            left,
            (language != "auto").then_some(language.as_str()),
        )?;
        let mut words = stitch(pending, words, millis(center));
        let finished = right == total;
        let keep = if finished {
            words.len()
        } else {
            let next_left = millis(center + STRIDE - CONTEXT);
            words.partition_point(|w| w.middle() < next_left)
        };
        pending = words.split_off(keep);
        // Timestamp estimates on opposite sides of a seam can overlap slightly.
        for word in &mut words {
            word.start_ms = word
                .start_ms
                .max(last_end)
                .min(millis(total).saturating_sub(1));
            word.end_ms = word.end_ms.max(word.start_ms + 1).min(millis(total));
            last_end = word.end_ms;
        }
        if !words.is_empty() {
            report(&mut writer, Update::Words { words: &words })
                .map_err(|_| "Расшифровка прервана.")?;
        }
        let progress = if finished {
            100
        } else {
            (center + STRIDE) * 100 / total
        };
        report(&mut writer, Update::Progress { progress }).map_err(|_| "Расшифровка прервана.")?;
        if finished {
            break;
        }
    }
    Ok(())
}

fn millis(sample: u64) -> u64 {
    sample / (RATE as u64 / 1000)
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
        .map_err(|_| "Whisper не смог распознать фрагмент записи.")?;
    let base = millis(offset);
    let limit = millis(offset + samples.len() as u64);
    for piece in state.as_iter().filter(|s| s.no_speech_probability() < 0.8) {
        let text = piece
            .to_str_lossy()
            .map_err(|_| "Whisper вернул некорректный текст.")?;
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

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Find a common word in the overlap, preferring nearby multiword matches.
/// One side supplies every word up to the anchor, the other supplies the rest.
/// If the two decodes disagree completely, use the shared time boundary.
fn stitch(mut previous: Vec<Word>, mut current: Vec<Word>, boundary: u64) -> Vec<Word> {
    if previous.is_empty() {
        return current;
    }
    if current.is_empty() {
        return previous;
    }
    let overlap_end = boundary + millis(CONTEXT);
    let head = current.partition_point(|w| w.middle() <= overlap_end);
    let left: Vec<_> = previous.iter().map(|w| normalized(&w.text)).collect();
    let right: Vec<_> = current[..head]
        .iter()
        .map(|w| normalized(&w.text))
        .collect();
    let mut best = None;
    for (i, word) in left.iter().enumerate() {
        for (j, other) in right.iter().enumerate() {
            let drift = previous[i].middle().abs_diff(current[j].middle());
            if word.is_empty() || word != other || drift > 1_500 {
                continue;
            }
            let score = left[..i]
                .iter()
                .rev()
                .zip(right[..j].iter().rev())
                .take(3)
                .take_while(|(a, b)| a == b)
                .count()
                + left[i + 1..]
                    .iter()
                    .zip(&right[j + 1..])
                    .take(3)
                    .take_while(|(a, b)| a == b)
                    .count();
            let proximity = previous[i].middle().abs_diff(boundary) + drift;
            let rank = (score, std::cmp::Reverse(proximity));
            if best.as_ref().is_none_or(|(old, _, _)| rank > *old) {
                best = Some((rank, i + 1, j + 1));
            }
        }
    }
    let (end, start) = best.map(|(_, i, j)| (i, j)).unwrap_or_else(|| {
        (
            previous.partition_point(|w| w.middle() < boundary),
            current.partition_point(|w| w.middle() < boundary),
        )
    });
    previous.truncate(end);
    previous.extend(current.drain(start..));
    previous
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(input: &[(u64, &str)]) -> Vec<Word> {
        input
            .iter()
            .map(|&(at, text)| Word {
                start_ms: at,
                end_ms: at + 80,
                text: text.into(),
            })
            .collect()
    }
    #[test]
    fn overlap_joins_once_with_jitter_punctuation_and_a_real_repetition() {
        let a = words(&[
            (54000, "Да,"),
            (54600, "да,"),
            (55200, "давайте"),
            (56100, "встретимся"),
        ]);
        let b = words(&[
            (54200, "да,"),
            (54800, "да,"),
            (55900, "давайте"),
            (56500, "встретимся."),
            (57100, "Завтра."),
        ]);
        let merged = stitch(a, b, 56000);
        assert_eq!(
            merged
                .iter()
                .map(|w| normalized(&w.text))
                .collect::<Vec<_>>(),
            ["да", "да", "давайте", "встретимся", "завтра"]
        );
    }
    #[test]
    fn divergent_overlap_has_one_owner_per_side_of_cut() {
        let result = stitch(
            words(&[(100, "слева"), (1100, "черновик")]),
            words(&[(100, "лишнее"), (1200, "справа")]),
            1000,
        );
        assert_eq!(
            result.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["слева", "справа"]
        );
    }
}
