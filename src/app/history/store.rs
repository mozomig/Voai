use super::{Entry, NewEntry, Outcome};
use rusqlite::{Connection, params};
use std::{
    fs,
    io::Cursor,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
};
use voai_l10n::tr;

pub(super) struct Store(Connection);

impl Store {
    pub fn open(dir: &Path) -> Result<Self, &'static str> {
        let error = tr("app.history.open_failed");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| error)?;
        let path = dir.join("entries.sqlite3");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|_| error)?;
        let db = Connection::open(path).map_err(|_| error)?;
        db.execute_batch(
            "PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT, started INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL, original TEXT NOT NULL,
                edited TEXT, failed INTEGER NOT NULL, wav BLOB NOT NULL
            );",
        )
        .map_err(|_| error)?;
        Ok(Self(db))
    }

    pub fn save(&mut self, entry: NewEntry) -> Result<(), &'static str> {
        let error = tr("app.history.save_failed");
        let duration = entry.audio.len() as i64 * 1000 / voai_speech::SAMPLE_RATE as i64;
        let mut wav = Cursor::new(Vec::with_capacity(entry.audio.len() * 2 + 64));
        let mut writer = hound::WavWriter::new(
            &mut wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: voai_speech::SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(|_| error)?;
        for sample in entry.audio {
            writer
                .write_sample((sample.clamp(-1., 1.) * 32768.).round() as i16)
                .map_err(|_| error)?;
        }
        writer.finalize().map_err(|_| error)?;
        let (edited, failed) = match entry.outcome {
            Outcome::Off => (None, false),
            Outcome::Edited(text) => (Some(text), false),
            Outcome::Failed => (None, true),
        };
        let tx = self.0.transaction().map_err(|_| error)?;
        tx.execute("INSERT INTO entries (started,duration_ms,original,edited,failed,wav) VALUES (?,?,?,?,?,?)",
            params![entry.started, duration, entry.original, edited, failed, wav.into_inner()]).map_err(|_| error)?;
        tx.execute("DELETE FROM entries WHERE id IN (SELECT id FROM entries ORDER BY id DESC LIMIT -1 OFFSET 20)", []).map_err(|_| error)?;
        tx.commit().map_err(|_| error)
    }

    pub fn list(&self) -> Result<Vec<Entry>, &'static str> {
        let error = tr("app.history.read_failed");
        let mut stmt = self.0.prepare("SELECT id,started,duration_ms,original,edited,failed FROM entries ORDER BY id DESC LIMIT 20").map_err(|_| error)?;
        stmt.query_map([], |row| {
            Ok(Entry {
                id: row.get(0)?,
                started: row.get(1)?,
                duration_ms: row.get(2)?,
                original: row.get(3)?,
                outcome: match row.get::<_, Option<String>>(4)? {
                    Some(text) => Outcome::Edited(text),
                    None if row.get::<_, bool>(5)? => Outcome::Failed,
                    None => Outcome::Off,
                },
            })
        })
        .map_err(|_| error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| error)
    }

    pub fn audio(&self, id: i64) -> Result<Vec<u8>, &'static str> {
        self.0
            .query_row("SELECT wav FROM entries WHERE id=?", [id], |row| row.get(0))
            .map_err(|_| tr("app.history.missing"))
    }

    pub fn delete(&self, id: i64) -> Result<(), &'static str> {
        self.0
            .execute("DELETE FROM entries WHERE id=?", [id])
            .map(|_| ())
            .map_err(|_| tr("app.history.delete_failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(started: i64, outcome: Outcome) -> NewEntry {
        NewEntry {
            started,
            original: format!("Диктовка {started}"),
            audio: vec![-1., 0., 1.],
            outcome,
        }
    }

    #[test]
    fn history_survives_restart_with_only_the_latest_twenty_recordings() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        for n in 1..=22 {
            store.save(entry(n, Outcome::Off)).unwrap();
        }
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let rows = store.list().unwrap();
        assert_eq!(
            rows.iter().map(|r| r.started).collect::<Vec<_>>(),
            (3..=22).rev().collect::<Vec<_>>()
        );
        assert!(store.audio(2).is_err());
        assert!(rows[0].outcome == Outcome::Off);
        let wav = store.audio(rows[0].id).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            [-32768, 0, 32767]
        );
    }

    #[test]
    fn preserves_processing_outcome_and_deletes_audio_with_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut processed = entry(1, Outcome::Edited("Итог.".into()));
        processed.audio.resize(24000, 0.);
        store.save(processed).unwrap();
        store.save(entry(2, Outcome::Failed)).unwrap();
        let rows = store.list().unwrap();
        assert!(rows[0].outcome == Outcome::Failed);
        assert_eq!(rows[1].duration_ms, 1500);
        assert_eq!(rows[1].original, "Диктовка 1");
        assert!(rows[1].outcome == Outcome::Edited("Итог.".into()));
        store.delete(rows[1].id).unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert!(store.audio(rows[1].id).is_err());
        assert_eq!(store.list().unwrap().len(), 1);
    }
}
