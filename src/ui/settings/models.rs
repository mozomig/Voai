use super::*;

type ModelCommand = fn(String) -> Command;

impl Settings {
    pub(super) fn models(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let s = &self.snapshot;
        let local = s
            .models
            .iter()
            .filter(|m| m.text_model == self.text_models && m.local_path.is_some());
        let occupied = s
            .models
            .iter()
            .filter(|m| m.local_path.is_none())
            .map(|m| m.on_disk)
            .sum();
        col()
            .child(
                row()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(self.tabs(
                        p,
                        &[("Речь", !self.text_models), ("Текст", self.text_models)],
                        |this, i, cx| {
                            this.text_models = i == 1;
                            this.file_dialog_error = None;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(
                        primary_button(
                            "attach-model",
                            if s.connecting_model {
                                "Проверяю файл…"
                            } else {
                                "Подключить файл…"
                            },
                            p,
                        )
                        .disabled(s.connecting_model)
                        .on_click(cx.listener(|this, _, _, cx| this.choose_file(cx))),
                    ),
            )
            .children(
                self.file_dialog_error
                    .as_ref()
                    .map(|message| text(message.clone(), 12., p.ink).mt_2()),
            )
            .when(local.clone().next().is_some(), |d| {
                d.child(group_title("Подключённые файлы", p))
                    .children(local.map(|m| self.model_row(m, p, cx)))
            })
            .child(group_title("Каталог", p))
            .children(
                s.models
                    .iter()
                    .filter(|m| m.text_model == self.text_models && m.local_path.is_none())
                    .map(|m| self.model_row(m, p, cx)),
            )
            .when(occupied > 0, |d| {
                d.child(
                    row()
                        .mt_4()
                        .gap_2()
                        .child(icon("disk", p.muted, 14.))
                        .child(text(
                            format!("Загружено на диск · {}", bytes(occupied)),
                            12.,
                            p.muted,
                        )),
                )
            })
    }

    fn choose_file(&mut self, cx: &mut Context<Self>) {
        let text_model = self.text_models;
        self.file_dialog_error = None;
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if text_model {
                    "Подключить GGUF"
                } else {
                    "Подключить Whisper GGML"
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
                            view.send(Command::ConnectModel { path, text_model });
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
                details = details.child(text(reason.clone(), 12., rgb(0xffa198).into()));
                action = Some(("Повторить", Command::Download));
            }
            ModelState::Unavailable(reason) => {
                details = details.child(text(reason.clone(), 12., rgb(0xffa198).into()));
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
                    .label("Отвязать")
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
