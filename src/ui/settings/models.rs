use super::*;
use crate::app::FileCommand;

type ModelCommand = fn(String) -> Command;

impl Settings {
    pub(super) fn models(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let s = &self.snapshot;
        let speakers = self.model_tab == ModelTab::Speakers;
        let tabs = [ModelTab::Speech, ModelTab::Text, ModelTab::Speakers];
        let mut page = col()
            .child(
                row()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(self.tabs(
                        p,
                        &[
                            ("Речь", self.model_tab == ModelTab::Speech),
                            ("Текст", self.model_tab == ModelTab::Text),
                            ("Спикеры", speakers),
                        ],
                        move |this, i, cx| {
                            this.model_tab = tabs[i];
                            this.file_dialog_error = None;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(
                        primary_button(
                            "attach-model",
                            if speakers {
                                "Подключить папку…"
                            } else if s.connecting_model {
                                "Проверяю файл…"
                            } else {
                                "Подключить файл…"
                            },
                            p,
                        )
                        .disabled(if speakers {
                            s.files.progress.is_some()
                        } else {
                            s.connecting_model
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.choose_file(cx))),
                    ),
            )
            .children(
                self.file_dialog_error
                    .as_ref()
                    .map(|message| text(message.clone(), 12., p.error).mt_2()),
            );
        if speakers {
            page = page.child(self.voice_models(p, cx));
            return page;
        }
        let text_models = self.model_tab == ModelTab::Text;
        let local = s
            .models
            .iter()
            .filter(|m| m.text_model == text_models && m.local_path.is_some());
        let occupied = s
            .models
            .iter()
            .filter(|m| m.local_path.is_none())
            .map(|m| m.on_disk)
            .sum();
        page.when(local.clone().next().is_some(), |d| {
            d.child(group_title("Подключённые файлы", p))
                .children(local.map(|m| self.model_row(m, p, cx)))
        })
        .child(group_title("Каталог", p))
        .children(
            s.models
                .iter()
                .filter(|m| m.text_model == text_models && m.local_path.is_none())
                .map(|m| self.model_row(m, p, cx)),
        )
        .when(occupied > 0, |d| {
            d.child(
                row()
                    .mt_4()
                    .gap_2()
                    .child(icon("disk", p.muted, 14.))
                    .child(text(
                        format!("На диске · {}", bytes(occupied)),
                        12.,
                        p.muted,
                    )),
            )
        })
    }

    fn choose_file(&mut self, cx: &mut Context<Self>) {
        let tab = self.model_tab;
        let speakers = tab == ModelTab::Speakers;
        self.file_dialog_error = None;
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: !speakers,
            directories: speakers,
            multiple: false,
            prompt: Some(
                match tab {
                    ModelTab::Speech => "Выбрать файл Whisper",
                    ModelTab::Text => "Выбрать файл GGUF",
                    ModelTab::Speakers => "Подключить папку",
                }
                .into(),
            ),
        });
        cx.spawn(async move |view, cx| {
            let selected = picker.await;
            let _ = view.update(cx, |view, cx| {
                match selected {
                    Ok(Ok(Some(files))) => {
                        if let Some(path) = files.into_iter().next() {
                            view.send(if speakers {
                                Command::File(FileCommand::ConnectVoices(path))
                            } else {
                                Command::ConnectModel {
                                    path,
                                    text_model: tab == ModelTab::Text,
                                }
                            });
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        view.file_dialog_error =
                            Some("Не удалось открыть окно выбора файла.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn voice_models(&self, p: Palette, cx: &Context<Self>) -> Div {
        let files = &self.snapshot.files;
        let busy = files.progress.is_some();
        let selected = files.voices_downloaded && !files.using_local_voices && files.voices_ready;
        let action = if selected {
            "Выбрано"
        } else if files.voices_downloaded {
            "Выбрать"
        } else {
            "Скачать"
        };
        let download = button("download-speakers", action, p)
            .disabled(selected || busy)
            .on_click(cx.listener(|this, _, _, _| {
                let choice = if this.snapshot.files.voices_downloaded {
                    FileCommand::UseDownloadedVoices
                } else {
                    FileCommand::InstallVoices
                };
                this.send(Command::File(choice));
            }));
        let mut panel = col().gap_1()
            .child(group_title("Определение спикеров", p))
            .child(Self::setting(p, "Pyannote 3.0 + CAM++", "31,1 МБ · Разделяет реплики по голосам", download))
            .child(text("Свой комплект sherpa-onnx можно подключить из папки с файлами segmentation.onnx и embedding.onnx.", 12., p.muted).mt_2());
        if let Some(path) = &files.voices_local {
            let active = files.using_local_voices && files.voices_ready;
            panel = panel.child(Self::setting(
                p,
                "Подключённая папка",
                &path.to_string_lossy(),
                button(
                    "use-local-speakers",
                    if active {
                        "Выбрано"
                    } else {
                        "Выбрать"
                    },
                    p,
                )
                .disabled(busy || active)
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(Command::File(FileCommand::UseLocalVoices))
                })),
            ));
        }
        if let Some(progress) = &files.progress {
            panel = panel.child(self.file_progress(progress, p, cx));
        }
        panel.children(files.error.as_ref().map(|e| text(e.clone(), 12., p.error)))
    }

    fn model_row(&self, m: &ModelRow, p: Palette, cx: &Context<Self>) -> Div {
        let local = m.local_path.is_some();
        let description = m.description.clone();
        let summary = if local || matches!(m.state, ModelState::Downloading { .. }) {
            m.description.clone()
        } else {
            format!("{} · {}", bytes(m.bytes), m.description)
        };
        let mut details = col()
            .flex_1()
            .min_w_0()
            .gap(px(3.))
            .child(
                row()
                    .gap(px(7.))
                    .when(local, |r| r.child(icon("file", p.muted, 15.)))
                    .child(label(m.name.clone(), p.ink).truncate()),
            )
            .child(
                text(summary, 12., p.muted)
                    .when(local, |d| d.truncate())
                    .id(SharedString::from(format!("model-detail-{}", m.id)))
                    .when(local, |d| {
                        d.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(description.clone())
                                .build(window, cx)
                        })
                    }),
            );
        let mut controls = row().gap_1().flex_shrink_0();
        let mut action: Option<(&str, ModelCommand)> = None;
        match &m.state {
            ModelState::Selected => {
                controls = controls.child(
                    row()
                        .gap_1()
                        .child(icon("check", p.accent, 14.))
                        .child(text("Выбрана", 12., p.accent)),
                )
            }
            ModelState::Available => action = Some(("Скачать", Command::Download)),
            ModelState::Installed => action = Some(("Выбрать", Command::SelectModel)),
            ModelState::Failed(reason) => {
                details = details.child(text(reason.clone(), 12., p.error));
                action = Some((
                    if m.on_disk > 0 {
                        "Продолжить"
                    } else {
                        "Повторить"
                    },
                    Command::Download,
                ));
            }
            ModelState::Unavailable(reason) => {
                details = details.child(text(reason.clone(), 12., p.error));
            }
            ModelState::Downloading { received } => {
                let fraction = *received as f32 / m.bytes.max(1) as f32;
                details = details
                    .child(div().pt_1().child(track(fraction, p)))
                    .child(text(
                        format!(
                            "{} из {} · {:.0}%",
                            bytes(*received),
                            bytes(m.bytes),
                            100. * fraction
                        ),
                        11.,
                        p.muted,
                    ));
                action = Some(("Отменить", Command::CancelDownload));
            }
        }
        if local
            && !matches!(
                &m.state,
                ModelState::Downloading { .. } | ModelState::Unavailable(_)
            )
        {
            details = details.child(text(bytes(m.bytes), 11., p.muted));
        }
        if let Some((title, cmd)) = action {
            let id = m.id.clone();
            controls = controls.child(
                button(
                    SharedString::from(format!("model-{id}")),
                    title.to_owned(),
                    p,
                )
                .on_click(cx.listener(move |this, _, _, _| this.send(cmd(id.clone())))),
            );
        }
        if local {
            let id = m.id.clone();
            controls = controls.child(
                Button::new(SharedString::from(format!("unlink-{id}")))
                    .ghost()
                    .small()
                    .label("Отключить")
                    .text_color(p.muted)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(Command::DisconnectModel(id.clone()))
                    })),
            );
        }
        row()
            .py(px(12.))
            .w_full()
            .gap_3()
            .border_b_1()
            .border_color(p.rule)
            .child(details)
            .child(controls)
    }
}
