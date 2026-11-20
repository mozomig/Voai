pub(crate) mod right_command;

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use std::sync::Arc;
use voai_l10n::{tf, tr};

pub(super) type NotifyKey = Arc<dyn Fn(GlobalHotKeyEvent) + Send + Sync>;

pub(super) struct Shortcuts {
    notify: NotifyKey,
    right_command: Option<right_command::RightCommandMonitor>,
    manager: GlobalHotKeyManager,
    pub dictation: Option<HotKey>,
    cancel_keys: Vec<HotKey>,
    listening: bool,
}

impl Shortcuts {
    pub fn new(notify: NotifyKey) -> Result<Self, String> {
        Ok(Self {
            manager: GlobalHotKeyManager::new().map_err(|e| e.to_string())?,
            dictation: None,
            notify,
            right_command: None,
            cancel_keys: Vec::new(),
            listening: false,
        })
    }

    pub fn set(&mut self, shortcut: &str) -> Result<(), String> {
        let next = shortcut.parse::<HotKey>().map_err(|e| e.to_string())?;
        let bare_command = shortcut == right_command::SHORTCUT;
        if !bare_command
            && !next
                .mods
                .intersects(Modifiers::ALT | Modifiers::CONTROL | Modifiers::META)
        {
            return Err(tr("app.shortcuts.need_modifier").into());
        }
        if next.key == Code::Escape {
            return Err(tr("app.shortcuts.escape_reserved").into());
        }
        if self.dictation == Some(next) {
            return Ok(());
        }
        let monitor = if bare_command {
            let notify = self.notify.clone();
            Some(right_command::RightCommandMonitor::new(move |down| {
                notify(GlobalHotKeyEvent {
                    id: next.id(),
                    state: if down {
                        HotKeyState::Pressed
                    } else {
                        HotKeyState::Released
                    },
                })
            })?)
        } else {
            self.manager.register(next).map_err(|e| {
                tf!(
                    "app.shortcuts.assign_failed",
                    shortcut = shortcut,
                    error = e
                )
            })?;
            None
        };
        if let Err(error) = self.suspend() {
            if monitor.is_none() {
                let _ = self.manager.unregister(next);
            }
            return Err(error);
        }
        self.right_command = monitor;
        self.dictation = Some(next);
        Ok(())
    }

    pub fn suspend(&mut self) -> Result<(), String> {
        if let Some(key) = self.dictation {
            if self.right_command.take().is_none() {
                self.manager.unregister(key).map_err(|e| e.to_string())?;
            }
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
            let held = self.dictation.map(|key| {
                let mods = if self.right_command.is_some() {
                    Modifiers::META
                } else {
                    key.mods
                };
                HotKey::new(Some(mods), Code::Escape)
            });
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
