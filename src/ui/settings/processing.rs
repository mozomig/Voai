use super::*;
use voai_l10n::tr;

impl Settings {
    pub(super) fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let secret = self.key.read(cx).value();
        if secret.trim().is_empty() || self.page != Page::Processing {
            return;
        }
        let current = self.engine.borrow().snapshot();
        let command = match self.snapshot.mode {
            Mode::Cloud
                if current.mode == Mode::Cloud && current.provider == self.snapshot.provider =>
            {
                Command::SaveKey(secret.to_string())
            }
            Mode::Server
                if current.mode == Mode::Server
                    && self.show_server_key
                    && voai_models::normalize_server_url(&self.server_url.read(cx).value())
                        .is_ok_and(|url| url == current.server_url) =>
            {
                Command::SaveServerKey(secret.to_string())
            }
            _ => return,
        };
        if self.engine.borrow_mut().try_dispatch(command) {
            self.key.update(cx, |key, cx| key.set_value("", window, cx));
        }
    }

    pub(super) fn discover_server(&self, cx: &Context<Self>) {
        let draft = self.server_url.read(cx).value();
        if !self.snapshot.server_loading && voai_models::normalize_server_url(&draft).is_ok() {
            self.send(Command::DiscoverServer(draft.to_string()));
        }
    }

    pub(super) fn save_prompt(&self, cx: &Context<Self>) {
        let draft = self.prompt.read(cx).value();
        if draft.as_str() != self.snapshot.processing_prompt
            && voai_models::validate_processing_prompt(&draft).is_ok()
        {
            self.send(Command::SetProcessingPrompt(draft.to_string()));
        }
    }

    pub(super) fn processing(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let s = &self.snapshot;
        let modes = [Mode::Off, Mode::Local, Mode::Server, Mode::Cloud];
        let tabs: Vec<_> = modes
            .iter()
            .map(|&mode| (mode_title(mode), mode == s.mode))
            .collect();
        let panel = match s.mode {
            Mode::Off => col().child(text(tr("ui.edit.off_hint"), 13., p.muted).mt_3()),
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
            .installed_models(true)
            .find(|model| matches!(model.state, ModelState::Selected));
        let choices = self
            .snapshot
            .installed_models(true)
            .map(|m| (m.name.clone(), m.id.clone()))
            .collect::<Vec<_>>();
        col()
            .mt_3()
            .when(!choices.is_empty(), |v| {
                v.child(Self::setting(
                    p,
                    tr("ui.edit.text_model"),
                    "",
                    self.picker(
                        "local-model",
                        selected
                            .map(|m| m.name.clone())
                            .unwrap_or_else(|| tr("ui.action.select").into()),
                        choices,
                        selected.map(|m| m.id.clone()).unwrap_or_default(),
                        Command::SelectModel,
                        p,
                    ),
                ))
            })
            .child(row().mt_2().gap_2().child(
                button("text-models", tr("ui.model.catalogue_open"), p).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.model_tab = ModelTab::Text;
                        this.navigate(Page::Models, window, cx);
                    },
                )),
            ))
    }

    pub(super) fn cloud_provider(&self) -> voai_models::Provider {
        if self.snapshot.provider == "Anthropic" {
            voai_models::Provider::Anthropic
        } else {
            voai_models::Provider::OpenAI
        }
    }

    fn cloud_processor(&self, p: Palette, cx: &Context<Self>) -> Div {
        let s = &self.snapshot;
        let panel = col().mt_2().child(Self::setting(
            p,
            tr("ui.edit.provider"),
            "",
            self.picker(
                "provider",
                s.provider.clone(),
                crate::app::EditorSource::CLOUD
                    .map(|source| source.name())
                    .map(|name| (name.to_owned(), name.to_owned()))
                    .into(),
                s.provider.clone(),
                Command::SetProvider,
                p,
            ),
        ));
        if let Some(agent) = voai_models::AgentCli::from_name(&s.provider) {
            return panel.child(self.agent_processor(agent, p, cx));
        }
        let provider = self.cloud_provider();
        panel
            .child(Self::setting(
                p,
                tr("ui.edit.model"),
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
            .child(text(tr("ui.edit.cloud_privacy"), 12., p.muted).mt_2())
    }

    fn secret_row(&self, server: bool, p: Palette, cx: &Context<Self>) -> Div {
        let has_key = if server {
            self.snapshot.server_has_key
        } else {
            self.snapshot.has_key
        };
        let label = if server {
            tr("ui.edit.server_token")
        } else {
            tr("ui.edit.api_key")
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
            if has_key { tr("ui.edit.key_saved") } else { "" },
            row()
                .gap_1()
                .child(div().w(px(125.)).child(Input::new(&self.key).small()))
                .child(
                    button("save-key", tr("ui.action.save"), p)
                        .disabled(blank)
                        .on_click(cx.listener(|this, _, window, cx| this.save_key(window, cx))),
                )
                .when(has_key, |r| {
                    r.child(button("delete-key", tr("ui.action.delete"), p).on_click(
                        cx.listener(move |this, _, _, _| this.send(command(String::new()))),
                    ))
                }),
        )
    }

    fn server_processor(&self, p: Palette, cx: &Context<Self>) -> Div {
        let s = &self.snapshot;
        let draft = self.server_url.read(cx).value();
        let address = voai_models::normalize_server_url(&draft);
        let saved = address.as_ref().is_ok_and(|url| url == &s.server_url);
        let mut panel = col()
            .mt_3()
            .gap_2()
            .child(label(tr("ui.edit.server_address"), p.ink))
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
                                tr("ui.edit.connecting")
                            } else {
                                tr("ui.edit.find_models")
                            },
                            p,
                        )
                        .disabled(s.server_loading || address.is_err())
                        .on_click(cx.listener(|this, _, _, cx| this.discover_server(cx))),
                    ),
            )
            .children(
                address
                    .err()
                    .filter(|_| !draft.trim().is_empty())
                    .map(|e| text(e.to_string(), 12., p.error)),
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
                                tr("ui.edit.hide_token")
                            } else {
                                tr("ui.edit.token")
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
            panel = panel.child(text(s.server_error.clone(), 12., p.error));
        }
        if saved && (!s.server_models.is_empty() || !s.server_model.is_empty()) {
            let title = if s.server_model.is_empty() {
                tr("ui.action.select").into()
            } else {
                s.server_model.clone()
            };
            if s.server_models.is_empty() {
                panel = panel.child(Self::setting(
                    p,
                    tr("ui.edit.model"),
                    "",
                    text(title, 12., p.ink),
                ));
            } else {
                panel = panel.child(Self::setting(
                    p,
                    tr("ui.edit.model"),
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
        let error = voai_models::validate_processing_prompt(&draft).err();
        let can_save = changed && error.is_none();
        let restoring = draft.as_str() != voai_models::default_processing_prompt()
            || self.snapshot.processing_prompt != voai_models::default_processing_prompt();
        col()
            .mt(px(19.))
            .gap_2()
            .child(
                row()
                    .justify_between()
                    .gap_2()
                    .child(label(tr("ui.edit.instructions"), p.ink))
                    .when(changed, |r| {
                        r.child(text(tr("ui.edit.unsaved"), 12., p.muted))
                    }),
            )
            .child(Input::new(&self.prompt).h(px(110.)).w_full())
            .children(error.map(|e| text(e.to_string(), 12., p.error)))
            .child(
                row()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("reset-prompt")
                            .ghost()
                            .small()
                            .label(tr("ui.edit.restore"))
                            .text_color(p.muted)
                            .disabled(!restoring)
                            .when(!restoring, |b| b.opacity(0.55))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.send(Command::ResetProcessingPrompt);
                                let persisted = this.engine.borrow().snapshot().processing_prompt;
                                if persisted == voai_models::default_processing_prompt() {
                                    this.prompt.update(cx, |input, cx| {
                                        input.set_value(persisted, window, cx)
                                    });
                                }
                            })),
                    )
                    .child(
                        primary_button(
                            "save-prompt",
                            if changed {
                                tr("ui.action.save")
                            } else {
                                tr("ui.edit.saved")
                            },
                            p,
                        )
                        .tooltip(tr("ui.edit.save_tip"))
                        .disabled(!can_save)
                        .when(!can_save, |b| b.opacity(0.55))
                        .on_click(cx.listener(|this, _, _, cx| this.save_prompt(cx))),
                    ),
            )
    }
}
