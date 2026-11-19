mod appearance;
mod panel;
mod settings;
mod style;
mod tray;

use gpui::{prelude::*, *};
use gpui_component::Root;
use std::{cell::RefCell, rc::Rc, time::Duration};

use crate::app::{Backend, Command, ModelState, Phase, Snapshot};
use panel::Panel;
use settings::Settings;
use tray::{MenuCommand, StatusItem};

type Engine = Rc<RefCell<Backend>>;

actions!(voai, [OpenSettings, Quit, CloseSettings]);

struct Desktop {
    engine: Engine,
    settings: Option<WindowHandle<Root>>,
    panel: Option<WindowHandle<Panel>>,
    tray: Option<StatusItem>,
    last: Option<Snapshot>,
}

impl Desktop {
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
                title: Some("Настройки Voai".into()),
                ..gpui_component::TitleBar::title_bar_options()
            }),
            ..Default::default()
        };
        match cx.open_window(options, move |window, cx| {
            let close_engine = engine.clone();
            window.on_window_should_close(cx, move |_, _| {
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
            Err(error) => eprintln!("Не удалось открыть окно настроек: {error}"),
        }
    }

    fn refresh_delay(&self) -> Duration {
        let busy = self.last.as_ref().is_some_and(|s| {
            matches!(
                s.phase,
                Phase::Listening | Phase::Finishing | Phase::Editing | Phase::Inserting
            ) || s
                .models
                .iter()
                .any(|m| matches!(m.state, ModelState::Downloading { .. }))
        });
        Duration::from_millis(if busy { 33 } else { 500 })
    }

    fn poll(&mut self, cx: &mut App) {
        if let Some(tray) = self.tray.as_ref() {
            let commands = tray.commands().collect::<Vec<_>>();
            for command in commands {
                match command {
                    MenuCommand::Settings => self.show_settings(cx),
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
        let changed = self.last.as_ref() != Some(&snapshot);
        if changed {
            if let Some(tray) = self.tray.as_mut() {
                tray.update(&snapshot);
            }
            if let Some(handle) = self.settings {
                if let Ok(root) = handle.read(cx) {
                    if let Ok(settings) = root.view().clone().downcast::<Settings>() {
                        let _ = cx.update_window(*handle, |_, window, cx| {
                            settings.update(cx, |view, cx| view.sync(snapshot.clone(), window, cx));
                        });
                    }
                } else {
                    self.settings = None;
                }
            }
        }
        if snapshot.phase == Phase::Idle {
            if let Some(panel) = self.panel.take() {
                let _ = panel.update(cx, |_, window, _| window.remove_window());
            }
        } else if self.panel.is_none() {
            if let Some(display) = appearance::pointer_display(cx).or_else(|| cx.primary_display())
            {
                let bounds = display.bounds();
                let dimensions = size(px(430.).min(bounds.size.width), px(178.));
                let origin = point(
                    (bounds.size.width - dimensions.width) / 2.,
                    (bounds.size.height - dimensions.height - px(85.)).max(px(0.)),
                );
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, dimensions))),
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
                        eprintln!("Не удалось открыть панель: {error}");
                        self.engine.borrow_mut().dispatch(Command::Cancel);
                    }
                }
            }
        } else if (changed || snapshot.phase == Phase::Listening)
            && let Some(panel) = self.panel
            && panel
                .update(cx, |panel, _, cx| panel.sync(snapshot.clone(), cx))
                .is_err()
        {
            self.panel = None;
        }
        self.last = Some(snapshot);
    }
}

pub fn run(backend: Backend) {
    let desktop = Rc::new(RefCell::new(Desktop {
        engine: Rc::new(RefCell::new(backend)),
        settings: None,
        panel: None,
        tray: None,
        last: None,
    }));
    let app = Application::new().with_assets(style::Artwork);
    let reopen = desktop.clone();
    app.on_reopen(move |cx| reopen.borrow_mut().show_settings(cx));
    app.run(move |cx| {
        appearance::use_dark_aqua();
        gpui_component::init(cx);
        style::Palette::dark().apply(cx);
        gpui_component::set_locale("ru");
        let settings = desktop.clone();
        cx.on_action(move |_: &OpenSettings, cx| settings.borrow_mut().show_settings(cx));
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
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        });
        cx.bind_keys([
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", CloseSettings, None),
        ]);
        cx.set_menus(vec![Menu {
            name: "Voai".into(),
            items: vec![
                MenuItem::action("Настройки…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Выйти из Voai", Quit),
            ],
        }]);
        match StatusItem::new() {
            Ok(tray) => desktop.borrow_mut().tray = Some(tray),
            Err(error) => eprintln!("Не удалось создать меню Voai: {error}"),
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
