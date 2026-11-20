use super::{panel::phase_title, style::shortcut_label};
use crate::app::{ModelState, Phase, Snapshot};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

pub(super) enum MenuCommand {
    Settings,
    History,
    Quit,
}

pub(super) struct StatusItem {
    _icon: TrayIcon,
    status: MenuItem,
    shortcut: MenuItem,
    settings: MenuItem,
    history: MenuItem,
    quit: MenuItem,
    last_labels: [String; 2],
}

impl StatusItem {
    pub(super) fn new() -> gpui::Result<Self> {
        let status = MenuItem::new("Voai", false, None);
        let shortcut = MenuItem::new("", false, None);
        let settings = MenuItem::new("Настройки…", true, None);
        let history = MenuItem::new("История диктовки", true, None);
        let quit = MenuItem::new("Выйти из Voai", true, None);
        let menu = Menu::with_items(&[
            &status,
            &shortcut,
            &PredefinedMenuItem::separator(),
            &history,
            &settings,
            &quit,
        ])?;
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
            last_labels: Default::default(),
        })
    }

    pub(super) fn update(&mut self, s: &Snapshot) {
        let title = if s.phase == Phase::Idle {
            if !s.access_allowed || !s.mic_allowed {
                "Нужны разрешения"
            } else if !s
                .models
                .iter()
                .any(|m| !m.text_model && matches!(m.state, ModelState::Selected))
            {
                "Выберите речевую модель"
            } else {
                phase_title(s.phase)
            }
        } else {
            phase_title(s.phase)
        };
        let labels = [
            title.to_owned(),
            format!("Диктовка · {}", shortcut_label(&s.shortcut)),
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
            } else if e.id == self.quit.id() {
                Some(MenuCommand::Quit)
            } else {
                None
            }
        })
    }
}
