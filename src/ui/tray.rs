use super::{panel::phase_title, style::shortcut_label};
use crate::app::{ModelState, Phase, Snapshot};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use voai_l10n::{tf, tr};

pub(super) enum MenuCommand {
    Settings,
    History,
    CheckUpdates,
    Quit,
}

pub(super) struct StatusItem {
    _icon: TrayIcon,
    status: MenuItem,
    shortcut: MenuItem,
    settings: MenuItem,
    history: MenuItem,
    check: MenuItem,
    quit: MenuItem,
    last_labels: [String; 2],
}

impl StatusItem {
    pub(super) fn new() -> gpui::Result<Self> {
        let status = MenuItem::new("Voai", false, None);
        let shortcut = MenuItem::new("", false, None);
        let settings = MenuItem::new(tr("ui.menu.settings"), true, None);
        let history = MenuItem::new(tr("ui.menu.history"), true, None);
        let check = MenuItem::new(tr("ui.menu.update"), true, None);
        let quit = MenuItem::new(tr("ui.menu.quit"), true, None);
        let menu = Menu::with_items(&[
            &status,
            &shortcut,
            &PredefinedMenuItem::separator(),
            &history,
            &settings,
        ])?;
        if super::updates::available() {
            menu.append(&check)?;
        }
        menu.append(&quit)?;
        // Draw at Retina resolution and let the status bar adapt its template tint.
        let mut rgba = vec![0; 36 * 36 * 4];
        for (x, height) in [(3, 9), (10, 18), (17, 28), (24, 16), (31, 7)] {
            for y in (36 - height) / 2..(36 - height) / 2 + height {
                for dx in 0..4 {
                    let cap = y == (36 - height) / 2 || y == (36 - height) / 2 + height - 1;
                    rgba[4 * (36 * y + x + dx) + 3] =
                        if cap && (dx == 0 || dx == 3) { 70 } else { 255 };
                }
            }
        }
        let icon = Icon::from_rgba(rgba, 36, 36)?;
        let item = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(icon)
            .with_tooltip("Voai")
            .build()?;
        Ok(Self {
            _icon: item,
            status,
            shortcut,
            settings,
            history,
            quit,
            check,
            last_labels: Default::default(),
        })
    }

    pub(super) fn refresh_language(&self) {
        self.settings.set_text(tr("ui.menu.settings"));
        self.history.set_text(tr("ui.menu.history"));
        self.check.set_text(tr("ui.menu.update"));
        self.quit.set_text(tr("ui.menu.quit"));
    }

    pub(super) fn update(&mut self, s: &Snapshot) {
        let title = if s.phase == Phase::Idle {
            if !s.access_allowed || !s.mic_allowed {
                tr("ui.menu.permissions")
            } else if !s
                .models
                .iter()
                .any(|m| !m.text_model && matches!(m.state, ModelState::Selected))
            {
                tr("ui.menu.choose_speech")
            } else {
                phase_title(s.phase)
            }
        } else {
            phase_title(s.phase)
        };
        let labels = [
            title.to_owned(),
            tf!("ui.menu.shortcut", shortcut = shortcut_label(&s.shortcut)),
        ];
        if self.last_labels != labels {
            self.status.set_text(&labels[0]);
            self.shortcut.set_text(&labels[1]);
            self.last_labels = labels;
        }
    }

    pub(super) fn commands(&self) -> impl Iterator<Item = MenuCommand> + '_ {
        MenuEvent::receiver().try_iter().filter_map(|e| {
            if e.id == self.settings.id() {
                Some(MenuCommand::Settings)
            } else if e.id == self.history.id() {
                Some(MenuCommand::History)
            } else if e.id == self.check.id() {
                Some(MenuCommand::CheckUpdates)
            } else if e.id == self.quit.id() {
                Some(MenuCommand::Quit)
            } else {
                None
            }
        })
    }
}
