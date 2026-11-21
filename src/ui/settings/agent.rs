use super::*;
use voai_models::{AgentCli, AgentConfig};

pub(super) struct AgentControls {
    path: Entity<InputState>,
    model: Entity<InputState>,
    effort: String,
    codex_models: Vec<String>,
}
fn path_hint(s: &Snapshot) -> String {
    AgentCli::from_name(&s.provider)
        .and_then(AgentCli::detect)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| tr("ui.agent.auto").to_owned())
}

impl AgentControls {
    pub(super) fn new(s: &Snapshot, window: &mut Window, cx: &mut Context<Settings>) -> Self {
        let field =
            |value: &str, placeholder: String, window: &mut Window, cx: &mut Context<Settings>| {
                let field = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(value.to_owned())
                        .placeholder(placeholder)
                });
                cx.subscribe_in(&field, window, |this, _, event, _, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.save_agent(cx),
                    _ => {}
                })
                .detach();
                field
            };
        Self {
            path: field(&s.agent_config.path, path_hint(s), window, cx),
            model: field(
                &s.agent_config.model,
                tr("ui.agent.default").to_owned(),
                window,
                cx,
            ),
            effort: s.agent_config.effort.clone(),
            codex_models: AgentCli::Codex.model_choices(),
        }
    }
    pub(super) fn sync(
        &mut self,
        prev: &Snapshot,
        next: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) {
        let switched = prev.provider != next.provider;
        if switched || prev.agent_config.effort != next.agent_config.effort {
            self.effort.clone_from(&next.agent_config.effort);
        }
        for (field, before, after, key) in [
            (
                &self.path,
                &prev.agent_config.path,
                &next.agent_config.path,
                "ui.agent.auto",
            ),
            (
                &self.model,
                &prev.agent_config.model,
                &next.agent_config.model,
                "ui.agent.default",
            ),
        ] {
            if switched || before != after {
                field.update(cx, |f, cx| f.set_value(after.clone(), window, cx));
            }
            if switched || prev.interface_language != next.interface_language {
                let hint = if key == "ui.agent.auto" {
                    path_hint(next)
                } else {
                    tr(key).to_owned()
                };
                field.update(cx, |f, cx| f.set_placeholder(hint, window, cx));
            }
        }
    }
    pub(super) fn choices(&self, agent: AgentCli, current: &str) -> Vec<(String, String)> {
        let mut models = match agent {
            AgentCli::Claude => agent.model_choices(),
            AgentCli::Codex => self.codex_models.clone(),
        };
        if !current.is_empty() && !models.iter().any(|s| s == current) {
            models.push(current.to_owned())
        }
        std::iter::once((tr("ui.agent.default").to_owned(), String::new()))
            .chain(models.into_iter().map(|m| (m.clone(), m)))
            .collect()
    }
    fn draft(&self, cx: &App) -> AgentConfig {
        AgentConfig {
            path: self.path.read(cx).value().trim().into(),
            model: self.model.read(cx).value().trim().into(),
            effort: self.effort.clone(),
        }
    }
}
impl Settings {
    fn save_agent(&self, cx: &Context<Self>) {
        if let Some(agent) = AgentCli::from_name(&self.snapshot.provider) {
            self.send(Command::ConfigureAgent(agent, self.agent_ui.draft(cx)));
        }
    }
    pub(super) fn agent_processor(&self, agent: AgentCli, p: Palette, cx: &Context<Self>) -> Div {
        let fields = &self.agent_ui;
        let draft = fields.draft(cx);
        let choices = fields.choices(agent, &draft.model);
        let input = fields.model.clone();
        let error = draft.validate(agent).err();
        let owner = cx.entity().downgrade();
        let effort = fields.effort.clone();
        let level_name = |id: &str| {
            if id.is_empty() {
                tr("ui.agent.default").to_owned()
            } else {
                id.to_owned()
            }
        };
        let effort_menu =
            button("agent-effort", level_name(&effort), p).dropdown_menu(move |mut menu, _, _| {
                for &id in std::iter::once(&"").chain(agent.efforts()) {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(level_name(id))
                            .checked(effort == id)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.agent_ui.effort = id.to_owned();
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        col()
            .gap_2()
            .mt_2()
            .child(text(tr("ui.agent.privacy"), 12., p.muted))
            .child(label(tr("ui.edit.model"), p.ink))
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&fields.model).small()),
                    )
                    .child(
                        button("agent-models", tr("ui.action.select"), p).dropdown_menu(
                            move |mut menu, _, _| {
                                for (label, id) in &choices {
                                    let value = id.clone();
                                    let input = input.clone();
                                    menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                                        move |_, window, cx| {
                                            input.update(cx, |f, cx| {
                                                f.set_value(value.clone(), window, cx)
                                            })
                                        },
                                    ));
                                }
                                menu
                            },
                        ),
                    ),
            )
            .child(Self::setting(p, tr("ui.agent.effort"), "", effort_menu))
            .child(label(tr("ui.agent.path"), p.ink))
            .child(Input::new(&fields.path).small().w_full())
            .children(error.as_ref().map(|e| text(e.to_string(), 12., p.error)))
            .child(
                row().justify_end().child(
                    button("save-agent", tr("ui.action.save"), p)
                        .disabled(error.is_some() || draft == self.snapshot.agent_config)
                        .on_click(cx.listener(|this, _, _, cx| this.save_agent(cx))),
                ),
            )
    }
}
