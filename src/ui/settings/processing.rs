use super::*;

impl Settings {
    pub(super) fn processing(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let s = &self.snapshot;
        let modes = [Mode::Off, Mode::Local, Mode::Server, Mode::Cloud];
        let tabs: Vec<_> = modes
            .iter()
            .map(|&mode| (mode_title(mode), mode == s.mode))
            .collect();
        let panel = match s.mode {
            Mode::Off => col()
                .child(text("Распознанная речь сразу появится у курсора.", 13., p.muted).mt_3()),
            Mode::Local => self.local_processor(p, cx),
            Mode::Server => self.server_processor(p, cx),
            Mode::Cloud => self.cloud_processor(p, cx),
        };
        col()
            .child(row().child(self.tabs(
                p,
                &tabs,
                move |this, i, _| this.send(Command::SetMode(modes[i])),
                cx,
            )))
            .child(panel)
            .child(self.prompt_editor(p, cx))
    }

    fn local_processor(&self, p: Palette, cx: &Context<Self>) -> Div {
        let selected = self
            .snapshot
            .models
            .iter()
            .find(|m| m.text_model && matches!(m.state, ModelState::Selected));
        let choices = self
            .snapshot
            .models
            .iter()
            .filter(|m| {
                m.text_model && matches!(m.state, ModelState::Selected | ModelState::Installed)
            })
            .map(|m| (m.name.clone(), m.id.clone()))
            .collect::<Vec<_>>();
        col()
            .mt_3()
            .when(!choices.is_empty(), |v| {
                v.child(Self::setting(
                    p,
                    "Текстовая модель",
                    "",
                    self.picker(
                        "local-model",
                        selected
                            .map(|m| m.name.clone())
                            .unwrap_or_else(|| "Выбрать".into()),
                        choices,
                        selected.map(|m| m.id.clone()).unwrap_or_default(),
                        Command::SelectModel,
                        p,
                    ),
                ))
            })
            .child(
                row()
                    .mt_2()
                    .gap_2()
                    .child(
                        button("text-models", "Открыть модели", p).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.text_models = true;
                                this.navigate(Page::Models, window, cx);
                            },
                        )),
                    ),
            )
    }

    fn cloud_processor(&self, p: Palette, cx: &Context<Self>) -> Div {
        let s = &self.snapshot;
        let provider = if s.provider == "Anthropic" {
            voai_models::Provider::Anthropic
        } else {
            voai_models::Provider::OpenAI
        };
        col()
            .mt_2()
            .child(Self::setting(
                p,
                "Провайдер",
                "",
                self.picker(
                    "provider",
                    s.provider.clone(),
                    ["OpenAI", "Anthropic"]
                        .map(|name| (name.to_owned(), name.to_owned()))
                        .into(),
                    s.provider.clone(),
                    Command::SetProvider,
                    p,
                ),
            ))
            .child(Self::setting(
                p,
                "Модель",
                "",
                self.picker(
                    "cloud-model",
                    s.cloud_model.clone(),
                    provider
                        .models()
                        .iter()
                        .map(|name| (name.to_string(), name.to_string()))
                        .collect(),
                    s.cloud_model.clone(),
                    Command::SetCloudModel,
                    p,
                ),
            ))
            .child(self.secret_row(false, p, cx))
            .child(
                text(
                    "Провайдер получит расшифровку и инструкцию. Аудио остаётся на Mac.",
                    12.,
                    p.muted,
                )
                .mt_2(),
            )
    }

    fn secret_row(&self, server: bool, p: Palette, cx: &Context<Self>) -> Div {
        let has_key = if server {
            self.snapshot.server_has_key
        } else {
            self.snapshot.has_key
        };
        let label = if server {
            "Токен сервера"
        } else {
            "API-ключ"
        };
        let command = if server {
            Command::SaveServerKey
        } else {
            Command::SaveKey
        };
        let blank = self.key.read(cx).value().trim().is_empty();
        Self::setting(
            p,
            label,
            if has_key {
                "В Связке ключей"
            } else {
                ""
            },
            row()
                .gap_1()
                .child(div().w(px(125.)).child(Input::new(&self.key).small()))
                .child(
                    button("save-key", "Сохранить", p)
                        .disabled(blank)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let secret = this.key.read(cx).value().to_string();
                            if secret.trim().is_empty() {
                                return;
                            }
                            this.send(command(secret));
                            this.key.update(cx, |key, cx| key.set_value("", window, cx));
                        })),
                )
                .when(has_key, |r| {
                    r.child(button("delete-key", "Удалить", p).on_click(
                        cx.listener(move |this, _, _, _| this.send(command(String::new()))),
                    ))
                }),
        )
    }

    fn server_processor(&self, p: Palette, cx: &Context<Self>) -> Div {
        let s = &self.snapshot;
        let draft = self.server_url.read(cx).value();
        let saved =
            voai_models::normalize_server_url(draft.as_str()).is_ok_and(|url| url == s.server_url);
        let mut panel = col()
            .mt_3()
            .gap_2()
            .child(label("Адрес сервера", p.ink))
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.server_url).small()),
                    )
                    .child(
                        button(
                            "discover",
                            if s.server_loading {
                                "Подключаюсь…"
                            } else {
                                "Получить модели"
                            },
                            p,
                        )
                        .disabled(s.server_loading || draft.trim().is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send(Command::DiscoverServer(
                                this.server_url.read(cx).value().to_string(),
                            ))
                        })),
                    ),
            )
            .child(
                row()
                    .flex_wrap()
                    .gap_2()
                    .children(
                        [
                            ("Ollama", "http://127.0.0.1:11434/v1"),
                            ("LM Studio", "http://127.0.0.1:1234/v1"),
                        ]
                        .map(|(name, url)| {
                            Button::new(name)
                                .ghost()
                                .small()
                                .label(name)
                                .text_color(p.accent)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.server_url
                                        .update(cx, |input, cx| input.set_value(url, window, cx))
                                }))
                        }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("server-token")
                            .ghost()
                            .small()
                            .label(if self.show_server_key {
                                "Скрыть токен"
                            } else {
                                "Токен…"
                            })
                            .text_color(p.muted)
                            .disabled(!saved)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_server_key = !this.show_server_key;
                                this.key.update(cx, |key, cx| key.set_value("", window, cx));
                                cx.notify()
                            })),
                    ),
            );
        if self.show_server_key && saved {
            panel = panel.child(self.secret_row(true, p, cx));
        }
        if saved && !s.server_error.is_empty() {
            panel = panel.child(text(s.server_error.clone(), 12., rgb(0xffa198).into()));
        }
        if saved && (!s.server_models.is_empty() || !s.server_model.is_empty()) {
            let title = if s.server_model.is_empty() {
                "Выбрать".into()
            } else {
                s.server_model.clone()
            };
            if s.server_models.is_empty() {
                panel = panel.child(Self::setting(p, "Модель", "", text(title, 12., p.ink)));
            } else {
                panel = panel.child(Self::setting(
                    p,
                    "Модель",
                    "",
                    self.picker(
                        "server-model",
                        title,
                        s.server_models
                            .iter()
                            .map(|name| (name.clone(), name.clone()))
                            .collect(),
                        s.server_model.clone(),
                        Command::SetServerModel,
                        p,
                    ),
                ));
            }
        }
        panel
    }

    fn prompt_editor(&self, p: Palette, cx: &Context<Self>) -> Div {
        let draft = self.prompt.read(cx).value();
        let changed = draft.as_str() != self.snapshot.processing_prompt;
        let limit = voai_models::MAX_PROMPT_BYTES;
        let invalid = draft.trim().is_empty() || draft.len() > limit;
        let restoring = draft.as_str() != voai_models::DEFAULT_PROCESSING_PROMPT
            || self.snapshot.processing_prompt != voai_models::DEFAULT_PROCESSING_PROMPT;
        col()
            .mt(px(19.))
            .gap_2()
            .child(label("Инструкция для обработки", p.ink))
            .child(Input::new(&self.prompt).h(px(110.)).w_full())
            .child(
                row()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("reset-prompt")
                            .ghost()
                            .small()
                            .label("По умолчанию")
                            .text_color(p.muted)
                            .disabled(!restoring)
                            .when(!restoring, |b| b.opacity(0.55))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.send(Command::ResetProcessingPrompt);
                                let persisted = this.engine.borrow().snapshot().processing_prompt;
                                if persisted == voai_models::DEFAULT_PROCESSING_PROMPT {
                                    this.prompt.update(cx, |input, cx| {
                                        input.set_value(persisted, window, cx)
                                    });
                                }
                            })),
                    )
                    .child(
                        row()
                            .gap_2()
                            .when(invalid, |r| {
                                r.child(text(
                                    if draft.trim().is_empty() {
                                        "Введите инструкцию".to_owned()
                                    } else {
                                        format!("Максимум {limit} байт")
                                    },
                                    12.,
                                    p.muted,
                                ))
                            })
                            .child(
                                primary_button("save-prompt", "Сохранить", p)
                                    .disabled(!changed || invalid)
                                    .when(!changed || invalid, |b| b.opacity(0.55))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.send(Command::SetProcessingPrompt(
                                            this.prompt.read(cx).value().to_string(),
                                        ))
                                    })),
                            ),
                    ),
            )
    }
}
