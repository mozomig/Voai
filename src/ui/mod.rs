use voai_l10n::tr;
mod appearance;
mod copy_text;
mod panel;
mod settings;
mod style;
mod tray;
mod updates;
mod widget_position;

use gpui::{prelude::*, *};
use gpui_component::Root;
use std::{cell::RefCell, rc::Rc, time::Duration};

use crate::app::{Backend, Command, ModelState, Phase, Snapshot};
use panel::Panel;
use settings::Settings;
use tray::{MenuCommand, StatusItem};

type Engine = Rc<RefCell<Backend>>;

actions!(
    voai,
    [
        CheckUpdates,
        OpenSettings,
        OpenPermissions,
        OpenSpeechModels,
        Quit,
        CloseSettings
    ]
);

struct Desktop {
    engine: Engine,
    settings: Option<WindowHandle<Root>>,
    panel: Option<WindowHandle<Panel>>,
    tray: Option<StatusItem>,
    last: Option<Snapshot>,
    language: &'static str,
}

impl Desktop {
    fn show_setup(&mut self, models: bool, cx: &mut App) {
        self.engine.borrow_mut().dispatch(Command::Cancel);
        self.show_settings(cx);
        if let Some(handle) = self.settings {
            let _ = handle.update(cx, |root, window, cx| {
                if let Ok(settings) = root.view().clone().downcast::<Settings>() {
                    settings.update(cx, |view, cx| view.show_dictation_setup(models, window, cx));
                }
            });
        }
    }

    fn show_settings(&mut self, cx: &mut App) {
        if let Some(handle) = self.settings
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            cx.activate(true);
            return;
        }
        let engine = self.engine.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(846.), px(602.)),
                cx,
            ))),
            window_min_size: Some(size(px(700.), px(448.))),
            titlebar: Some(TitlebarOptions {
                title: Some(tr("ui.settings.title").into()),
                ..gpui_component::TitleBar::title_bar_options()
            }),
            ..Default::default()
        };
        match cx.open_window(options, move |window, cx| {
            let close_engine = engine.clone();
            window.on_window_should_close(cx, move |_, _| {
                close_engine
                    .borrow_mut()
                    .dispatch(Command::File(crate::app::FileCommand::Stop));
                close_engine.borrow_mut().dispatch(Command::StopPlayback);
                close_engine
                    .borrow_mut()
                    .dispatch(Command::TestFocus(false));
                close_engine
                    .borrow_mut()
                    .dispatch(Command::CaptureShortcut(false));
                true
            });
            let settings = cx.new(|cx| Settings::new(engine, window, cx));
            cx.new(|cx| Root::new(settings, window, cx))
        }) {
            Ok(window) => {
                self.settings = Some(window);
                cx.activate(true);
            }
            Err(error) => eprintln!("Could not open settings: {error}"),
        }
    }

    fn refresh_delay(&self) -> Duration {
        let Some(snapshot) = &self.last else {
            return Duration::from_millis(500);
        };
        if matches!(
            snapshot.phase,
            Phase::Listening | Phase::Finishing | Phase::Editing | Phase::Inserting
        ) {
            return Duration::from_millis(33);
        }
        if self.settings.is_some()
            && snapshot
                .models
                .iter()
                .any(|m| matches!(m.state, ModelState::Downloading { .. }))
        {
            return Duration::from_millis(200);
        }
        Duration::from_millis(500)
    }

    fn poll(&mut self, cx: &mut App) {
        if let Some(tray) = self.tray.as_ref() {
            let commands = tray.commands().collect::<Vec<_>>();
            for command in commands {
                match command {
                    MenuCommand::Settings => self.show_settings(cx),
                    MenuCommand::History => {
                        self.show_settings(cx);
                        if let Some(handle) = self.settings {
                            let _ = handle.update(cx, |root, window, cx| {
                                if let Ok(settings) = root.view().clone().downcast::<Settings>() {
                                    settings.update(cx, |view, cx| {
                                        view.navigate(settings::Page::History, window, cx)
                                    });
                                }
                            });
                        }
                    }
                    MenuCommand::CheckUpdates => updates::check(),
                    MenuCommand::Quit => {
                        cx.quit();
                        return;
                    }
                }
            }
        }
        if let Some(handle) = self.settings
            && let Ok(root) = handle.read(cx)
            && let Ok(settings) = root.view().clone().downcast::<Settings>()
        {
            let _ = cx.update_window(*handle, |_, window, cx| {
                settings.update(cx, |view, cx| view.check_test_target(window, cx));
            });
        }
        let snapshot = {
            let mut engine = self.engine.borrow_mut();
            engine.tick();
            engine.snapshot()
        };
        updates::set_busy(
            snapshot.phase != Phase::Idle
                || snapshot.files.progress.is_some()
                || snapshot.telegram.processing
                || snapshot.connecting_model
                || snapshot
                    .models
                    .iter()
                    .any(|m| matches!(m.state, ModelState::Downloading { .. })),
        );
        let language = voai_l10n::language();
        let language_changed = self.language != language;
        if language_changed {
            self.language = language;
            gpui_component::set_locale(language);
            set_app_menu(cx);
            if let Some(tray) = &self.tray {
                tray.refresh_language();
            }
        }
        let changed = language_changed || self.last.as_ref() != Some(&snapshot);
        if changed {
            if let Some(tray) = self.tray.as_mut() {
                tray.update(&snapshot);
            }
            if let Some(handle) = self.settings {
                if let Ok(root) = handle.read(cx) {
                    if let Ok(settings) = root.view().clone().downcast::<Settings>() {
                        let _ = cx.update_window(*handle, |_, window, cx| {
                            settings.update(cx, |view, cx| {
                                if language_changed {
                                    view.refresh_language(window, cx);
                                }
                                view.sync(snapshot.clone(), window, cx);
                            });
                        });
                    }
                } else {
                    self.settings = None;
                }
            }
        }
        let compact = panel::compact(&snapshot);
        // A recovery panel needs a different frame and must accept clicks again.
        if self
            .last
            .as_ref()
            .is_some_and(|last| panel::compact(last) != compact)
            && let Some(previous) = self.panel.take()
        {
            let _ = previous.update(cx, |_, window, _| window.remove_window());
        }
        if snapshot.phase == Phase::Idle {
            if let Some(panel) = self.panel.take() {
                let _ = panel.update(cx, |_, window, _| window.remove_window());
            }
        } else if self.panel.is_none() {
            let placement = if compact {
                let anchor = self.engine.borrow().widget_anchor().or_else(|| {
                    let root = self.settings?.read(cx).ok()?;
                    let view = root.view().clone().downcast::<Settings>().ok()?;
                    view.read(cx).test_widget_anchor()
                });
                widget_position::compact_bounds(anchor, cx)
            } else {
                appearance::pointer_display(cx)
                    .or_else(|| cx.primary_display())
                    .map(|display| {
                        let bounds = display.bounds();
                        let dimensions =
                            size(px(430.).min(bounds.size.width), panel::height(&snapshot));
                        let origin = point(
                            (bounds.size.width - dimensions.width) / 2.,
                            (bounds.size.height - dimensions.height - px(85.)).max(px(0.)),
                        );
                        (display, Bounds::new(origin, dimensions))
                    })
            };
            if let Some((display, frame)) = placement {
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(frame)),
                    display_id: Some(display.id()),
                    kind: WindowKind::PopUp,
                    titlebar: None,
                    focus: false,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                };
                match cx.open_window(options, |window, cx| {
                    cx.new(|_| Panel::new(self.engine.clone(), snapshot.clone(), window))
                }) {
                    Ok(panel) => self.panel = Some(panel),
                    Err(error) => {
                        eprintln!("Could not open the dictation panel: {error}");
                        self.engine.borrow_mut().dispatch(Command::Cancel);
                    }
                }
            }
        } else if (changed || snapshot.phase == Phase::Listening || compact)
            && let Some(panel) = self.panel
            && panel
                .update(cx, |panel, window, cx| {
                    panel.sync(snapshot.clone(), window, cx)
                })
                .is_err()
        {
            self.panel = None;
        }
        self.last = Some(snapshot);
    }
}

