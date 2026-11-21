use super::*;
use crate::app::{EditorSource, FileEditor, ServiceModel};
use gpui_component::menu::PopupMenu;
use voai_l10n::{tf, tr};

impl Settings {
    pub(super) fn file_setup(&self, p: Palette, cx: &Context<Self>) -> Div {
        let files = &self.snapshot.files;
        let speech = self.file_model(ModelTab::Speech, Some(&files.speech_model));
        let mut missing_editor = false;
        let editor_name = match &files.text_model {
            Some(FileEditor::Service(target)) => target.name(),
            Some(FileEditor::Local(id)) => match self.file_model(ModelTab::Text, Some(id)) {
                Some(model) => &model.name,
                None => {
                    missing_editor = true;
                    tr("ui.model.unavailable")
                }
            },
            None => tr("ui.edit.none"),
        };
        let local_voices = files
            .voices_local
            .as_deref()
            .filter(|_| files.using_local_voices && files.identify_speakers && files.voices_ready);
        let voices = if !files.identify_speakers {
            tr("ui.files.without_speakers")
        } else if !files.voices_ready {
            tr("ui.model.choose")
        } else if let Some(path) = local_voices {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(tr("ui.model.linked_folder"))
        } else {
            "Pyannote 3.0 + CAM++"
        };
        let mut setup = col()
            .gap_1()
            .child(text(tr("ui.files.local_hint"), 13., p.muted).mb_2());
        for (tab, title, value) in [
            (
                ModelTab::Speech,
                tr("ui.files.recognition"),
                speech
                    .map(|m| m.name.as_str())
                    .unwrap_or(tr("ui.model.choose")),
            ),
            (ModelTab::Speakers, tr("ui.files.speakers"), voices),
            (ModelTab::Text, tr("ui.files.editing"), editor_name),
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
                    let settings = view.read(cx);
                    let menu = menu
                        .min_w(px(208.))
                        .max_w(px(352.))
                        .max_h(px(320.))
                        .scrollable(true);
                    let menu = if tab == ModelTab::Text {
                        settings.file_editor_menu(menu)
                    } else {
                        settings.file_setup_menu(menu, tab)
                    };
                    Self::file_menu_links(menu, tab, &owner)
                });
            setup = setup.child(Self::setting(p, title, "", picker));
        }
        if let Some(FileEditor::Service(target)) = &files.text_model
            && !matches!(target, ServiceModel::Server { .. })
        {
            setup = setup.child(text(
                tf!("ui.files.cloud_hint", provider = target.source_name()),
                12.,
                p.muted,
            ));
        }
        let import_label = if files.identify_speakers && !files.voices_ready {
            tr("ui.files.transcribe_without")
        } else {
            tr("ui.files.choose_file")
        };
        let actions = row()
            .gap_2()
            .mt_2()
            .child(
                primary_button("file-import", import_label, p)
                    .disabled(speech.is_none() || missing_editor)
                    .on_click(cx.listener(|this, _, _, cx| this.import_media(cx))),
            )
            .when(files.document.is_some(), |r| {
                r.child(
                    Button::new("file-back")
                        .small()
                        .ghost()
                        .label(tr("ui.files.back"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.files_ui.configuring = false;
                            cx.notify();
                        })),
                )
            });
        setup.child(actions)
    }

    fn file_model(&self, tab: ModelTab, id: Option<&str>) -> Option<&ModelRow> {
        self.snapshot
            .installed_models(tab == ModelTab::Text)
            .find(|model| Some(model.id.as_str()) == id)
    }

    fn file_setup_menu(&self, mut menu: PopupMenu, tab: ModelTab) -> PopupMenu {
        let files = &self.snapshot.files;
        if tab == ModelTab::Speakers {
            menu = menu.item(self.file_option(
                tr("ui.files.without_speakers"),
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
                    tf!("ui.model.folder_name", name = name),
                    files.identify_speakers && files.using_local_voices && files.voices_ready,
                    || FileCommand::UseLocalVoices,
                ));
            }
        } else {
            for model in self.snapshot.installed_models(false) {
                let id = model.id.clone();
                menu = menu.item(self.file_option(
                    model.name.clone(),
                    files.speech_model == id,
                    move || FileCommand::SetSpeech(id.clone()),
                ));
            }
        }
        menu
    }

    fn file_menu_links(mut menu: PopupMenu, tab: ModelTab, owner: &WeakEntity<Self>) -> PopupMenu {
        if !menu.is_empty() {
            menu = menu.separator();
        }
        let catalog = owner.clone();
        menu = menu.item(PopupMenuItem::new(tr("ui.model.catalogue_open")).on_click(
            move |_, window, cx| {
                let _ = catalog.update(cx, |view, cx| {
                    view.model_tab = tab;
                    view.file_dialog_error = None;
                    view.navigate(Page::Models, window, cx);
                });
            },
        ));
        if tab == ModelTab::Text {
            let owner = owner.clone();
            menu = menu.item(PopupMenuItem::new(tr("ui.page.editing")).on_click(
                move |_, window, cx| {
                    let _ =
                        owner.update(cx, |view, cx| view.navigate(Page::Processing, window, cx));
                },
            ));
        }
        menu
    }

    fn file_editor_menu(&self, menu: PopupMenu) -> PopupMenu {
        let current = self.snapshot.files.text_model.as_ref();
        let mut menu = menu.item(self.file_option(tr("ui.edit.none"), current.is_none(), || {
            FileCommand::SetText(None)
        }));
        let mut local = self.snapshot.installed_models(true).peekable();
        if local.peek().is_some() {
            menu = menu.label(tr("ui.switcher.on_mac"));
            for model in local {
                let value = FileEditor::Local(model.id.clone());
                menu = menu.item(self.file_option(
                    model.name.clone(),
                    current == Some(&value),
                    move || FileCommand::SetText(Some(value.clone())),
                ));
            }
        }
        for service in EditorSource::services() {
            menu = self.file_service_models(menu.separator().label(service.name()), service);
        }
        menu
    }

    fn file_service_models(&self, mut menu: PopupMenu, service: EditorSource) -> PopupMenu {
        let s = &self.snapshot;
        let choices: Vec<_> = match service {
            EditorSource::Server => {
                let mut models = s.server_models.clone();
                if !s.server_model.is_empty() && !models.contains(&s.server_model) {
                    models.push(s.server_model.clone());
                }
                models
                    .into_iter()
                    .map(|model| {
                        (
                            model.clone(),
                            ServiceModel::Server {
                                endpoint: s.server_url.clone(),
                                model,
                            },
                        )
                    })
                    .collect()
            }
            EditorSource::Api(provider) => provider
                .models()
                .iter()
                .map(|&model| {
                    (
                        model.to_owned(),
                        ServiceModel::Api {
                            provider,
                            model: model.into(),
                        },
                    )
                })
                .collect(),
            EditorSource::Cli(agent) => self
                .agent_ui
                .choices(agent, &s.agent_profiles.get(agent).model)
                .into_iter()
                .map(|(label, model)| (label, ServiceModel::Cli { agent, model }))
                .collect(),
        };
        for (label, target) in choices {
            let value = FileEditor::Service(target);
            menu = menu.item(self.file_option(
                label,
                s.files.text_model.as_ref() == Some(&value),
                move || FileCommand::SetText(Some(value.clone())),
            ));
        }
        if let EditorSource::Server = service {
            let endpoint = s.server_url.clone();
            let engine = self.engine.clone();
            menu = menu.item(PopupMenuItem::new(tr("ui.edit.find_models")).on_click(
                move |_, _, _| {
                    engine
                        .borrow_mut()
                        .dispatch(Command::DiscoverServer(endpoint.clone()));
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
