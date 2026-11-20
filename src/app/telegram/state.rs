use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path, time::SystemTime};
use uuid::Uuid;
use zeroize::Zeroizing;

const SERVICE: &str = "Voai.Telegram";
const STORE_ERROR: &str = "Не удалось сохранить настройки Telegram на Mac.";
const KEY_ERROR: &str = "Не удалось открыть токен в Связке ключей.";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct State {
    account: String,
    pub bot: Option<i64>,
    pub owner: Option<i64>,
    next_update: i64,
    updated: u64,
}

impl State {
    pub fn new() -> Self {
        Self {
            account: Uuid::new_v4().to_string(),
            bot: None,
            owner: None,
            next_update: 0,
            updated: 0,
        }
    }
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        match fs::read(path) {
            Ok(data) => serde_json::from_slice(&data)
                .map(Some)
                .map_err(|_| "Не удалось прочитать настройки Telegram.".into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Не удалось открыть настройки Telegram.".into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or(STORE_ERROR)?)
            .map_err(|_| STORE_ERROR)?;
        serde_json::to_writer(&mut file, self).map_err(|_| STORE_ERROR)?;
        file.as_file().sync_all().map_err(|_| STORE_ERROR)?;
        file.persist(path).map_err(|_| STORE_ERROR)?;
        Ok(())
    }
    pub fn offset(&self, now: u64) -> i64 {
        // Telegram can assign a random update ID after a week of inactivity.
        if now.saturating_sub(self.updated) > 6 * 86400 {
            0
        } else {
            self.next_update
        }
    }
    pub fn advance(&mut self, update: i64, now: u64) {
        self.updated = now;
        self.next_update = update.saturating_add(1);
    }
    pub fn token(&self) -> Result<Zeroizing<String>, String> {
        let key =
            Zeroizing::new(get_generic_password(SERVICE, &self.account).map_err(|_| KEY_ERROR)?);
        Ok(Zeroizing::new(
            std::str::from_utf8(&key).map_err(|_| KEY_ERROR)?.to_owned(),
        ))
    }
    pub fn set_token(&self, token: &str) -> Result<(), String> {
        set_generic_password(SERVICE, &self.account, token.as_bytes())
            .map_err(|_| "Не удалось сохранить токен в Связке ключей.".into())
    }
    pub fn forget(&self) -> Result<(), String> {
        match delete_generic_password(SERVICE, &self.account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(_) => Err("Не удалось удалить токен из Связки ключей.".into()),
        }
    }
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restores_owner_and_expires_stale_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("telegram.json");
        let mut state = State::new();
        state.bot = Some(101);
        state.owner = Some(42);
        state.advance(345, 123);
        state.save(&path).unwrap();
        let saved = State::load(&path).unwrap().unwrap();
        assert_eq!(
            (saved.bot, saved.owner, saved.offset(130)),
            (Some(101), Some(42), 346)
        );
        assert_eq!(saved.offset(123 + 7 * 86400), 0);
    }
}