fn set_app_menu(cx: &mut App) {
    let mut items = vec![MenuItem::action(tr("ui.menu.settings"), OpenSettings)];
    if updates::available() {
        items.push(MenuItem::action(tr("ui.menu.update"), CheckUpdates));
    }
    items.extend([
        MenuItem::separator(),
        MenuItem::action(tr("ui.menu.quit"), Quit),
    ]);
    cx.set_menus(vec![Menu {
        name: "Voai".into(),
        items,
    }]);
}

pub fn run(backend: Backend) {
    let desktop = Rc::new(RefCell::new(Desktop {
        engine: Rc::new(RefCell::new(backend)),
        settings: None,
        panel: None,
        tray: None,
        last: None,
        language: voai_l10n::language(),
    }));
    let app = Application::new().with_assets(style::Artwork);
    let reopen = desktop.clone();
    app.on_reopen(move |cx| reopen.borrow_mut().show_settings(cx));
    app.run(move |cx| {
        appearance::use_dark_aqua();
        updates::start();
        gpui_component::init(cx);
        style::Palette::dark().apply(cx);
        gpui_component::set_locale(voai_l10n::language());
        let settings = desktop.clone();
        cx.on_action(move |_: &OpenSettings, cx| settings.borrow_mut().show_settings(cx));
        let permissions = desktop.clone();
        cx.on_action(move |_: &OpenPermissions, cx| permissions.borrow_mut().show_setup(false, cx));
        let models = desktop.clone();
        cx.on_action(move |_: &OpenSpeechModels, cx| models.borrow_mut().show_setup(true, cx));
        cx.on_action(|_: &Quit, cx| cx.quit());
        let closable = desktop.clone();
        cx.on_action(move |_: &CloseSettings, cx| {
            let desktop = closable.borrow();
            if let Some(handle) = desktop.settings {
                desktop
                    .engine
                    .borrow_mut()
                    .dispatch(Command::TestFocus(false));
                desktop
                    .engine
                    .borrow_mut()
                    .dispatch(Command::CaptureShortcut(false));
                desktop
                    .engine
                    .borrow_mut()
                    .dispatch(Command::File(crate::app::FileCommand::Stop));
                desktop.engine.borrow_mut().dispatch(Command::StopPlayback);
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        });
        cx.bind_keys([
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", CloseSettings, None),
        ]);
        cx.on_action(|_: &CheckUpdates, _| updates::check());
        set_app_menu(cx);
        match StatusItem::new() {
            Ok(tray) => desktop.borrow_mut().tray = Some(tray),
            Err(error) => eprintln!("Could not create the Voai menu: {error}"),
        }
        desktop.borrow_mut().show_settings(cx);
        let wake = desktop.borrow().engine.borrow().wake_signal();
        cx.spawn(async move |cx| {
            while let Ok(delay) = cx.update(|cx| {
                let mut desktop = desktop.borrow_mut();
                desktop.poll(cx);
                desktop.refresh_delay()
            }) {
                let timer = cx.background_executor().timer(delay);
                let running =
                    futures_lite::future::race(async { wake.recv().await.is_ok() }, async {
                        timer.await;
                        true
                    })
                    .await;
                if !running {
                    break;
                }
            }
        })
        .detach();
    });
}
