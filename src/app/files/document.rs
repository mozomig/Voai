use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLine {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    pub speaker: Option<u32>,
    pub review: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDocument {
    #[serde(skip)]
    pub id: String,
    pub title: String,
    pub source: PathBuf,
    pub speakers: BTreeMap<u32, String>,
    pub lines: Vec<FileLine>,
    pub warning: Option<String>,
    pub(super) source_stamp: SourceStamp,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SourceStamp {
    bytes: u64,
    modified_sec: i64,
    modified_ns: i64,
    dev: u64,
    inode: u64,
}

impl SourceStamp {
    pub fn read(path: &Path) -> Result<Self, String> {
        let m = path.metadata().map_err(|_| "Исходный файл недоступен.")?;
        if !m.is_file() {
            return Err("Выберите аудио- или видеофайл.".into());
        }
        Ok(Self {
            bytes: m.len(),
            modified_sec: m.mtime(),
            modified_ns: m.mtime_nsec(),
            dev: m.dev(),
            inode: m.ino(),
        })
    }
}

#[derive(Deserialize)]
pub(super) struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}
#[derive(Deserialize)]
pub(super) struct Turn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: u32,
}

pub(super) fn align(mut words: Vec<Word>, mut turns: Vec<Turn>) -> Vec<FileLine> {
    words.sort_by_key(|w| w.start_ms);
    turns.sort_by_key(|s| s.start_ms);
    let mut pending = turns.iter().peekable();
    let mut active: Vec<&Turn> = Vec::new();
    let mut lines: Vec<FileLine> = Vec::new();
    for w in words {
        let text = w.text.trim();
        if text.is_empty() || w.end_ms <= w.start_ms {
            continue;
        }
        active.retain(|t| t.end_ms > w.start_ms);
        while let Some(t) = pending.next_if(|t| t.start_ms < w.end_ms) {
            if t.end_ms > w.start_ms {
                active.push(t);
            }
        }
        let mut scores = BTreeMap::<u32, u64>::new();
        let mut overlap = false;
        for (i, t) in active.iter().enumerate() {
            let from = t.start_ms.max(w.start_ms);
            let to = t.end_ms.min(w.end_ms);
            if to <= from {
                continue;
            }
            *scores.entry(t.speaker).or_default() += to - from;
            overlap |= active[..i]
                .iter()
                .any(|o| o.speaker != t.speaker && o.start_ms < to && o.end_ms > from);
        }
        let speaker = scores.iter().max_by_key(|(_, n)| **n).map(|(s, _)| *s);
        let review = overlap
            || (scores.len() > 1
                && scores.values().max().copied().unwrap_or(0) * 5 < (w.end_ms - w.start_ms) * 3);
        if let Some(line) = lines.last_mut()
            && line.speaker == speaker
            && line.review == review
            && w.start_ms.saturating_sub(line.end_ms) < 1000
            && w.end_ms.saturating_sub(line.start_ms) <= 20000
        {
            if !text.starts_with(['.', ',', '!', '?', ':', ';', '…', ')', '»']) {
                line.text.push(' ');
            }
            line.text.push_str(text);
            line.end_ms = line.end_ms.max(w.end_ms);
            continue;
        }
        lines.push(FileLine {
            start_ms: w.start_ms,
            end_ms: w.end_ms,
            text: text.to_owned(),
            original: None,
            speaker,
            review,
        });
    }
    lines
}

#[derive(Clone, Copy)]
pub enum ExportFormat {
    Txt,
    Srt,
}

fn timestamp(ms: u64, millis: bool) -> String {
    let mut t = format!(
        "{:02}:{:02}:{:02}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60
    );
    if millis {
        write!(t, ",{:03}", ms % 1000).unwrap();
    }
    t
}

impl FileDocument {
    pub(super) fn export_text(&self, format: ExportFormat) -> String {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            let name = line.speaker.and_then(|s| self.speakers.get(&s));
            match format {
                ExportFormat::Txt => write!(out, "[{}] ", timestamp(line.start_ms, false)).unwrap(),
                ExportFormat::Srt => writeln!(
                    out,
                    "{}\n{} --> {}",
                    i + 1,
                    timestamp(line.start_ms, true),
                    timestamp(line.end_ms, true)
                )
                .unwrap(),
            }
            if let Some(name) = name {
                write!(out, "{name}: ").unwrap();
            }
            match format {
                ExportFormat::Txt => writeln!(out, "{}", line.text).unwrap(),
                ExportFormat::Srt => writeln!(
                    out,
                    "{}",
                    line.text.split_whitespace().collect::<Vec<_>>().join(" ")
                )
                .unwrap(),
            }
            if matches!(format, ExportFormat::Srt) {
                out.push('\n');
            }
        }
        out
    }

    pub(super) fn assign(&mut self, line: usize, speaker: Option<u32>) -> Result<(), String> {
        if speaker.is_some_and(|s| !self.speakers.contains_key(&s)) {
            return Err("Спикер недоступен.".into());
        }
        let row = self.lines.get_mut(line).ok_or("Реплика недоступна.")?;
        row.speaker = speaker;
        row.review = false;
        Ok(())
    }

    pub(super) fn set_name(&mut self, id: u32, name: String) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err("Введите имя спикера длиной до 80 знаков.".into());
        }
        self.speakers.insert(id, name.to_owned());
        Ok(())
    }
}
