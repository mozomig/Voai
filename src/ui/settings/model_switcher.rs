use super::*;
use gpui_component::menu::PopupMenu;
use voai_l10n::{tf, tr};

impl Settings {
    pub(super) fn model_switcher(&self, p: Palette, cx: &Context<Self>) -> Div {
        col()
            .w_full()
            .flex_shrink_0()
            .pt(px(10.))
            .border_t_1()
            .border_color(p.rule)
            .when(matches!(self.page, Page::Files | Page::Telegram), |d| {
                d.child(
                    text(tr("ui.switcher.dictation"), 10., p.muted)
                        .mb_1()
                        .pl_1(),
                )
            })
            .child(
                col()
                    .w_full()
                    .rounded(px(8.))
                    .bg(p.paper)
                    .border_1()
                    .border_color(p.rule)
                    .p(px(3.))
                    .child(self.model_switch(false, p, cx))
                    .child(div().mx(px(6.)).h(px(1.)).bg(p.rule))
                    .child(self.model_switch(true, p, cx)),
            )
    }

    fn model_switch(
        &self,
        processing: bool,
        p: Palette,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let s = &self.snapshot;
        let local = s
            .models
            .iter()
            .find(|m| m.text_model == processing && matches!(m.state, ModelState::Selected));
        let local_name = local.map(|m| m.name.as_str());
        let (title, model) = if processing {
            match s.mode {
                Mode::Off => (tr("ui.page.editing").to_owned(), None),
                Mode::Local => (tr("ui.page.editing").into(), local_name),
                Mode::Server => (
                    tr("ui.switcher.server").into(),
                    (!s.server_model.is_empty()).then_some(s.server_model.as_str()),
                ),
                Mode::Cloud => (
                    tf!("ui.switcher.cloud", provider = &s.provider),
                    Some(if voai_models::AgentCli::from_name(&s.provider).is_some() {
                        if s.agent_config.model.is_empty() {
                            tr("ui.agent.default")
                        } else {
                            s.agent_config.model.as_str()
                        }
                    } else {
                        s.cloud_model.as_str()
                    }),
                ),
            }
        } else {
            (tr("ui.models.speech").into(), local_name)
        };
        let value = model.unwrap_or(if processing && s.mode == Mode::Off {
            tr("ui.switcher.off")
        } else {
            tr("ui.action.select")
        });
        let display = if local.is_some_and(|m| m.local_path.is_some())
            && (!processing || s.mode == Mode::Local)
        {
            value
                .strip_suffix(".bin")
                .or_else(|| value.strip_suffix(".gguf"))
                .unwrap_or(value)
        } else {
            value
        };
        let owner = cx.entity().downgrade();
        Button::new(if processing {
            "quick-processing"
        } else {
            "quick-speech"
        })
        .ghost()
        .w_full()
        .h(px(46.))
        .px(px(6.))
        .rounded(px(5.))
        .when_some(model, |button, name| button.tooltip(name.to_owned()))
        .child(
            row()
                // Both rows share the same content width in the fixed-width sidebar.
                .w(px(132.))
                .min_w_0()
                .gap(px(6.))
                .child(
                    icon(if processing { "sliders" } else { "mic" }, p.muted, 14.).flex_shrink_0(),
                )
                .child(
                    col()
                        .min_w_0()
                        .flex_1()
                        .gap(px(1.))
                        .child(text(title, 10., p.muted).truncate())
                        .child(
                            text(display.to_owned(), 12., p.ink)
                                .font_weight(FontWeight::MEDIUM)
                                .truncate(),
                        ),
                )
                .child(icon("chevron", p.muted, 12.).flex_shrink_0()),
        )
        .dropdown_menu_with_anchor(Corner::BottomLeft, move |menu, _, cx| {
            let Some(view) = owner.upgrade() else {
                return menu;
            };
            view.read(cx).model_menu(menu, processing, &owner)
        })
    }

    fn model_menu(
        &self,
        mut menu: PopupMenu,
        processing: bool,
        owner: &WeakEntity<Self>,
    ) -> PopupMenu {
        let s = &self.snapshot;
        menu = menu.min_w(px(212.)).max_w(px(380.)).scrollable(true);
        if processing {
            let engine = self.engine.clone();
            menu = menu.item(
                PopupMenuItem::new(tr("ui.edit.none"))
                    .checked(s.mode == Mode::Off)
                    .on_click(move |_, _, _| {
                        engine.borrow_mut().dispatch(Command::SetMode(Mode::Off))
                    }),
            );
            match s.mode {
                Mode::Server if !s.server_models.is_empty() => {
                    menu = menu.label(tr("ui.mode.server"));
                    for name in &s.server_models {
                        menu = menu.item(self.model_choice(
                            name,
                            name,
                            name == &s.server_model,
                            Command::SetServerModel,
                        ));
                    }
                }
                Mode::Cloud => {
                    menu = menu.label(tf!("ui.switcher.cloud", provider = &s.provider));
                    if let Some(agent) = voai_models::AgentCli::from_name(&s.provider) {
                        for (label, id) in self.agent_ui.choices(agent, &s.agent_config.model) {
                            menu = menu.item(self.model_choice(
                                &label,
                                &id,
                                id == s.agent_config.model,
                                Command::SetCloudModel,
                            ));
                        }
                    } else {
                        for name in self.cloud_provider().models() {
                            menu = menu.item(self.model_choice(
                                name,
                                name,
                                *name == s.cloud_model,
                                Command::SetCloudModel,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut models = s.installed_models(processing).peekable();
        if models.peek().is_some() {
            if processing {
                menu = menu.label(tr("ui.switcher.on_mac"));
            }
            for m in models {
                let selected = matches!(m.state, ModelState::Selected)
                    && (!processing || s.mode == Mode::Local);
                menu = menu.item(self.model_choice(
                    &m.name,
                    &m.id,
                    selected,
                    if processing {
                        Command::UseTextModel
                    } else {
                        Command::SelectModel
                    },
                ));
            }
        }
        if !menu.is_empty() {
            menu = menu.separator();
        }
        let catalog = owner.clone();
        menu = menu.item(PopupMenuItem::new(tr("ui.model.catalogue_open")).on_click(
            move |_, window, cx| {
                let _ = catalog.update(cx, |view, cx| {
                    view.model_tab = if processing {
                        ModelTab::Text
                    } else {
                        ModelTab::Speech
                    };
                    view.file_dialog_error = None;
                    view.navigate(Page::Models, window, cx);
                });
            },
        ));
        if processing {
            let owner = owner.clone();
            menu = menu.item(
                PopupMenuItem::new(tr("ui.switcher.edit_settings")).on_click(
                    move |_, window, cx| {
                        let _ = owner
                            .update(cx, |view, cx| view.navigate(Page::Processing, window, cx));
                    },
                ),
            );
        }
        menu
    }

    fn model_choice(
        &self,
        title: &str,
        value: &str,
        checked: bool,
        command: fn(String) -> Command,
    ) -> PopupMenuItem {
        let engine = self.engine.clone();
        let value = value.to_owned();
        PopupMenuItem::new(title.to_owned())
            .checked(checked)
            .on_click(move |_, _, _| {
                engine.borrow_mut().dispatch(command(value.clone()));
            })
    }
}
