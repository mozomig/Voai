mod engine;
mod speakers;

use engine::{Diarizer, Embedder};
use serde::Serialize;
use speakers::Registry;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::ExitCode;

const SAMPLE_RATE: i32 = 16_000;
const WINDOW: u64 = 75 * SAMPLE_RATE as u64;
const CONTEXT: u64 = 5 * SAMPLE_RATE as u64;

#[derive(Clone, Serialize)]
struct Turn {
    start_ms: u64,
    end_ms: u64,
    speaker: u32,
}

#[derive(Serialize)]
struct Response<'a> {
    turns: &'a [Turn],
}

fn stream(
    engine: &Diarizer,
    voices: &Embedder,
    mut file: File,
    mut out: impl Write,
) -> Result<(), ()> {
    let meta = file.metadata().map_err(|_| ())?;
    if !meta.is_file() || meta.len() == 0 || !meta.len().is_multiple_of(2) {
        return Err(());
    }
    let duration = meta.len() / 2;
    let mut registry = Registry::default();
    let mut previous_tail = Vec::new();
    let mut bytes = Vec::new();
    let mut samples = Vec::new();
    let mut start = 0;
    writeln!(out, "{{\"progress\":0}}").map_err(|_| ())?;
    while start < duration {
        let end = (start + WINDOW).min(duration);
        let left = start.saturating_sub(CONTEXT);
        let right = (end + CONTEXT).min(duration);
        file.seek(SeekFrom::Start(left * 2)).map_err(|_| ())?;
        bytes.resize(((right - left) * 2) as usize, 0);
        file.read_exact(&mut bytes).map_err(|_| ())?;
        let mut turns = Vec::new();
        if bytes.iter().any(|b| *b != 0) {
            samples.clear();
            samples.extend(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&pair| f32::from(i16::from_le_bytes(pair)) / 32768.0),
            );
            turns = engine.turns(&samples)?;
            if duration > WINDOW {
                let local = speakers::describe(&turns, &samples, voices);
                for t in &mut turns {
                    t.start_ms += left / 16;
                    t.end_ms += left / 16;
                }
                registry.assign(&mut turns, local, &previous_tail);
            }
        }
        let end_ms = end.div_ceil(16);
        previous_tail.clear();
        previous_tail.extend(
            turns
                .iter()
                .filter(|t| t.end_ms > end_ms.saturating_sub(CONTEXT / 16))
                .cloned(),
        );
        for turn in &mut turns {
            turn.start_ms = turn.start_ms.max(start / 16);
            turn.end_ms = turn.end_ms.min(end_ms);
        }
        turns.retain(|turn| turn.start_ms < turn.end_ms);
        serde_json::to_writer(&mut out, &Response { turns: &turns }).map_err(|_| ())?;
        writeln!(out).map_err(|_| ())?;
        let percent = u128::from(end) * 100 / u128::from(duration);
        writeln!(out, "{{\"progress\":{percent}}}").map_err(|_| ())?;
        out.flush().map_err(|_| ())?;
        start = end;
    }
    Ok(())
}

fn run() -> Result<(), ()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [input, dir] = args.as_slice() else {
        return Err(());
    };
    let dir = Path::new(dir);
    let engine = Diarizer::load(dir)?;
    let voices = Embedder::load(dir)?;
    if input == "--validate" {
        // Exercise both graphs on generated audio before selecting a local kit.
        let probe: Vec<_> = (0..SAMPLE_RATE)
            .map(|i| (i as f32 * 0.08).sin() * 0.1)
            .collect();
        engine.turns(&probe)?;
        return voices
            .extract(&probe)
            .filter(|v| v.iter().all(|x| x.is_finite()))
            .map(|_| ())
            .ok_or(());
    }
    let file = File::open(input).map_err(|_| ())?;
    stream(&engine, &voices, file, std::io::stdout().lock())
}

fn main() -> ExitCode {
    if run().is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
