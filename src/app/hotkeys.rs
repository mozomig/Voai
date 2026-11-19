use global_hotkey::{
    GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};

pub(super) struct Shortcuts {
    manager: GlobalHotKeyManager,
    pub dictation: Option<HotKey>,
    cancel_keys: Vec<HotKey>,
    listening: bool,
}

impl Shortcuts {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            manager: GlobalHotKeyManager::new().map_err(|e| e.to_string())?,
            dictation: None,
            cancel_keys: Vec::new(),
            listening: false,
        })
    }

    pub fn set(&mut self, shortcut: &str) -> Result<(), String> {
        let next = shortcut.parse::<HotKey>().map_err(|e| e.to_string())?;
        if !next
            .mods
            .intersects(Modifiers::ALT | Modifiers::CONTROL | Modifiers::META)
        {
            return Err("Добавьте ⌥, ⌘ или ⌃ к сочетанию.".into());
        }
        if next.key == Code::Escape {
            return Err("Esc используется для отмены диктовки.".into());
        }
        if self.dictation == Some(next) {
            return Ok(());
        }
        self.manager
            .register(next)
            .map_err(|e| format!("Не удалось назначить {shortcut}: {e}"))?;
        if let Some(previous) = self.dictation
            && let Err(e) = self.manager.unregister(previous)
        {
            let _ = self.manager.unregister(next);
            return Err(e.to_string());
        }
        self.dictation = Some(next);
        Ok(())
    }

    pub fn suspend(&mut self) -> Result<(), String> {
        if let Some(key) = self.dictation {
            self.manager.unregister(key).map_err(|e| e.to_string())?;
            self.dictation = None;
        }
        Ok(())
    }

    pub fn listen_for_cancel(&mut self, enabled: bool) {
        if enabled == self.listening {
            return;
        }
        self.listening = enabled;
        if enabled {
            let escape = HotKey::new(None, Code::Escape);
            let held = self
                .dictation
                .map(|key| HotKey::new(Some(key.mods), Code::Escape));
            for key in std::iter::once(escape).chain(held) {
                if self.manager.register(key).is_ok() {
                    self.cancel_keys.push(key);
                }
            }
        } else {
            for key in self.cancel_keys.drain(..) {
                let _ = self.manager.unregister(key);
            }
        }
    }

    pub fn is_cancel(&self, id: u32) -> bool {
        self.cancel_keys.iter().any(|key| key.id() == id)
    }
}
