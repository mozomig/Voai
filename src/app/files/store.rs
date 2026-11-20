use super::{ExportFormat, FileDocument, FileSummary};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};
use tempfile::NamedTempFile;

pub(super) struct Store(Connection);

impl Store {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let error = "Не удалось открыть библиотеку расшифровок.";
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| error)?;
        let path = dir.join("library.sqlite3");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|_| error)?;
        let db = Connection::open(path).map_err(|_| error)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY, title TEXT NOT NULL, content BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pipeline_options (
                id INTEGER PRIMARY KEY CHECK(id=1), value BLOB NOT NULL
            );",
        )
        .map_err(|_| error)?;
        Ok(Self(db))
    }

    pub fn list(&self) -> Result<Vec<FileSummary>, String> {
        let error = "Не удалось загрузить список расшифровок.";
        self.0
            .prepare("SELECT id,title FROM documents ORDER BY id DESC")
            .map_err(|_| error)?
            .query_map([], |r| {
                Ok(FileSummary {
                    id: r.get::<_, i64>(0)?.to_string(),
                    title: r.get(1)?,
                })
            })
            .map_err(|_| error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| error.into())
    }

    pub fn save(&self, doc: &mut FileDocument) -> Result<(), String> {
        let error = "Не удалось сохранить расшифровку.";
        let data = serde_json::to_vec(doc).map_err(|_| error)?;
        if doc.id.is_empty() {
            self.0
                .execute(
                    "INSERT INTO documents(title,content) VALUES(?,?)",
                    params![doc.title, data],
                )
                .map_err(|_| error)?;
            doc.id = self.0.last_insert_rowid().to_string();
        } else if self
            .0
            .execute(
                "UPDATE documents SET content=? WHERE id=?",
                params![data, doc.id],
            )
            .map_err(|_| error)?
            != 1
        {
            return Err(error.into());
        }
        Ok(())
    }

    pub fn load(&self, id: String) -> Result<FileDocument, String> {
        let bytes: Vec<u8> = self
            .0
            .query_row("SELECT content FROM documents WHERE id=?", [&id], |r| {
                r.get(0)
            })
            .map_err(|_| "Расшифровка недоступна.")?;
        let mut doc: FileDocument =
            serde_json::from_slice(&bytes).map_err(|_| "Не удалось прочитать расшифровку.")?;
        // Version 1.3.0 stored the ordinary text-only mode as a warning.
        if let Some(rest) = doc
            .warning
            .as_deref()
            .and_then(|s| s.strip_prefix("Расшифровано без определения спикеров."))
        {
            let rest = rest.trim();
            doc.warning = (!rest.is_empty()).then(|| rest.to_owned());
        }
        doc.id = id;
        Ok(doc)
    }
}

pub(super) fn export(
    path: &Path,
    format: ExportFormat,
    doc: &FileDocument,
    private: &Path,
) -> Result<(), String> {
    let error = "Не удалось сохранить файл экспорта.";
    let extension = match format {
        ExportFormat::Txt => "txt",
        ExportFormat::Srt => "srt",
    };
    if !path
        .extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| x.eq_ignore_ascii_case(extension))
    {
        return Err(format!("Выберите имя с расширением .{extension}."));
    }
    let parent = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .ok_or(error)?;
    if parent.starts_with(private.canonicalize().map_err(|_| error)?) {
        return Err("Выберите папку вне хранилища Voai.".into());
    }
    if let (Ok(a), Ok(b)) = (doc.source.metadata(), path.metadata())
        && a.dev() == b.dev()
        && a.ino() == b.ino()
    {
        return Err("Выберите файл, отличный от исходной записи.".into());
    }
    let mut temp = NamedTempFile::new_in(parent).map_err(|_| error)?;
    temp.write_all(doc.export_text(format).as_bytes())
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| error)?;
    temp.persist(path).map_err(|_| error)?;
    Ok(())
}

impl Store {
    pub fn pipeline_options(&self) -> Result<super::FileOptions, String> {
        let data: Option<Vec<u8>> = self
            .0
            .query_row("SELECT value FROM pipeline_options WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|_| "Не удалось прочитать настройки расшифровки.")?;
        data.map(|v| serde_json::from_slice(&v))
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(|_| "Не удалось прочитать настройки расшифровки.".into())
    }

    pub fn save_choice(&self, choice: &super::FileOptions) -> Result<(), String> {
        let err = "Не удалось сохранить настройки расшифровки.";
        let data = serde_json::to_vec(choice).map_err(|_| err)?;
        self.0
            .execute(
                "INSERT OR REPLACE INTO pipeline_options VALUES(1,?)",
                [data],
            )
            .map_err(|_| err)?;
        Ok(())
    }
}
