use super::*;
use gpui_component::menu::PopupMenu;

impl Settings {
    pub(super) fn file_setup(&self, p: Palette, cx: &Context<Self>) -> Div {
        let files = &self.snapshot.files;
        let speech = self.file_model(ModelTab::Speech, Some(&files.speech_model));
        let text_model = self.file_model(ModelTab::Text, files.text_model.as_deref());
        let local_voices = files
            .voices_local
            .as_deref()
            .filter(|_| files.using_local_voices && files.identify_speakers && files.voices_ready);
        let voices = if !files.identify_speakers {
            "Без спикеров"
        } else if !files.voices_ready {
            "Выбрать модель"
        } else if let Some(path) = local_voices {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Подключённая папка")
        } else {
            "Pyannote 3.0 + CAM++"
        };
        let mut setup = col()
            .gap_1()
            .child(text("Аудио и видео обрабатываются на этом Mac.", 13., p.muted).mb_2());
        for (tab, title, value) in [
            (
                ModelTab::Speech,
                "Распознавание",
                speech.map(|m| m.name.as_str()).unwrap_or("Выбрать модель"),
            ),
            (ModelTab::Speakers, "Спикеры", voices),
            (
                ModelTab::Text,
                "Обработка текста",
                text_model
                    .map(|m| m.name.as_str())
                    .unwrap_or(if files.text_model.is_some() {
                        "Модель недоступна"
                    } else {
                        "Без обработки"
                    }),
            ),
        ] {
            let owner = cx.entity().downgrade();
            let display = value
                .strip_suffix(".bin")
                .or_else(|| value.strip_suffix(".gguf"))
                .unwrap_or(value);
            let picker = Button::new(title)
                .small()
                .rounded(px(6.))
                .bg(p.surface)
                .border_1()
                .border_color(p.rule)
                .tooltip(match (tab, local_voices) {
                    (ModelTab::Speakers, Some(folder)) => folder.display().to_string(),
                    _ => value.to_owned(),
                })
                .child(
                    row()
                        .w(px(184.))
                        .gap_2()
                        .child(
                            text(display.to_owned(), 12., p.ink)
                                .flex_1()
                                .min_w_0()
                                .truncate(),
                        )
                        .child(icon("chevron", p.muted, 12.).flex_shrink_0()),
                )
                .dropdown_menu(move |menu, _, cx| {
                    let Some(view) = owner.upgrade() else {
                        return menu;
                    };
                    view.read(cx).file_setup_menu(menu, tab, &owner)
                });
            setup = setup.child(Self::setting(p, title, "", picker));
        }
        let import_label = if files.identify_speakers && !files.voices_ready {
            "Расшифровать без спикеров…"
        } else {
            "Выбрать файл…"
        };
        let actions = row()
            .gap_2()
            .mt_2()
            .child(
                primary_button("file-import", import_label, p)
                    .disabled(
                        speech.is_none() || (files.text_model.is_some() && text_model.is_none()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.import_media(cx))),
            )
            .when(files.document.is_some(), |r| {
                r.child(
                    Button::new("file-back")
                        .small()
                        .ghost()
                        .label("К расшифровке")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.files_ui.configuring = false;
                            cx.notify();
                        })),
                )
            });
        setup.child(actions)
    }

    fn file_model(&self, tab: ModelTab, id: Option<&str>) -> Option<&ModelRow> {
        self.snapshot.models.iter().find(|model| {
            Some(model.id.as_str()) == id
                && model.text_model == (tab == ModelTab::Text)
                && matches!(model.state, ModelState::Selected | ModelState::Installed)
        })
    }

    fn file_setup_menu(
        &self,
        mut menu: PopupMenu,
        tab: ModelTab,
        owner: &WeakEntity<Self>,
    ) -> PopupMenu {
        let files = &self.snapshot.files;
        menu = menu
            .min_w(px(208.))
            .max_w(px(352.))
            .max_h(px(280.))
            .scrollable(true);
        if tab == ModelTab::Speakers {
            menu = menu.item(self.file_option(
                "Без спикеров",
                !files.identify_speakers,
                || FileCommand::SetIdentifySpeakers(false),
            ));
            if files.voices_downloaded {
                menu = menu.item(self.file_option(
                    "Pyannote 3.0 + CAM++",
                    files.identify_speakers && !files.using_local_voices && files.voices_ready,
                    || FileCommand::UseDownloadedVoices,
                ));
            }
            if let Some(path) = &files.voices_local {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                menu = menu.item(self.file_option(
                    format!("Папка · {name}"),
                    files.identify_speakers && files.using_local_voices && files.voices_ready,
                    || FileCommand::UseLocalVoices,
                ));
            }
        } else {
            let is_text = tab == ModelTab::Text;
            if is_text {
                menu = menu.item(self.file_option(
                    "Без обработки",
                    files.text_model.is_none(),
                    || FileCommand::SetText(None),
                ));
            }
            for model in self.snapshot.models.iter().filter(|m| {
                m.text_model == is_text
                    && matches!(m.state, ModelState::Selected | ModelState::Installed)
            }) {
                let id = model.id.clone();
                let checked = if is_text {
                    files.text_model.as_ref() == Some(&id)
                } else {
                    files.speech_model == id
                };
                menu = menu.item(self.file_option(model.name.clone(), checked, move || {
                    if is_text {
                        FileCommand::SetText(Some(id.clone()))
                    } else {
                        FileCommand::SetSpeech(id.clone())
                    }
                }));
            }
        }
        if !menu.is_empty() {
            menu = menu.separator();
        }
        let catalog = owner.clone();
        menu = menu.item(
            PopupMenuItem::new("Открыть каталог").on_click(move |_, window, cx| {
                let _ = catalog.update(cx, |view, cx| {
                    view.model_tab = tab;
                    view.file_dialog_error = None;
                    view.navigate(Page::Models, window, cx);
                });
            }),
        );
        if tab == ModelTab::Text {
            let owner = owner.clone();
            menu = menu.item(PopupMenuItem::new("Инструкция для обработки").on_click(
                move |_, window, cx| {
                    let _ =
                        owner.update(cx, |view, cx| view.navigate(Page::Processing, window, cx));
                },
            ));
        }
        menu
    }

    fn file_option(
        &self,
        label: impl Into<SharedString>,
        checked: bool,
        command: impl Fn() -> FileCommand + 'static,
    ) -> PopupMenuItem {
        let engine = self.engine.clone();
        PopupMenuItem::new(label)
            .checked(checked)
            .on_click(move |_, _, _| engine.borrow_mut().dispatch(Command::File(command())))
    }
}
